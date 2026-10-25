//! The Speaking routes of the agent API (scene audio settings, dialogue, ElevenLabs), the app-level ElevenLabs key, the project's video type and the
//! four prompt routes that run as jobs. No test calls ElevenLabs or an LLM: a request that would is stopped by a missing key or answered as a job.

use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct Server {
    base: String,
    client: Client,
    _dir: tempfile::TempDir,
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
    Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(60)).build().unwrap(), _dir: dir }
}

impl Server {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn ok(&self, method: Method, path: &str, body: Value) -> Value {
        let (code, response) = self.call(method, path, Some(body)).await;
        assert_eq!(code, 200, "{path}: {response}");
        response["data"].clone()
    }

    async fn fails(&self, method: Method, path: &str, body: Value, status: u16) -> String {
        let (code, response) = self.call(method, path, Some(body)).await;
        assert_eq!(code, status, "{path}: {response}");
        response["error"]["message"].as_str().unwrap_or("").to_string()
    }
}

#[tokio::test]
async fn speaking_routes_follow_the_projects_video_type_and_never_show_a_key() {
    let s = start().await;
    s.ok(Method::POST, "/projects", json!({"name": "Short Film"})).await;
    let p = "/projects/Short%20Film";

    // Outside Speaking mode the routes say so.
    let message = s.fails(Method::GET, &format!("{p}/elevenlabs"), json!(null), 400).await;
    assert!(message.contains("Speaking"), "{message}");

    // The video type is a project setting. A wrong value is refused.
    s.fails(Method::PATCH, &format!("{p}/settings"), json!({"project": {"video_type": "musical"}}), 400).await;
    let settings = s.ok(Method::PATCH, &format!("{p}/settings"), json!({"project": {"video_type": "speaking"}})).await;
    assert_eq!(settings["settings"]["project"]["video_type"], "speaking");

    // The project's key: saved, never returned, cleared with an empty string.
    let status = s.ok(Method::GET, &format!("{p}/elevenlabs"), json!(null)).await;
    assert_eq!((status["configured"].clone(), status["source"].clone()), (json!(false), json!("none")));
    s.fails(Method::PUT, &format!("{p}/elevenlabs"), json!({}), 400).await;
    let saved = s.ok(Method::PUT, &format!("{p}/elevenlabs"), json!({"api_key": "sk-project-secret"})).await;
    assert_eq!((saved["configured"].clone(), saved["source"].clone()), (json!(true), json!("project")));
    assert!(!saved.to_string().contains("sk-project-secret"));
    let cleared = s.ok(Method::PUT, &format!("{p}/elevenlabs"), json!({"api_key": ""})).await;
    assert_eq!(cleared["configured"], false);

    // The key for every project (App Settings) is used when the project has none.
    let global = s.ok(Method::GET, "/setup/elevenlabs", json!(null)).await;
    assert_eq!(global["configured"], false);
    let app = s.ok(Method::PUT, "/setup/elevenlabs", json!({"api_key": "sk-app-secret"})).await;
    assert_eq!(app["configured"], true);
    assert!(!app.to_string().contains("sk-app-secret"));
    let inherited = s.ok(Method::GET, &format!("{p}/elevenlabs"), json!(null)).await;
    assert_eq!((inherited["configured"].clone(), inherited["source"].clone()), (json!(true), json!("app")));
    let own = s.ok(Method::PUT, &format!("{p}/elevenlabs"), json!({"api_key": "sk-project-secret"})).await;
    assert_eq!(own["source"], "project");
    s.ok(Method::PUT, &format!("{p}/elevenlabs"), json!({"api_key": ""})).await;
    s.ok(Method::PUT, "/setup/elevenlabs", json!({"api_key": ""})).await;
    let gone = s.ok(Method::GET, &format!("{p}/elevenlabs"), json!(null)).await;
    assert_eq!(gone["configured"], false);
    // Without a key nothing is sent to ElevenLabs.
    let message = s.fails(Method::POST, "/setup/elevenlabs/test", json!({}), 400).await;
    assert!(message.contains("No ElevenLabs key"), "{message}");
    s.fails(Method::GET, &format!("{p}/elevenlabs/voices"), json!(null), 400).await;
    s.fails(Method::POST, &format!("{p}/elevenlabs/test"), json!({}), 400).await;
    s.fails(Method::POST, &format!("{p}/elevenlabs/voice-design/previews"), json!({"voice_description": "A warm, low voice with a slow, steady cadence."}), 400).await;

    // Project defaults for the silence around dialogue.
    let defaults = s.ok(Method::GET, &format!("{p}/speaking-audio/defaults"), json!(null)).await;
    assert_eq!((defaults["silence_before"].clone(), defaults["fit_duration"].clone()), (json!(0.0), json!(true)));
    let changed = s.ok(Method::PATCH, &format!("{p}/speaking-audio/defaults"), json!({"settings": {"silence_before": 1.5}})).await;
    assert_eq!(changed["silence_before"], 1.5);
    s.fails(Method::PATCH, &format!("{p}/speaking-audio/defaults"), json!({"settings": {"silence_before": 99}}), 400).await;

    // A scene's own audio settings.
    s.ok(Method::POST, &format!("{p}/timeline/bulk"), json!({"text": "4 first line\n4 second line", "mode": "durations"})).await;
    let (_, scenes) = s.call(Method::GET, &format!("{p}/scenes"), None).await;
    let scene = scenes["data"][0]["id"].as_str().unwrap().to_string();
    let view = s.ok(Method::GET, &format!("{p}/scenes/{scene}/audio-settings"), json!(null)).await;
    assert_eq!((view["scene_id"].clone(), view["scene_duration"].clone(), view["clips"].clone()), (json!(scene), json!(4.0), json!([])));
    assert_eq!(view["effective_settings"]["silence_before"], 1.5);
    let patched = s.ok(Method::PATCH, &format!("{p}/scenes/{scene}/audio-settings"), json!({"settings": {"use_project_defaults": false, "silence_after": 2}})).await;
    assert_eq!((patched["settings"]["use_project_defaults"].clone(), patched["effective_settings"]["silence_before"].clone()), (json!(false), json!(0.0)));
    // The scene's number on the timeline names it as well as its id.
    let by_number = s.ok(Method::GET, &format!("{p}/scenes/1/audio-settings"), json!(null)).await;
    assert_eq!(by_number["scene_id"], json!(scene));
    let by_number = s.ok(Method::PATCH, &format!("{p}/scenes/1/audio-settings"), json!({"settings": {"silence_after": 2}})).await;
    assert_eq!(by_number["scene_id"], json!(scene));
    s.fails(Method::GET, &format!("{p}/scenes/nope/audio-settings"), json!(null), 404).await;
    let message = s.fails(Method::PATCH, &format!("{p}/scenes/{scene}/audio-settings"), json!({"settings": {}, "dialogue": {"speaker_id": "nobody", "text": "Hello"}}), 400).await;
    assert!(message.contains("primary character"), "{message}");

    // Dialogue needs a character with an enabled voice.
    let message = s.fails(Method::POST, &format!("{p}/scenes/{scene}/dialogue/craft"), json!({}), 400).await;
    assert!(message.contains("Dialogue") || message.contains("character"), "{message}");
    s.ok(Method::PUT, &format!("{p}/references/subjects/subj1"), json!({"name": "Ava", "description": "a quiet pilot", "reference_type": "character", "elevenlabs_voice": {"enabled": true, "voice_id": "voice123", "name": "Aria"}})).await;
    let draft = json!({"dialogue": {"speaker_id": "subj1", "text": "We are almost there.", "delivery": "calm"}});
    s.ok(Method::PATCH, &format!("{p}/scenes/{scene}/audio-settings"), json!({"settings": {}, "dialogue": draft["dialogue"].clone()})).await;
    let (code, job) = s.call(Method::POST, &format!("{p}/scenes/{scene}/dialogue/craft"), Some(json!({}))).await;
    assert_eq!(code, 202, "{job}");
    assert!(job["data"]["job_id"].as_str().is_some());
    // No key: no speech is requested.
    let message = s.fails(Method::POST, &format!("{p}/scenes/{scene}/dialogue/generate"), json!({}), 400).await;
    assert!(message.to_lowercase().contains("key"), "{message}");

    // A voice description is a job for a primary character.
    let (code, job) = s.call(Method::POST, &format!("{p}/references/subjects/subj1/voice-design/description"), Some(json!({"user_input": "warm and low"}))).await;
    assert_eq!(code, 202, "{job}");
    s.fails(Method::POST, &format!("{p}/references/subjects/nobody/voice-design/description"), json!({"user_input": "warm and low"}), 400).await;
    s.fails(Method::POST, &format!("{p}/references/subjects/subj1/voice-design/description"), json!({"user_input": ""}), 400).await;

    // The four prompt routes answer as jobs, and name a scene that is not there.
    for path in [format!("{p}/prompts/concepts"), format!("{p}/prompts/motion-notes"), format!("{p}/scenes/{scene}/prompts/enhance"), format!("{p}/scenes/{scene}/prompts/edit")] {
        let (code, job) = s.call(Method::POST, &path, Some(json!({}))).await;
        assert_eq!(code, 202, "{path}: {job}");
    }
    s.fails(Method::POST, &format!("{p}/scenes/nope/prompts/enhance"), json!({}), 404).await;
    s.fails(Method::POST, &format!("{p}/scenes/nope/prompts/edit"), json!({}), 404).await;
}

