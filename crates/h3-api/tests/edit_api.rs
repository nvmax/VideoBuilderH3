//! Building a project through the API: create, audio, timeline, scene fields, prompts, lyrics, references and files.

use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct Server {
    base: String,
    client: Client,
    dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        self.call(Method::GET, path, &[], None).await
    }

    async fn ok(&self, method: Method, path: &str, body: Value) -> Value {
        let (code, response) = self.call(method, path, &[], Some(body)).await;
        assert_eq!(code, 200, "{path}: {response}");
        response["data"].clone()
    }
}

/// A 16 kHz mono WAV with a click every half second.
fn click_track(seconds: u32) -> Vec<u8> {
    let rate = 16_000u32;
    let mut samples: Vec<i16> = Vec::new();
    for n in 0..rate * seconds {
        let phase = (n % (rate / 2)) as f64 / f64::from(rate);
        samples.push((20_000.0 * (-phase * 14.0).exp() * (2.0 * std::f64::consts::PI * 220.0 * n as f64 / f64::from(rate)).sin()) as i16);
    }
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[tokio::test]
async fn a_project_is_built_edited_and_deleted_through_the_api() {
    let s = start().await;
    let created = s.ok(Method::POST, "/projects", json!({"name": "Demo Song"})).await;
    assert_eq!(created["project_id"], "Demo Song");
    let (_, list) = s.get("/projects").await;
    assert_eq!(list["data"][0]["id"], "Demo Song");
    let (code, bad) = s.call(Method::POST, "/projects", &[], Some(json!({"name": " "}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));

    let p = "/projects/Demo%20Song";
    // A timeline from text, with words on the lines.
    let bulk = s.ok(Method::POST, &format!("{p}/timeline/bulk"), json!({"text": "4 first line\n4 second line\n4", "mode": "durations"})).await;
    assert_eq!((bulk["scene_count"].as_i64(), bulk["scenes_with_lyrics"].as_i64()), (Some(3), Some(2)));
    let (_, scenes) = s.get(&format!("{p}/scenes")).await;
    let ids: Vec<String> = scenes["data"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.iter().all(|i| i.starts_with("seg_") && i.len() == 16), "{ids:?}");
    assert_eq!(scenes["data"][0]["lyrics"], "first line");
    assert_eq!(scenes["data"][2]["end"], 12.0);

    // Edit the timeline.
    let split = s.ok(Method::POST, &format!("{p}/scenes/1/split"), json!({"at_time": 2.0})).await;
    assert_eq!(split["left_scene"]["end"], 2.0);
    s.ok(Method::POST, &format!("{p}/scenes/2/resize"), json!({"duration": 3.0})).await;
    s.ok(Method::POST, &format!("{p}/scenes/1/merge"), json!({"with_direction": "next"})).await;
    let added = s.ok(Method::POST, &format!("{p}/scenes"), json!({"position": "after", "ref_scene_id": "1", "duration": 2.0, "label": "Bridge"})).await;
    assert_eq!(added["scene"]["label"], "Bridge");
    let (code, bad) = s.call(Method::POST, &format!("{p}/scenes/1/split"), &[], Some(json!({"at_time": 99.0}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (code, bad) = s.call(Method::DELETE, &format!("{p}/scenes/1"), &[("If-Match", "1")], None).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (409, Some("REVISION_CONFLICT")));
    let deleted = s.ok(Method::DELETE, &format!("{p}/scenes/{}?ripple=true", added["scene"]["id"].as_str().unwrap()), json!(null)).await;
    assert_eq!(deleted["deleted_scene_id"], added["scene"]["id"]);
    let (_, check) = s.call(Method::POST, &format!("{p}/validate"), &[], None).await;
    assert_eq!(check["data"]["valid"], true, "{check}");

    // Scene fields, prompts and lyrics.
    let patched = s.ok(Method::PATCH, &format!("{p}/scenes/1"), json!({"label": "Opening", "story_beat": "A walk."})).await;
    assert_eq!(patched["scene"]["story_beat"], "A walk.");
    let put = s.ok(Method::PUT, &format!("{p}/scenes/1/prompts/minimax_h3_prompt"), json!({"prompt": "A singer on a roof.", "origin": "manual"})).await;
    assert_eq!(put["origin"], "manual");
    let (_, v) = s.call(Method::POST, &format!("{p}/scenes/1/prompts/minimax/validate"), &[], Some(json!({"prompt": "x".repeat(7001)}))).await;
    assert_eq!(v["data"]["errors"][0]["code"], "MINIMAX_H3_PROMPT_TOO_LONG");
    s.ok(Method::PUT, &format!("{p}/lyrics"), json!({"lyrics_text": "one\ntwo"})).await;
    let (_, lyrics) = s.get(&format!("{p}/lyrics")).await;
    assert_eq!(lyrics["data"]["lyrics_text"], "one\ntwo");

    // A character, mapped to scene 1.
    s.ok(Method::PUT, &format!("{p}/references/subjects/subj1"), json!({"name": "Ava", "description": "a singer"})).await;
    s.ok(Method::PUT, &format!("{p}/references/scene-mapping"), json!({"subjects": {"1": ["subj1"]}})).await;
    let (_, mapping) = s.get(&format!("{p}/references/scene-mapping")).await;
    assert_eq!(mapping["data"]["scene_mapping"]["subjects"]["1"], json!(["subj1"]));
    // Assign Scenes: a dry run saves nothing, the real run fills both maps.
    s.ok(Method::PUT, &format!("{p}/references/locations/loc1"), json!({"name": "Roof"})).await;
    let rotate = json!({"character_pattern": "rotate", "location_pattern": "rotate", "replace_existing": true});
    let preview = s.ok(Method::POST, &format!("{p}/references/assign-scenes"), json!({"character_pattern": "rotate", "location_pattern": "rotate", "replace_existing": true, "dry_run": true})).await;
    assert!(preview["dry_run"].as_bool().unwrap() && preview["revision"].is_null() && preview["scenes_assigned"].as_u64().unwrap() > 0);
    let before = s.get(&format!("{p}/references/scene-mapping")).await.1["data"]["scene_mapping"]["locations"].clone();
    assert_eq!(before, json!({}));
    let done = s.ok(Method::POST, &format!("{p}/references/assign-scenes"), rotate).await;
    assert!(done["revision"].as_i64().is_some() && done["location_use"]["Roof"].as_u64().unwrap() > 0);
    let after = s.get(&format!("{p}/references/scene-mapping")).await.1["data"]["scene_mapping"]["locations"].clone();
    assert_eq!(after.as_object().unwrap().len() as u64, done["scenes_assigned"].as_u64().unwrap());
    let (code, _) = s.call(Method::POST, &format!("{p}/references/assign-scenes"), &[], Some(json!({"character_pattern": "shuffle"}))).await;
    assert_eq!(code, 400);
    // Timeline notes: added, listed with the scenes they overlap, changed, deleted.
    let created = s.call(Method::POST, &format!("{p}/timeline/notes"), &[], Some(json!({"id": "n1", "start": 0.5, "end": 3.0, "label": "Intro"}))).await;
    assert_eq!(created.0, 201, "{}", created.1);
    let (code, bad) = s.call(Method::POST, &format!("{p}/timeline/notes"), &[], Some(json!({"label": "no start"}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));
    let (_, listed) = s.get(&format!("{p}/timeline/notes")).await;
    assert_eq!((listed["data"]["count"].as_i64(), listed["data"]["notes"][0]["label"].as_str()), (Some(1), Some("Intro")));
    assert!(listed["data"]["notes"][0]["scene_ids"].as_array().is_some_and(|a| !a.is_empty()));
    let (code, changed) = s.call(Method::PATCH, &format!("{p}/timeline/notes/n1"), &[], Some(json!({"end": null, "label": "Hook"}))).await;
    assert_eq!((code, changed["data"]["note"]["end"].is_null(), changed["data"]["note"]["label"].as_str()), (200, true, Some("Hook")));
    let (code, _) = s.call(Method::PATCH, &format!("{p}/timeline/notes/nope"), &[], Some(json!({"label": "x"}))).await;
    assert_eq!(code, 404);
    let (code, gone) = s.call(Method::DELETE, &format!("{p}/timeline/notes/n1"), &[], None).await;
    assert_eq!((code, gone["data"]["deleted"]["id"].as_str(), gone["data"]["notes"].as_array().map(Vec::len)), (200, Some("n1"), Some(0)));

    // Enforce scene lengths: the dry run says what would change and saves nothing, the real run makes the same edits.
    let (code, _) = s.call(Method::POST, &format!("{p}/timeline/enforce-length"), &[], Some(json!({"min_scene_seconds": 5}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, &format!("{p}/timeline/enforce-length"), &[], Some(json!({"min_scene_seconds": 5, "max_scene_seconds": 2}))).await;
    assert_eq!(code, 400);
    let limits = json!({"min_scene_seconds": 0.5, "max_scene_seconds": 1.0});
    let (_, before) = s.get(&format!("{p}/scenes")).await;
    let scenes_before = before["data"].as_array().map_or(0, Vec::len);
    let dry = s.ok(Method::POST, &format!("{p}/timeline/enforce-length"), json!({"min_scene_seconds": 0.5, "max_scene_seconds": 1.0, "dry_run": true})).await;
    assert_eq!((dry["dry_run"].as_bool(), dry["scenes_before"].as_u64().map(|n| n as usize)), (Some(true), Some(scenes_before)));
    let (_, after_dry) = s.get(&format!("{p}/scenes")).await;
    assert_eq!(after_dry["data"].as_array().map_or(0, Vec::len), scenes_before, "a dry run saves nothing");
    let real = s.ok(Method::POST, &format!("{p}/timeline/enforce-length"), limits).await;
    assert_eq!((real["scenes_after"].clone(), real["splits"].clone(), real["merges"].clone()), (dry["scenes_after"].clone(), dry["splits"].clone(), dry["merges"].clone()));
    let (_, after) = s.get(&format!("{p}/scenes")).await;
    assert_eq!(after["data"].as_array().map_or(0, Vec::len), real["scenes_after"].as_u64().unwrap() as usize);

    // The whole story layer can be read and replaced, and the settings are checked against the engine.
    let (code, story) = s.get(&format!("{p}/story")).await;
    assert_eq!(code, 200, "{story}");
    let (code, put) = s.call(Method::PUT, &format!("{p}/story"), &[], Some(json!({"overall_story_idea": "A fox", "song_story_brief": "Brief"}))).await;
    assert_eq!((code, put["data"]["song_story_brief"].as_str()), (200, Some("Brief")));
    let (_, again) = s.get(&format!("{p}/story")).await;
    assert_eq!(again["data"]["overall_story_idea"], "A fox");
    let (code, check) = s.call(Method::POST, &format!("{p}/settings/preflight"), &[], None).await;
    assert_eq!(code, 200, "{check}");
    assert!(check["data"]["valid"].is_boolean() && check["data"]["effective_settings"].is_object());

    // A silent track as the project's audio, and a copy of the whole project.
    let silent = s.ok(Method::POST, &format!("{p}/audio/silent"), json!({"duration": 2.5})).await;
    assert!((silent["duration"].as_f64().unwrap() - 2.5).abs() < 0.05, "{silent}");
    assert!(silent["audio_path"].as_str().unwrap().ends_with("project_silence_2_5s.wav"), "{silent}");
    let (code, _) = s.call(Method::POST, &format!("{p}/audio/silent"), &[], Some(json!({"duration": 0}))).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::POST, &format!("{p}/audio/silent"), &[], Some(json!({"duration": 3, "scope": "scene"}))).await;
    assert_eq!(code, 400);
    let (code, copy) = s.call(Method::POST, &format!("{p}/duplicate"), &[], Some(json!({"new_name": "Copy Of It"}))).await;
    assert_eq!(code, 201, "{copy}");
    let copy_id = copy["data"]["project_id"].as_str().unwrap().replace(' ', "%20");
    let (code, copied) = s.get(&format!("/projects/{copy_id}")).await;
    assert_eq!(code, 200, "{copied}");
    let copied_audio = copied["data"]["audio_path"].as_str().or_else(|| copied["data"]["session"]["audio_path"].as_str()).unwrap_or("");
    assert!(copied_audio.is_empty() || copied_audio.contains("Copy Of It"), "the copy points at its own audio: {copied_audio}");
    let (code, _) = s.call(Method::POST, &format!("{p}/duplicate"), &[], Some(json!({"new_name": " "}))).await;
    assert_eq!(code, 400);

    // Story settings are saved, a bad value is refused, and the Storyboard-only steps say so.
    let saved = s.ok(Method::PUT, &format!("{p}/story/settings"), json!({"defaults": {"camera_motion_speed": 6}, "story": {"overall_story_idea": "A fox"}})).await;
    assert_eq!((saved["defaults"]["camera_motion_speed"].as_f64(), saved["story"]["overall_story_idea"].as_str()), (Some(6.0), Some("A fox")));
    let (code, _) = s.call(Method::PUT, &format!("{p}/story/settings"), &[], Some(json!({"defaults": {"camera_motion_speed": 40}}))).await;
    assert_eq!(code, 400);
    let (code, body) = s.call(Method::POST, &format!("{p}/story/arc"), &[], Some(json!({}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (501, Some("NOT_SUPPORTED")));
    s.ok(Method::DELETE, &format!("{p}/references/subjects/subj1"), json!(null)).await;

    // Deleting needs the id typed back.
    let (code, _) = s.call(Method::DELETE, p, &[], None).await;
    assert_eq!(code, 400);
    let (code, _) = s.call(Method::DELETE, &format!("{p}?confirm=Demo%20Song"), &[], None).await;
    assert_eq!(code, 200);
    assert!(!s.dir.path().join("projects/Demo Song").exists());
}

#[tokio::test]
async fn audio_is_attached_with_peaks_and_beats_and_files_are_served_in_ranges() {
    if h3_media::Tools::find("ffmpeg", "ffprobe").is_err() {
        eprintln!("skipped: ffmpeg is not on PATH");
        return;
    }
    let s = start().await;
    s.ok(Method::POST, "/projects", json!({"name": "Songy"})).await;
    let wav = s.dir.path().join("song.wav");
    std::fs::write(&wav, click_track(6)).unwrap();
    let p = "/projects/Songy";
    let attached = s.ok(Method::PUT, &format!("{p}/audio"), json!({"audio_path": wav.display().to_string()})).await;
    assert!((attached["duration"].as_f64().unwrap() - 6.0).abs() < 0.01, "{attached}");
    assert_eq!(attached["peaks_count"], 1600);
    assert!(attached["beat_count"].as_i64().unwrap() >= 8, "{attached}");
    let (_, wave) = s.get(&format!("{p}/audio/waveform?peaks=100")).await;
    assert_eq!(wave["data"]["peaks"].as_array().unwrap().len(), 100);
    let (_, beats) = s.get(&format!("{p}/audio/beats")).await;
    assert!(beats["data"]["tempo_bpm"].as_f64().unwrap() > 100.0, "{beats}");
    s.ok(Method::POST, &format!("{p}/timeline/calibrate"), json!({"offset_seconds": 0.1})).await;
    let (_, detail) = s.get(p).await;
    assert_eq!(detail["data"]["audio"]["attached"], true);
    let (code, bad) = s.call(Method::PUT, &format!("{p}/audio"), &[], Some(json!({"audio_path": "nowhere.wav"}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")));

    // The saved song is served with range support, and a file outside the project folders is refused.
    let saved = detail["data"]["audio"]["path"].as_str().unwrap().to_string();
    let url = |path: &str| format!("{}/files?path={}", s.base, path.replace('\\', "%5C").replace(' ', "%20"));
    let full = s.client.get(url(&saved)).send().await.unwrap();
    assert_eq!(full.status(), 200);
    let part = s.client.get(url(&saved)).header("Range", "bytes=0-99").send().await.unwrap();
    assert_eq!(part.status(), 206);
    assert_eq!(part.bytes().await.unwrap().len(), 100);
    let outside = s.client.get(url(&wav.display().to_string())).send().await.unwrap();
    assert_eq!(outside.status(), 403);
}
