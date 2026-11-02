//! The API Nodes (Seedance) video engine: the ComfyUI API key the server keeps, the graph for ComfyUI, the scene audio excerpt and the
//! prompt conversion. ComfyUI and the LLM are small local servers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::Client;
use serde_json::{json, Value};

/// What the page sends in place of the key (`COMFY_KEY_PLACEHOLDER` of the server).
const PLACEHOLDER: &str = "h3-studio:saved-comfy-api-key";

struct Comfy {
    url: String,
    /// The jobs sent to `POST /prompt`.
    jobs: Arc<Mutex<Vec<Value>>>,
    input: PathBuf,
    output: PathBuf,
}

/// A ComfyUI that knows its folders and the given nodes, and keeps the jobs it is sent.
async fn fake_comfy(dir: &Path, nodes: &[&str]) -> Comfy {
    let (input, output) = (dir.join("comfy").join("input"), dir.join("comfy").join("output"));
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    let main = dir.join("comfy").join("main.py").display().to_string();
    let stats = json!({"system": {"os": "nt", "comfyui_version": "0", "python_version": "3", "argv": [main, "--output-directory", output.display().to_string(), "--input-directory", input.display().to_string()]}, "devices": []});
    let folders = json!({"custom_nodes": [dir.join("comfy").join("custom_nodes").display().to_string()]});
    let info: serde_json::Map<String, Value> = nodes.iter().map(|name| (name.to_string(), json!({"input": {}, "output": [], "name": name}))).collect();
    let jobs: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = Arc::clone(&jobs);
    let app = Router::new()
        .route("/system_stats", get(move || async move { Json(stats) }))
        .route("/internal/folder_paths", get(move || async move { Json(folders) }))
        .route("/object_info", get(move || async move { Json(Value::Object(info)) }))
        .route(
            "/prompt",
            post(move |Json(body): Json<Value>| async move {
                sink.lock().unwrap().push(body);
                Json(json!({"prompt_id": "p1", "number": 1, "node_errors": {}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Comfy { url: format!("http://{addr}"), jobs, input, output }
}

/// The app, pointed at the given ComfyUI. Returns its address.
async fn start(dir: &Path, comfy: &str) -> String {
    let cfg = AppConfig { comfy_url: comfy.to_string(), project_root: dir.join("projects"), ..AppConfig::default() };
    std::fs::create_dir_all(&cfg.project_root).unwrap();
    let state = h3_api::build_state(cfg, &dir.join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    format!("http://{addr}")
}

async fn send(client: &Client, method: reqwest::Method, url: String, body: Value) -> (u16, Value) {
    let reply = client.request(method, url).json(&body).send().await.unwrap();
    let status = reply.status().as_u16();
    (status, reply.json().await.unwrap_or(Value::Null))
}

const ALL_NODES: [&str; 5] = ["ByteDance2ReferenceNodeV2", "SaveVideo", "LoadImage", "LoadVideo", "LoadAudio"];

#[tokio::test]
async fn the_comfyui_key_is_kept_on_the_server_and_added_to_a_job() {
    let dir = tempfile::tempdir().unwrap();
    let comfy = fake_comfy(dir.path(), &ALL_NODES).await;
    let app = start(dir.path(), &comfy.url).await;
    let client = Client::new();
    let status = format!("{app}/api/v1/setup/comfy_api_key");

    // Nothing is saved at first. A key with a space in it is refused. The key is never returned.
    let (code, body) = send(&client, reqwest::Method::GET, status.clone(), json!(null)).await;
    assert_eq!((code, body["data"]["configured"].clone()), (200, json!(false)));
    let (code, _) = send(&client, reqwest::Method::PUT, status.clone(), json!({"api_key": "two words"})).await;
    assert_eq!(code, 400);
    let (code, _) = send(&client, reqwest::Method::PUT, status.clone(), json!({})).await;
    assert_eq!(code, 400);
    let (code, body) = send(&client, reqwest::Method::PUT, status.clone(), json!({"api_key": "  comfyui-secret-123  "})).await;
    assert_eq!((code, body["data"]["configured"].clone()), (200, json!(true)));
    assert!(!body.to_string().contains("comfyui-secret-123"));

    // A job that carries the placeholder reaches ComfyUI with the saved key. Any other key is left as the page sent it.
    let job = |key: &str| json!({"prompt": {"1": {"class_type": "SaveVideo", "inputs": {}}}, "client_id": "c", "extra_data": {"api_key_comfy_org": key, "comfy_usage_source": "vrgdg-video-builder"}});
    let (code, _) = send(&client, reqwest::Method::POST, format!("{app}/prompt"), job(PLACEHOLDER)).await;
    assert_eq!(code, 200);
    let (code, _) = send(&client, reqwest::Method::POST, format!("{app}/prompt"), job("their-own-key")).await;
    assert_eq!(code, 200);
    {
        let jobs = comfy.jobs.lock().unwrap();
        assert_eq!(jobs[0]["extra_data"]["api_key_comfy_org"], "comfyui-secret-123");
        assert_eq!(jobs[0]["extra_data"]["comfy_usage_source"], "vrgdg-video-builder");
        assert_eq!(jobs[0]["client_id"], "c");
        assert_eq!(jobs[1]["extra_data"]["api_key_comfy_org"], "their-own-key");
    }

    // With the key removed, the placeholder is dropped instead of being sent as if it were a key.
    let (_, body) = send(&client, reqwest::Method::PUT, status, json!({"api_key": ""})).await;
    assert_eq!(body["data"]["configured"], false);
    send(&client, reqwest::Method::POST, format!("{app}/prompt"), job(PLACEHOLDER)).await;
    let jobs = comfy.jobs.lock().unwrap();
    assert!(jobs[2]["extra_data"].get("api_key_comfy_org").is_none(), "{}", jobs[2]);
}

#[tokio::test]
async fn the_seedance_graph_stages_the_references_in_comfyuis_input_folder() {
    let dir = tempfile::tempdir().unwrap();
    let comfy = fake_comfy(dir.path(), &ALL_NODES).await;
    let app = start(dir.path(), &comfy.url).await;
    let client = Client::new();
    let url = format!("{app}/vrgdg/workflow_runner/build_seedance25_reference_prompt");

    let picture = dir.path().join("Ava.png");
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30])).save(&picture).unwrap();
    let sound = dir.path().join("excerpt.wav");
    std::fs::write(&sound, b"RIFF").unwrap();
    let request = json!({
        "prompt": "Ava walks across the roof.", "duration": 6.2, "resolution": "1080p", "ratio": "9:16", "generate_audio": false, "seed": 5,
        "reference_images": [{"path": picture.display().to_string()}, {"data": format!("data:image/png;base64,{}", base64_png())}],
        "reference_audios": [sound.display().to_string()],
    });
    let (code, body) = send(&client, reqwest::Method::POST, url.clone(), request).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!((body["generation_duration"].clone(), body["timeline_duration"].clone()), (json!(7), json!(6.2)));
    assert_eq!(body["output_root"], comfy.output.display().to_string());
    let seedance = &body["prompt"]["seedance"]["inputs"];
    assert_eq!(seedance["model"], "Seedance 2.5");
    assert_eq!((seedance["model.resolution"].clone(), seedance["model.ratio"].clone(), seedance["model.duration"].clone()), (json!("1080p"), json!("9:16"), json!(7)));
    assert_eq!((seedance["model.generate_audio"].clone(), seedance["seed"].clone()), (json!(false), json!(5)));
    assert_eq!(seedance["model.reference_images.image_1"], json!(["image_1", 0]));
    assert_eq!(seedance["model.reference_images.image_2"], json!(["image_2", 0]));
    assert_eq!(seedance["model.reference_audios.audio_1"], json!(["audio_1", 0]));
    for (node, kind) in [("image_1", "LoadImage"), ("image_2", "LoadImage"), ("audio_1", "LoadAudio")] {
        assert_eq!(body["prompt"][node]["class_type"], kind);
        let field = if kind == "LoadImage" { "image" } else { "audio" };
        let staged = body["prompt"][node]["inputs"][field].as_str().unwrap();
        assert!(staged.starts_with("vrgdg_seedance_"), "{staged}");
        assert!(comfy.input.join(staged).is_file(), "{staged} is in ComfyUI's input folder");
    }
    assert_eq!(body["prompt"]["save_video"]["inputs"]["video"], json!(["seedance", 0]));

    // What cannot be sent is refused before anything is staged.
    let refused = |request: Value| send(&client, reqwest::Method::POST, url.clone(), request);
    let (code, body) = refused(json!({"prompt": "x", "duration": 31, "reference_images": [{"path": picture.display().to_string()}]})).await;
    assert_eq!((code, body["error"].as_str().unwrap().contains("at most 30 seconds")), (400, true), "{body}");
    let (code, body) = refused(json!({"prompt": "x", "ratio": "5:4", "reference_images": [{"path": picture.display().to_string()}]})).await;
    assert_eq!((code, body["error"].clone()), (400, json!("Unsupported Seedance aspect ratio.")));
    let (code, body) = refused(json!({"prompt": "x"})).await;
    assert_eq!((code, body["error"].clone()), (400, json!("Reference to Video needs at least one image, video, or audio reference.")));
    let (code, body) = refused(json!({"prompt": "x", "reference_images": [{"path": "Z:/nowhere.png"}]})).await;
    assert!(code == 400 && body["error"].as_str().unwrap().contains("reference was not found"), "{body}");
    let (code, body) = refused(json!({"prompt": "x", "audio_mode": "scene_audio", "reference_images": [{"path": picture.display().to_string()}]})).await;
    assert!(code == 400 && body["error"].as_str().unwrap().contains("exactly one prepared"), "{body}");
}

#[tokio::test]
async fn a_comfyui_without_the_partner_nodes_is_told_so() {
    let dir = tempfile::tempdir().unwrap();
    let comfy = fake_comfy(dir.path(), &["LoadImage"]).await;
    let app = start(dir.path(), &comfy.url).await;
    let (code, body) = send(&Client::new(), reqwest::Method::POST, format!("{app}/vrgdg/workflow_runner/build_seedance25_reference_prompt"), json!({"prompt": "x", "reference_images": []})).await;
    assert_eq!(code, 400);
    assert_eq!(body["error"], "Update ComfyUI and enable API nodes. Missing nodes: ByteDance2ReferenceNodeV2, SaveVideo");
}

fn base64_png() -> String {
    use base64::Engine;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30])).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
}

/// A language model server that answers every request with the given text.
async fn fake_llm(answer: Arc<Mutex<String>>) -> String {
    let app = Router::new()
        .route("/v1/chat/completions", post(|State(answer): State<Arc<Mutex<String>>>| async move { Json(json!({"choices": [{"message": {"content": answer.lock().unwrap().clone()}}]})) }))
        .with_state(answer);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn a_prompt_is_backed_up_and_laid_out_without_losing_a_block() {
    let dir = tempfile::tempdir().unwrap();
    let comfy = fake_comfy(dir.path(), &ALL_NODES).await;
    let app = start(dir.path(), &comfy.url).await;
    let client = Client::new();
    let project = dir.path().join("projects").join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let answer = Arc::new(Mutex::new(String::new()));
    let llm = fake_llm(Arc::clone(&answer)).await;

    // The backup comes first and is refused without scenes, a folder or a prompt.
    let backup_url = format!("{app}/vrgdg/music_builder/backup_seedance_prompts");
    let scene = json!({"id": "s1", "label": "Scene 1", "source_prompt": "[Shot 1] <Picture 1> sings <d>[English] hold me</d>.\n\n[Shot 2] A wide shot.", "image_references": [{"label": "Ava"}], "reference_aliases": {}, "extra_field": "left out"});
    let (code, body) = send(&client, reqwest::Method::POST, backup_url.clone(), json!({"project_folder": project.display().to_string(), "scenes": []})).await;
    assert_eq!((code, body["error"].clone()), (400, json!("No scene prompts were supplied for backup.")));
    let (code, _) = send(&client, reqwest::Method::POST, backup_url.clone(), json!({"project_folder": "", "scenes": [scene.clone()]})).await;
    assert_eq!(code, 400);
    let (code, _) = send(&client, reqwest::Method::POST, backup_url.clone(), json!({"project_folder": project.display().to_string(), "scenes": [scene.clone(), scene.clone()]})).await;
    assert_eq!(code, 400, "a scene ID must be unique");
    let (code, body) = send(&client, reqwest::Method::POST, backup_url, json!({"project_folder": project.display().to_string(), "video_engine": "api_nodes", "scenes": [scene]})).await;
    assert_eq!(code, 200, "{body}");
    let id = body["backup_id"].as_str().unwrap().to_string();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(body["backup_path"].as_str().unwrap()).unwrap()).unwrap();
    assert!(project.join("project_context").join("prompt_backups").join(format!("seedance_{id}.json")).is_file());
    assert_eq!((saved["schema_version"].clone(), saved["video_engine"].clone(), saved["scenes"][0].get("extra_field").is_none()), (json!(1), json!("api_nodes"), true));

    // The LLM only lays the blocks out. A plan that drops one is refused and nothing is returned.
    let convert_url = format!("{app}/vrgdg/music_builder/convert_seedance_prompt");
    let request = |scene: &str| json!({"project_folder": project.display().to_string(), "backup_id": id, "scene_id": scene, "text_runner": "own_server", "own_server_url": llm, "own_server_model": "m"});
    *answer.lock().unwrap() = json!({"sections": [{"heading": "Shots", "block_ids": ["b1", "b2"]}]}).to_string();
    let (code, body) = send(&client, reqwest::Method::POST, convert_url.clone(), request("s1")).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["prompt"], "Shots:\n[Shot 1] @Image 1 sings (English) \"hold me\".\n\n[Shot 2] A wide shot.");
    assert_eq!(body["preserved_blocks"], 2);
    *answer.lock().unwrap() = json!({"sections": [{"heading": "Shots", "block_ids": ["b1"]}]}).to_string();
    let (code, body) = send(&client, reqwest::Method::POST, convert_url.clone(), request("s1")).await;
    assert!(code == 400 && body["error"].as_str().unwrap().contains("dropped, duplicated, or reordered"), "{body}");
    let (code, body) = send(&client, reqwest::Method::POST, convert_url.clone(), request("nope")).await;
    assert_eq!((code, body["error"].clone()), (400, json!("Scene was not found in the saved prompt backup.")));
    let (code, body) = send(&client, reqwest::Method::POST, convert_url, json!({"project_folder": project.display().to_string(), "backup_id": "not-an-id", "scene_id": "s1"})).await;
    assert_eq!((code, body["error"].clone()), (400, json!("Save a prompt backup before converting.")));
}

