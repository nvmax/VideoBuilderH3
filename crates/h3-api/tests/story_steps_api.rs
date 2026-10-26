//! The story steps through the API: the arc, brief, beats and Story Mode steps run as jobs on the app's LLM, and Story Mode fields can be written by hand.

use std::time::Duration;

use axum::routing::post;
use axum::{Json, Router};
use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

const BEAT: &str = "Ava walks along Main Street beside the warm shop windows with her collar turned up and her reflection sliding across the glass as she passes each door on the wet road.";

struct Server {
    base: String,
    client: Client,
    dir: tempfile::TempDir,
}

fn session() -> Value {
    json!({
        "project_name": "Demo",
        "revision": 1,
        "video_engine": "minimax_h3",
        "segments": [
            {"id": "a", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "walking home alone"},
            {"id": "b", "start": 4.0, "end": 8.0, "label": "Scene 2", "lyric_text": "the street is empty"}
        ],
        "builder_story_layer": {"overall_story_idea": "A woman walks home through a quiet town.", "user_story_arc": "She walks home."},
        "flux_reference_builder": {
            "subjects": [{"id": "ava", "name": "Ava", "description": "dark hair", "image": {"path": "C:\\refs\\ava.png"}}],
            "locations": [{"id": "street", "name": "Main Street", "description": "wet asphalt and warm windows"}],
            "subject_scene_map": {"a": ["ava"], "b": ["ava"]},
            "scene_map": {"a": "street", "b": "street"},
            "use_subject_reference": true
        }
    })
}