#[tokio::test]
async fn instructions_and_their_presets_are_read_changed_and_reset_through_the_api() {
    let s = start().await;
    s.ok(Method::POST, "/projects", json!({"name": "Film"})).await;
    let key = "minimax_h3_prompt_enhance";
    let before = s.ok(Method::GET, &format!("/instructions/{key}?project_id=Film"), json!(null)).await;
    let default_text = before["text"].as_str().unwrap().to_string();
    assert!(!default_text.is_empty(), "{before}");

    let saved = s.ok(Method::PUT, &format!("/instructions/{key}"), json!({"project_id": "Film", "scope": "all", "text": "Keep every shot, tighten the wording."})).await;
    assert_eq!(saved["text"], "Keep every shot, tighten the wording.", "{saved}");
    s.fails(Method::PUT, &format!("/instructions/{key}"), json!({"project_id": "Film", "scope": "all", "text": "  "}), 400).await;
    let reset = s.ok(Method::DELETE, &format!("/instructions/{key}/override"), json!({"project_id": "Film", "scope": "all"})).await;
    assert_eq!(reset["text"], default_text.as_str());

    let preset = s.ok(Method::PUT, "/instructions/presets/Tight", json!({"key": key, "text": "Tighten the wording."})).await;
    assert_eq!(preset["name"], "Tight");
    let listed = s.ok(Method::GET, &format!("/instructions?key={key}"), json!(null)).await;
    assert_eq!(listed["presets"][0]["name"], "Tight", "{listed}");
    let loaded = s.ok(Method::POST, "/instructions/presets/Tight/load", json!({"key": key})).await;
    assert_eq!(loaded["text"], "Tighten the wording.");
    s.fails(Method::POST, "/instructions/presets/Missing/load", json!({"key": key}), 400).await;
    s.fails(Method::GET, &format!("/instructions/{key}?project_id=Nobody"), json!(null), 404).await;
}
