//! The render phases of a scene (`render_phase`): the whole render, pass 1 only as a draft, and pass 2 from the saved draft. What each
//! phase prepares, what it refuses and what a finished scene records.

use std::path::Path;
use std::process::Command;

use h3_comfy::ComfyDirs;
use h3_core::timing::TimingPlan;
use h3_graph::env::{GraphEnv, LatentFacts};
use h3_pipeline::{build_draft_graph, finish_scene, prepare_finalize, stage_fields, FinishOptions, LiveEnv, Prepared, RenderError, RenderOutcome, RenderParams, RenderPhase};
use h3_project::session::LoadedSession;
use serde_json::{json, Value};

type Obj = serde_json::Map<String, Value>;

/// The environment asks only for the installed LoRAs here.
struct Env(Vec<String>);
impl GraphEnv for Env {
    fn is_file(&self, _: &str) -> bool { false }
    fn is_dir(&self, _: &str) -> bool { true }
    fn ensure_dir(&self, _: &str) {}
    fn node_info(&self, _: &str) -> Option<Value> { None }
    fn model_choices(&self, category: &str) -> Vec<String> { if category == "loras" { self.0.clone() } else { vec![] } }
    fn output_dir(&self) -> String { String::new() }
    fn probe_duration(&self, _: &str) -> Result<f64, String> { Ok(10.0) }
    fn trim_audio(&self, _: &str, _: &str, _: u32, _: &TimingPlan) -> Result<Value, String> { Err("not used".into()) }
    fn latent_exists(&self, _: &str, _: u32) -> bool { false }
    fn latent_path(&self, _: &str, _: u32) -> String { String::new() }
    fn latent_facts(&self, _: &str, _: u32) -> LatentFacts { unimplemented!("not used") }
    fn pass1_latent_matches(&self, _: &str, _: u32) -> bool { false }
    fn random_seed(&self) -> u64 { 1 }
}

fn object(value: Value) -> Obj {
    value.as_object().cloned().unwrap()
}

#[test]
fn a_phase_is_read_like_the_pack_does() {
    for (sent, phase) in [(json!(null), RenderPhase::Full), (json!(""), RenderPhase::Full), (json!("full"), RenderPhase::Full), (json!(" Draft "), RenderPhase::Draft), (json!("FINALIZE"), RenderPhase::Finalize)] {
        let params = RenderParams { phase: sent.clone(), ..RenderParams::default() };
        assert_eq!(params.phase().unwrap(), phase, "{sent}");
    }
    assert_eq!(RenderParams::default().phase().unwrap(), RenderPhase::Full, "nothing sent");
    let error = RenderParams { phase: json!("pass2"), ..RenderParams::default() }.phase().unwrap_err();
    assert_eq!(error.to_string(), "Unknown render_phase 'pass2'. Supported: full, draft, finalize.");
    assert_eq!((RenderPhase::Full.name(), RenderPhase::Draft.name(), RenderPhase::Finalize.name()), ("full", "draft", "finalize"));
}

#[test]
fn a_scene_records_which_pass_made_its_clip() {
    let two_pass = object(json!({"two_pass": {}, "timing": {"fps": 24}, "draft_id": "d1"}));
    let single = object(json!({"timing": {}}));

    let draft = stage_fields(RenderPhase::Draft, "C:/v/clip.mp4", &two_pass);
    assert_eq!(draft["minimax_h3_stage1_path"], "C:/v/clip.mp4");
    assert_eq!(draft["minimax_h3_stage1_source_path"], "C:/v/clip.mp4");
    assert_eq!((draft["minimax_h3_stage2_path"].clone(), draft["video_source_path"].clone()), (json!(""), json!("C:/v/clip.mp4")));
    assert_eq!(draft["minimax_h3_draft"], json!({"id": "d1", "video_path": "C:/v/clip.mp4"}));
    assert_eq!(draft["minimax_h3_timing"], json!({"fps": 24}));

    let finished = stage_fields(RenderPhase::Finalize, "C:/v/clip.mp4", &two_pass);
    assert_eq!((finished["minimax_h3_stage1_path"].clone(), finished["minimax_h3_stage2_path"].clone()), (json!(""), json!("C:/v/clip.mp4")));
    assert_eq!(finished["minimax_h3_draft"], Value::Null, "a finished scene has no draft");

    // A render in one pass has no second pass. An empty `advanced_two_pass` still marks a two pass graph.
    assert_eq!(stage_fields(RenderPhase::Full, "x.mp4", &single)["minimax_h3_stage2_path"], "");
    assert_eq!(stage_fields(RenderPhase::Full, "x.mp4", &object(json!({"advanced_two_pass": {}})))["minimax_h3_stage2_path"], "x.mp4");
    assert_eq!(stage_fields(RenderPhase::Full, "x.mp4", &single)["minimax_h3_timing"], json!({}));
}