/// A server whose LLM answers every question with one beat. With `llm_up` false the app's LLM is not there.
async fn start(llm_up: bool) -> Server {
    let llm = Router::new()
        .route("/v1/models", axum::routing::get(|| async { Json(json!({"data": [{"id": "test-model"}]})) }))
        .route("/v1/chat/completions", post(|| async { Json(json!({"choices": [{"message": {"content": BEAT}}]})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, llm).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    let project = root.join("Demo");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session()).unwrap()).unwrap();
    let mut cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    cfg.llm.lm_studio_base_url = "http://127.0.0.1:9/v1".into();
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    let server = Server { base: format!("http://{addr}/api/v1"), client: Client::builder().timeout(Duration::from_secs(30)).build().unwrap(), dir };
    if llm_up {
        let (code, body) = server.call(Method::PUT, "/llm/settings", Some(json!({"runner": "own_server", "own_server_url": mock, "own_server_model": "m"}))).await;
        assert_eq!(code, 200, "{body}");
    }
    server
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

    async fn job(&self, path: &str, body: Value) -> Value {
        let (code, started) = self.call(Method::POST, path, Some(body)).await;
        assert_eq!(code, 202, "{path}: {started}");
        let id = started["data"]["job_id"].as_str().unwrap().to_string();
        for _ in 0..200 {
            let (_, body) = self.call(Method::GET, &format!("/jobs/{id}"), None).await;
            if ["succeeded", "failed", "cancelled"].contains(&body["data"]["status"].as_str().unwrap_or("")) {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {id} did not finish");
    }

    fn saved(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.dir.path().join("projects/Demo/vrgdg_builder_session.json")).unwrap()).unwrap()
    }
}

#[tokio::test]
async fn scene_beats_run_as_a_job_on_the_apps_llm_and_are_saved_on_the_scenes() {
    let s = start(true).await;
    let job = s.job("/projects/Demo/story/beats", json!({"scene_ids": ["2", "a"]})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["created"], 2, "{job}");
    let saved = s.saved();
    let beat = |id: &str| saved["segments"].as_array().unwrap().iter().find(|seg| seg["id"] == id).unwrap()["story_beat"].as_str().unwrap_or("").to_string();
    assert!(beat("a").starts_with("Ava walks along Main Street") && beat("b").starts_with("Ava walks along Main Street"), "{saved}");
    // A step that does not exist is refused before any job is made, and so is a project that does not exist.
    let (code, body) = s.call(Method::POST, "/projects/Demo/story/dance", Some(json!({}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "{body}");
    let (code, body) = s.call(Method::POST, "/projects/Nope/story/beats", Some(json!({}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (404, Some("PROJECT_NOT_FOUND")), "{body}");
}

#[tokio::test]
async fn a_story_step_that_cannot_run_fails_the_job_and_saves_nothing() {
    let s = start(false).await;
    let before = s.saved();
    // The LLM is not there.
    let down = s.job("/projects/Demo/story/beats", json!({})).await;
    assert_eq!(down["status"], "failed", "{down}");
    assert!(down["error"]["message"].as_str().is_some_and(|m| !m.is_empty()), "{down}");
    // What the step needs is missing.
    let missing = s.job("/projects/Demo/story/chapter", json!({})).await;
    assert_eq!((missing["status"].as_str(), missing["error"]["code"].as_str()), (Some("failed"), Some("VALIDATION_ERROR")), "{missing}");
    assert!(missing["error"]["message"].as_str().unwrap().contains("chapter_id is required"), "{missing}");
    let no_scene_plan = s.job("/projects/Demo/story/book", json!({})).await;
    assert_eq!(no_scene_plan["status"], "failed", "{no_scene_plan}");
    assert_eq!(s.saved(), before);
}

#[tokio::test]
async fn story_mode_fields_are_written_by_hand_and_checked() {
    let s = start(true).await;
    let book = "Title: The Walk Home\n\nAct 1: The Walk\nSetting: Main Street\n\nAva walks down Main Street.\n\nThe last light goes out.";
    let (code, saved) = s.call(Method::PUT, "/projects/Demo/story/settings", Some(json!({"story": {"story_book_text": book}}))).await;
    assert_eq!(code, 200, "{saved}");
    let layer = &saved["data"]["story"];
    assert_eq!((layer["story_flow_version"].as_i64(), layer["story_book"]["chapters"].as_array().map(Vec::len)), (Some(2), Some(1)), "{layer}");
    assert_eq!(layer["story_settings"][0]["name"], "Main Street");

    // Acts that leave a scene out are refused and say why. Acts that cover the timeline are saved.
    let act = |scenes: Value| json!({"id": "act_1", "chapter_id": "ch_1", "title": "The Walk", "summary": "She walks.", "cast_ids": ["ava"], "scene_ids": scenes, "setting_id": "setting_main_street"});
    let (code, bad) = s.call(Method::PUT, "/projects/Demo/story/settings", Some(json!({"story": {"story_acts": [act(json!(["a"]))]}}))).await;
    assert_eq!((code, bad["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "{bad}");
    assert!(bad["error"]["message"].as_str().unwrap().contains("story_acts are not valid"), "{bad}");
    let (code, good) = s.call(Method::PUT, "/projects/Demo/story/settings", Some(json!({"story": {"story_acts": [act(json!(["a", "b"]))]}}))).await;
    assert_eq!(code, 200, "{good}");
    assert_eq!(good["data"]["story"]["story_acts"][0]["scene_ids"], json!(["a", "b"]));

    // A scene plan sets each scene's performance on the timeline and the continuation of scene 2 as a masked continuation.
    let plan = json!([
        {"scene_id": "a", "act_id": "act_1", "story_slice": "She leaves.", "spot": "Main Street", "mood": "calm", "performance_mode": "singing", "cast_ids": ["ava"]},
        {"scene_id": "b", "act_id": "act_1", "story_slice": "She walks on.", "spot": "Main Street", "mood": "calm", "performance_mode": "no_lip_sync", "cast_ids": ["ava"], "continues_previous": true, "continuation_source": "manual", "continuation_direction": "Ava keeps walking."}
    ]);
    let (code, planned) = s.call(Method::PUT, "/projects/Demo/story/settings", Some(json!({"story": {"story_scene_plan": plan, "story_continuation_mode": "location"}}))).await;
    assert_eq!(code, 200, "{planned}");
    let saved = s.saved();
    let scene = |id: &str| saved["segments"].as_array().unwrap().iter().find(|seg| seg["id"] == id).unwrap().clone();
    assert_eq!((scene("a")["performance_mode"].as_str(), scene("a")["lyric_no_lip_sync"].as_bool()), (Some("singing"), Some(false)), "{saved}");
    assert_eq!((scene("b")["performance_mode"].as_str(), scene("b")["lyric_no_lip_sync"].as_bool()), (Some("no_lip_sync"), Some(true)), "{saved}");
    assert_eq!(scene("b")["minimax_h3_settings"]["continuity_mode"], "latent_continuation_masked", "{}", scene("b"));
    assert_eq!(scene("b")["minimax_h3_continuation_direction"], "Ava keeps walking.");
    assert!(scene("a").get("minimax_h3_settings").is_none(), "the first scene never continues");

    // A field that is not a story field is still refused.
    let (code, nope) = s.call(Method::PUT, "/projects/Demo/story/settings", Some(json!({"story": {"story_flavour": "x"}}))).await;
    assert_eq!(code, 400, "{nope}");
}

#[tokio::test]
async fn the_prompt_writers_run_on_the_apps_llm_without_runner_settings_in_the_request() {
    // The request names no runner, as an agent's does. The writer must use the LLM App Settings choose (the mock), not the Gemma in ComfyUI, which is down.
    let s = start(true).await;
    let (code, started) = s
        .call(Method::POST, "/projects/Demo/scenes/1/prompts/edit", Some(json!({"edit_request": "Make it night.", "current_prompt": "A woman walks along a street at noon.", "save": false})))
        .await;
    assert_eq!(code, 202, "{started}");
    let id = started["data"]["job_id"].as_str().unwrap().to_string();
    let mut job = Value::Null;
    for _ in 0..200 {
        let (_, body) = s.call(Method::GET, &format!("/jobs/{id}"), None).await;
        if ["succeeded", "failed", "cancelled"].contains(&body["data"]["status"].as_str().unwrap_or("")) {
            job = body["data"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(job["status"], "succeeded", "{job}");
    assert!(job["result"]["prompt"].as_str().is_some_and(|p| p.contains("Main Street")), "{job}");
}

#[tokio::test]
async fn a_story_written_by_hand_is_mapped_and_linked_without_the_llm() {
    // The LLM is down: nothing here may ask it.
    let s = start(false).await;
    let path = s.dir.path().join("projects/Demo/vrgdg_builder_session.json");
    let mut saved = s.saved();
    saved["flux_reference_builder"]["subject_scene_map"] = json!({});
    saved["flux_reference_builder"]["scene_map"] = json!({});
    saved["flux_reference_builder"]["locations"] = json!([]);
    std::fs::write(&path, saved.to_string()).unwrap();

    let book = "Title: The Walk Home\n\nAct 1: The Walk\nSetting: Main Street\n\nAva walks down Main Street.\n\nThe last light goes out.";
    let (code, put) = s.call(Method::PUT, "/projects/Demo/story/settings", Some(json!({"story": {"story_book_text": book}}))).await;
    assert_eq!(code, 200, "{put}");
    // Places by hand: a card named "Setting - Spot" for each spot the plan will use.
    for (id, name) in [("corner", "Main Street - Corner"), ("door", "Main Street - Door")] {
        let (code, card) = s.call(Method::PUT, &format!("/projects/Demo/references/locations/{id}"), Some(json!({"name": name, "description": format!("{name} card.")}))).await;
        assert_eq!(code, 200, "{card}");
    }
    let acts = json!([{"id": "act_1", "chapter_id": "ch_1", "title": "The Walk", "cast_ids": ["ava"], "scene_ids": ["a", "b"]}]);
    let entry = |scene: &str, spot: &str| json!({"scene_id": scene, "act_id": "act_1", "story_slice": "She walks.", "spot": spot, "mood": "calm", "performance_mode": "singing", "cast_ids": ["ava"]});
    let plan = json!([entry("a", "Corner"), entry("b", "Door")]);
    let (code, put) = s.call(Method::PUT, "/projects/Demo/story/settings", Some(json!({"story": {"story_acts": acts, "story_scene_plan": plan}}))).await;
    assert_eq!(code, 200, "{put}");
    let refs = &s.saved()["flux_reference_builder"];
    assert_eq!(refs["subject_scene_map"], json!({"a": ["ava"], "b": ["ava"]}), "saving the plan maps each planned scene's cast: {refs}");

    let job = s.job("/projects/Demo/story/locations", json!({"map_only": true})).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["result"]["added"], 0, "{job}");
    assert_eq!(job["result"]["scene_map"], json!({"a": "Main Street - Corner", "b": "Main Street - Door"}), "{job}");
    let saved = s.saved();
    let cards = saved["flux_reference_builder"]["locations"].as_array().unwrap();
    assert_eq!(cards.len(), 2, "no card is made");
    assert!(cards.iter().all(|c| c["story_setting_id"].as_str().is_some_and(|id| !id.is_empty())), "the cards are linked to their chapter setting: {cards:?}");
    assert_eq!(saved["flux_reference_builder"]["scene_map"]["a"], "corner");
}
