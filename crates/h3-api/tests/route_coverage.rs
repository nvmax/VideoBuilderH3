//! Every endpoint of the pack's agent API is either served by this app or named here with the reason it is left out.
//! `fixtures/api/pack_endpoints.json` comes from the pack's own endpoint list.

use std::path::Path;
use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::Value;

/// Endpoints this app does not serve, with why.
const LEFT_OUT: &[(&str, &str, &str)] = &[
    // Speaking videos and voices are not music video features.
    ("GET", "/projects/{pid}/speaking-audio/defaults", "speaking"),
    ("PATCH", "/projects/{pid}/speaking-audio/defaults", "speaking"),
    ("GET", "/projects/{pid}/scenes/{sid}/audio-settings", "speaking"),
    ("PATCH", "/projects/{pid}/scenes/{sid}/audio-settings", "speaking"),
    ("POST", "/projects/{pid}/scenes/{sid}/dialogue/craft", "speaking"),
    ("POST", "/projects/{pid}/scenes/{sid}/dialogue/generate", "speaking"),
    ("GET", "/projects/{pid}/elevenlabs", "elevenlabs"),
    ("PUT", "/projects/{pid}/elevenlabs", "elevenlabs"),
    ("GET", "/projects/{pid}/elevenlabs/voices", "elevenlabs"),
    ("POST", "/projects/{pid}/elevenlabs/test", "elevenlabs"),
    ("POST", "/projects/{pid}/references/subjects/{rid}/voice-design/description", "elevenlabs"),
    ("POST", "/projects/{pid}/elevenlabs/voice-design/previews", "elevenlabs"),
    ("POST", "/projects/{pid}/elevenlabs/voice-design/create", "elevenlabs"),
    // The app never loads, unloads or switches the LLM model.
    ("POST", "/llm/unload", "model policy"),
    // LTX only.
    ("POST", "/projects/{project_id}/pipelines/build-flf", "ltx"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/face-fix/estimate", "face fix (ltx)"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/face-fix/prepare", "face fix (ltx)"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/face-fix/anchors/{n}/enhance", "face fix (ltx)"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/face-fix/runs/{n}/ltx", "face fix (ltx)"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/face-fix/finalize", "face fix (ltx)"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/face-fix/auto", "face fix (ltx)"),
    // Planned: still to port.
    ("POST", "/projects/{pid}/prompts/concepts", "llm prompt routes"),
    ("POST", "/projects/{pid}/prompts/motion-notes", "llm prompt routes"),
    ("POST", "/projects/{pid}/scenes/{sid}/prompts/image", "llm prompt routes"),
    ("POST", "/projects/{pid}/scenes/{sid}/prompts/video", "llm prompt routes"),
    ("POST", "/projects/{pid}/scenes/{sid}/prompts/video-chained", "llm prompt routes"),
    ("POST", "/projects/{pid}/scenes/{sid}/prompts/enhance", "llm prompt routes"),
    ("POST", "/projects/{pid}/scenes/{sid}/prompts/edit", "llm prompt routes"),
    ("POST", "/projects/{pid}/prompts/batch", "llm prompt routes"),
    ("GET", "/instructions", "instruction presets"),
    ("GET", "/instructions/{key}", "instruction presets"),
    ("PUT", "/instructions/{key}", "instruction presets"),
    ("DELETE", "/instructions/{key}/override", "instruction presets"),
    ("PUT", "/instructions/presets/{name}", "instruction presets"),
    ("POST", "/instructions/presets/{name}/load", "instruction presets"),
    ("DELETE", "/post/luts/previews/{id}", "post previews"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/post/lut/preview", "post previews"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/post/film-grain", "film grain"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/post/film-grain/preview", "film grain"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/post/adjust", "adjust"),
    ("POST", "/projects/{project_id}/scenes/{scene_id}/post/adjust/preview", "adjust"),
    ("GET", "/post/adjust/presets", "adjust"),
    ("PUT", "/post/adjust/presets/{name}", "adjust"),
];

fn left_out(method: &str, path: &str) -> bool {
    LEFT_OUT.iter().any(|(m, p, _)| *m == method && *p == path)
}

async fn start() -> (String, Client, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let cfg = AppConfig { comfy_url: "http://127.0.0.1:9".into(), project_root: root, ..AppConfig::default() };
    let state = h3_api::build_state(cfg, &dir.path().join("jobs.db")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, h3_api::router(state)).await.unwrap() });
    (format!("http://{addr}/api/v1"), Client::builder().timeout(Duration::from_secs(20)).build().unwrap(), dir)
}

/// Fill `{name}` placeholders with a name that is not a project, scene or job.
fn concrete(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map_or(rest.len(), |c| open + c + 1);
        out.push('x');
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

#[tokio::test]
async fn every_pack_endpoint_is_served_or_named_as_left_out() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/api/pack_endpoints.json");
    let endpoints: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(endpoints.len() >= 150);
    let (base, client, _dir) = start().await;
    let (mut served, mut missing, mut stale) = (0, Vec::new(), Vec::new());
    for endpoint in &endpoints {
        let (method, pattern) = (endpoint["method"].as_str().unwrap(), endpoint["path"].as_str().unwrap());
        // The event stream never ends. Its route is checked by the API tests.
        if method == "GET" && pattern == "/events" {
            served += 1;
            continue;
        }
        let url = format!("{base}{}", concrete(pattern));
        let method_value = Method::from_bytes(method.as_bytes()).unwrap();
        let request = client.request(method_value, &url).json(&serde_json::json!({}));
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let is_json = response.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("json"));
        let routed = status != 405 && !(status == 404 && !is_json);
        match (routed, left_out(method, pattern)) {
            (true, false) => served += 1,
            (false, false) => missing.push(format!("{method} {pattern} -> {status}")),
            (true, true) => stale.push(format!("{method} {pattern}")),
            (false, true) => {}
        }
    }
    assert!(missing.is_empty(), "endpoints neither served nor listed as left out:\n{}", missing.join("\n"));
    assert!(stale.is_empty(), "endpoints listed as left out but served now, remove them from LEFT_OUT:\n{}", stale.join("\n"));
    assert!(served + LEFT_OUT.len() >= endpoints.len(), "{served} served, {} left out, {} endpoints", LEFT_OUT.len(), endpoints.len());
    eprintln!("[route coverage] {served} of {} pack endpoints served, {} left out on purpose", endpoints.len(), LEFT_OUT.len());
}