/// A WAV file of silence, written by hand so the test needs no audio library.
fn silent_wav(path: &Path, seconds: u32) {
    let rate = 8000u32;
    let data = vec![0u8; (rate * seconds * 2) as usize];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn a_scenes_audio_is_cut_and_padded_to_whole_seconds() {
    if h3_media::Tools::find("ffmpeg", "ffprobe").is_err() {
        eprintln!("ffmpeg is not installed: the audio excerpt is not tested");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let comfy = fake_comfy(dir.path(), &ALL_NODES).await;
    let app = start(dir.path(), &comfy.url).await;
    let client = Client::new();
    let url = format!("{app}/vrgdg/workflow_runner/prepare_seedance_scene_audio");
    let song = dir.path().join("song.wav");
    silent_wav(&song, 10);
    let project = dir.path().join("projects").join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    let ask = |extra: Value| {
        let mut body = json!({"source_path": song.display().to_string(), "project_folder": project.display().to_string()});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body
    };

    // 2.5 seconds of the song become 4: Seedance makes whole seconds and at least four.
    let (code, body) = send(&client, reqwest::Method::POST, url.clone(), ask(json!({"start": 1.0, "duration": 2.5}))).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!((body["generation_duration"].clone(), body["duration"].clone()), (json!(4), json!(2.5)));
    let path = PathBuf::from(body["audio_path"].as_str().unwrap());
    assert!(path.starts_with(project.join("seedance_scene_audio")));
    let wav = hound::WavReader::open(&path).unwrap();
    let seconds = f64::from(wav.duration()) / f64::from(wav.spec().sample_rate);
    assert!((seconds - 4.0).abs() < 0.1, "{seconds} seconds");

    // A scene past the end of the song, a scene too long and a missing file are refused.
    let (code, body) = send(&client, reqwest::Method::POST, url.clone(), ask(json!({"start": 12.0, "duration": 2.0}))).await;
    assert!(code == 400 && body["error"].as_str().unwrap().contains("starts after its source audio ends"), "{body}");
    let (code, body) = send(&client, reqwest::Method::POST, url.clone(), ask(json!({"start": 0.0, "duration": 31.0}))).await;
    assert!(code == 400 && body["error"].as_str().unwrap().contains("at most 30 seconds"), "{body}");
    let (code, body) = send(&client, reqwest::Method::POST, url.clone(), ask(json!({"start": 0.0, "duration": 3.0, "source_duration": 5.0}))).await;
    assert_eq!((code, body["error"].clone()), (400, json!("Invalid custom audio clip placement or duration.")));
    let (code, _) = send(&client, reqwest::Method::POST, url, json!({"source_path": dir.path().join("nope.wav").display().to_string(), "project_folder": project.display().to_string(), "start": 0, "duration": 2})).await;
    assert_eq!(code, 400);
}