fn project(dir: &Path, segment_extra: Value, settings: Value) -> LoadedSession {
    let mut segment = object(json!({"id": "a", "start": 10.0, "end": 14.5, "label": "Scene 1", "minimax_h3_prompt": "Brad walks."}));
    segment.extend(object(segment_extra));
    let session = object(json!({"project_name": "Demo", "revision": 1, "video_engine": "minimax_h3", "minimax_h3_settings": settings, "segments": [Value::Object(segment)]}));
    std::fs::write(dir.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
    LoadedSession { project_folder: dir.to_path_buf(), session_path: dir.join("vrgdg_builder_session.json"), revision: 1, session }
}

/// A two pass graph as the builder reports it, with the nodes a draft keeps.
fn two_pass_graph(model: &str) -> Obj {
    object(json!({"two_pass": {}, "prompt": {
        "119": {"class_type": "VAELoader", "inputs": {}},
        "124": {"class_type": "KSampler", "inputs": {"steps": 4}},
        "125": {"class_type": "Sampler", "inputs": {"latent": ["124", 0]}},
        "130": {"class_type": "Sampler", "inputs": {"latent": ["125", 0], "model": ["20", 0]}},
        "190": {"class_type": "Primitive", "inputs": {"value": 3}},
        "142": {"class_type": "VHS_VideoCombine", "inputs": {"images": ["130", 0], "filename_prefix": "scene"}},
        "10": {"class_type": "UNETLoader", "inputs": {"unet_name": model}},
        "20": {"class_type": "LoraLoaderModelOnly", "inputs": {"model": ["10", 0], "lora_name": "minimax_h3_ref2v_turbo_4step_v0.1_comfyui_bf16.safetensors"}},
    }}))
}

/// A draft saved the way a draft render leaves it, for scene 1 at 10.0 to 14.5 seconds.
fn saved_draft(dir: &Path, model: &str, restore: Value) -> String {
    let payload = object(json!({"project_folder": dir.display().to_string(), "scene_number": 1, "timeline_start_seconds": 10.0, "timeline_end_seconds": 14.5, "draft_restore_audio": restore}));
    let draft = h3_pipeline::drafts::build_draft(&two_pass_graph(model), &payload).unwrap();
    let id = draft["draft_id"].as_str().unwrap().to_string();
    std::fs::write(dir.join("minimax_drafts").join(&id).join("pass1.pt"), b"latent").unwrap();
    id
}

#[test]
fn a_draft_is_finished_from_its_saved_graph() {
    let dir = tempfile::tempdir().unwrap();
    let id = saved_draft(dir.path(), "big.gguf", json!({"restore_audio_path": "C:/song.wav", "restore_audio_start_seconds": 10.0}));
    let settings = json!({"video_mode": "reference_to_video", "render_pass": "two_pass", "continuity_mode": "off", "diffusion_model_name": "models/big.gguf"});
    let loaded = project(dir.path(), json!({"minimax_h3_draft": {"id": id, "video_path": "x.mp4"}}), settings.clone());
    // A newer Turbo LoRA release replaced the file the draft was made with: pass 2 uses the one that is installed.
    let env = Env(vec!["minimax_h3_ref2v_turbo_4step_v0.2_comfyui_bf16.safetensors".into()]);

    let (prepared, result) = prepare_finalize(&env, &loaded, "1").unwrap();
    assert_eq!((prepared.scene_number, prepared.scene_id.as_str(), prepared.workflow_key), (1, "a", "minimax_h3_2pass"));
    assert_eq!(result["prompt"]["125"]["class_type"], "VRGDG_MiniMaxH3LoadDraft", "pass 1 is the saved latent");
    assert_eq!(result["prompt"]["142"]["inputs"]["filename_prefix"], "scene_finalized");
    assert_eq!(result["prompt"]["20"]["inputs"]["lora_name"], "minimax_h3_ref2v_turbo_4step_v0.2_comfyui_bf16.safetensors");
    assert_eq!(prepared.restore_audio, Some((std::path::PathBuf::from("C:/song.wav"), 10.0)));
    assert_eq!(prepared.payload["continuity_mode"], "off");

    // The scene can be named by its number or its id.
    assert!(prepare_finalize(&env, &loaded, "a").is_ok());
    assert!(matches!(prepare_finalize(&env, &loaded, "9"), Err(RenderError::SceneNotFound(_))));

    // The draft was made with another model: the scene says so instead of replaying it.
    let other = project(dir.path(), json!({"minimax_h3_draft": {"id": id}}), json!({"render_pass": "two_pass", "diffusion_model_name": "other.gguf"}));
    let error = prepare_finalize(&env, &other, "1").unwrap_err().to_string();
    assert_eq!(error, "Scene 1's draft was made with big.gguf, but the scene now uses other.gguf. Render a new draft, or switch the diffusion model back to finalize this one.");

    // A scene that moved on the timeline needs a new draft.
    let moved = project(dir.path(), json!({"end": 15.0, "minimax_h3_draft": {"id": id}}), settings.clone());
    assert_eq!(prepare_finalize(&env, &moved, "1").unwrap_err().to_string(), "Scene 1: The scene timing changed. Render a new draft before finalizing.");

    // No draft, and a draft whose latent is gone.
    let none = project(dir.path(), json!({}), settings.clone());
    assert_eq!(prepare_finalize(&env, &none, "1").unwrap_err().to_string(), "Scene 1 has no draft. Render it with render_phase 'draft' first.");
    std::fs::remove_file(dir.path().join("minimax_drafts").join(&id).join("pass1.pt")).unwrap();
    assert_eq!(prepare_finalize(&env, &loaded, "1").unwrap_err().to_string(), "Scene 1: The saved pass-1 latent is missing. Render a new draft first.");
}

#[test]
fn only_a_two_pass_scene_can_be_a_draft() {
    let prepared = Prepared {
        scene_index: 0,
        scene_number: 1,
        scene_id: "a".into(),
        workflow_key: "minimax_h3",
        payload: object(json!({"start": 0.0, "end": 4.0})),
        restore_audio: None,
    };
    let error = build_draft_graph(&Env(vec![]), &prepared).unwrap_err().to_string();
    assert_eq!(error, "Draft rendering requires a MiniMax two-pass workflow (render_pass two_pass or three_pass).");
}

fn make_clip(path: &Path, seconds: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let source = format!("color=c=red:s=64x64:d={seconds}:r=24");
    let status = Command::new("ffmpeg").args(["-y", "-f", "lavfi", "-i", &source, "-pix_fmt", "yuv420p"]).arg(path).output().expect("ffmpeg is on the PATH").status;
    assert!(status.success());
}

fn outcome(dir: &Path, raw: &Path, phase: RenderPhase, graph: Value) -> RenderOutcome {
    RenderOutcome {
        scene_index: 0,
        scene_number: 1,
        scene_id: "a".into(),
        continuity_mode: "off".into(),
        prompt_id: "p1".into(),
        raw_video_path: raw.display().to_string(),
        output_folder: dir.display().to_string(),
        graph: object(graph),
        graph_nodes: Value::Null,
        restore_audio: None,
        phase,
    }
}

#[test]
fn a_finished_scene_keeps_its_draft_and_a_finished_draft_replaces_it() {
    let Ok(tools) = h3_media::Tools::find("ffmpeg", "ffprobe") else {
        eprintln!("ffmpeg is not installed: the finished scene is not tested");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let project_dir = dir.path().join("Demo");
    std::fs::create_dir_all(&project_dir).unwrap();
    let env = LiveEnv::new(json!({}), ComfyDirs { output: dir.path().display().to_string(), input: String::new(), temp: String::new() }, tools);
    project(&project_dir, json!({}), json!({"render_pass": "two_pass"}));
    let read = || -> Obj {
        let text = std::fs::read_to_string(project_dir.join("vrgdg_builder_session.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["segments"][0].as_object().cloned().unwrap()
    };

    // A draft is pass 1 only: its clip is the first stage, and the scene remembers the draft to finish later.
    let raw = dir.path().join("MiniMaxH3_scene_0001_draft_stage1.mp4");
    make_clip(&raw, 2);
    let done = finish_scene(&env, &project_dir, &outcome(dir.path(), &raw, RenderPhase::Draft, json!({"two_pass": {}, "draft_id": "d1"})), &FinishOptions::default()).unwrap();
    assert_eq!((done.render_phase.as_str(), done.draft_id.as_str()), ("draft", "d1"));
    let scene = read();
    assert_eq!(scene["minimax_h3_draft"], json!({"id": "d1", "video_path": done.video_path}));
    assert_eq!((scene["minimax_h3_stage1_path"].clone(), scene["minimax_h3_stage2_path"].clone()), (json!(done.video_path), json!("")));
    assert_eq!(done.backup_path, "", "the first clip replaces nothing");

    // A second full render keeps the draft's clip as a backup, and a scene rendered in two passes has a second stage.
    let raw2 = dir.path().join("MiniMaxH3_scene_0001_full.mp4");
    make_clip(&raw2, 2);
    let full = finish_scene(&env, &project_dir, &outcome(dir.path(), &raw2, RenderPhase::Full, json!({"two_pass": {}})), &FinishOptions::default()).unwrap();
    assert_eq!((full.render_phase.as_str(), full.draft_id.as_str()), ("full", ""));
    assert!(!full.backup_path.is_empty(), "a full render keeps the clip it replaces");
    let scene = read();
    assert_eq!(scene["minimax_h3_draft"], Value::Null);
    assert_eq!((scene["minimax_h3_stage1_path"].clone(), scene["minimax_h3_stage2_path"].clone()), (json!(""), json!(full.video_path)));

    // Finishing a draft overwrites the clip it came from.
    let raw3 = dir.path().join("MiniMaxH3_scene_0001_finalized.mp4");
    make_clip(&raw3, 2);
    let finished = finish_scene(&env, &project_dir, &outcome(dir.path(), &raw3, RenderPhase::Finalize, json!({"two_pass": {}})), &FinishOptions::default()).unwrap();
    assert_eq!(finished.render_phase, "finalize");
    assert_eq!(finished.backup_path, "", "the draft's clip is replaced, not kept");
}
