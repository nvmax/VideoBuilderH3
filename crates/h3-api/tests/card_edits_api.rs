//! Bulk scene edits fail loudly, a location save keeps what it is not sent, and a card can be bound to a saved RefMod.

use std::path::Path;
use std::time::Duration;

use h3_project::AppConfig;
use reqwest::{Client, Method};
use serde_json::{json, Value};

struct Server {
    base: String,
    client: Client,
    _dir: tempfile::TempDir,
}

/// A ComfyUI that only tells where its folders are, with two saved RefMods in its `refmods` folder.
async fn fake_comfy(refmods: &Path) -> String {
    for (name, concept) in [("vehicle/C8_Corvette", "vehicle"), ("identity/Darrel_noclothes", "identity")] {
        let path = refmods.join(format!("{name}.safetensors"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let meta = json!({"kind": "video", "latent_t": 3, "latent_h": 8, "latent_w": 8, "concept_type": concept});
        let header = serde_json::to_vec(&json!({"__metadata__": {"refmod_meta": meta.to_string()}})).unwrap();
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend(header);
        std::fs::write(path, bytes).unwrap();
    }
    let folders = json!({"refmods": [refmods.display().to_string()]});
    let app = axum::Router::new().route("/internal/folder_paths", axum::routing::get(move || async move { axum::Json(folders) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    let comfy = fake_comfy(&dir.path().join("refmods")).await;
    let cfg = AppConfig { comfy_url: comfy, project_root: root, ..AppConfig::default() };
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

    async fn get(&self, path: &str) -> Value {
        let (code, response) = self.call(Method::GET, path, None).await;
        assert_eq!(code, 200, "{path}: {response}");
        response["data"].clone()
    }

    async fn project(&self) -> &Self {
        self.ok(Method::POST, "/projects", json!({"name": "Cards"})).await;
        self.ok(Method::POST, "/projects/Cards/timeline/bulk", json!({"text": "4 a\n4 b\n4 c", "mode": "durations"})).await;
        self
    }

    async fn scene(&self, number: u32) -> Value {
        let (code, body) = self.call(Method::GET, &format!("/projects/Cards/scenes/{number}"), None).await;
        assert_eq!(code, 200, "{body}");
        body["data"].clone()
    }

    async fn revision(&self) -> i64 {
        let (_, body) = self.call(Method::GET, "/projects/Cards/settings", None).await;
        body["revision"].as_i64().unwrap()
    }
}

#[tokio::test]
async fn a_bulk_edit_takes_scene_numbers_and_a_bad_operation_saves_nothing() {
    let s = start().await;
    s.project().await;
    let first = s.scene(1).await["id"].as_str().unwrap().to_string();
    let done = s
        .ok(Method::POST, "/projects/Cards/scenes/bulk", json!({"operations": [
            {"op": "patch", "scene_id": "2", "fields": {"timeline_note": "Rain starts"}},
            {"op": "resize", "scene_id": 3, "duration": 2.0},
            {"op": "patch", "scene_id": first, "fields": {"timeline_note": "Open on the door"}},
        ]}))
        .await;
    assert_eq!(done["applied_operations"], 3);
    assert_eq!(s.scene(2).await["timeline_note"], "Rain starts");
    assert_eq!(s.scene(1).await["timeline_note"], "Open on the door");
    assert_eq!(s.scene(3).await["end"], 10.0);

    let before = s.revision().await;
    let bad = [
        (json!({}), 400),
        (json!({"operations": []}), 400),
        (json!({"operations": "patch"}), 400),
        (json!({"operations": ["patch"]}), 400),
        (json!({"operations": [{"op": "paint", "scene_id": "1"}]}), 400),
        (json!({"operations": [{"scene_id": "1", "fields": {"notes": "x"}}]}), 400),
        (json!({"operations": [{"op": "patch", "fields": {"notes": "x"}}]}), 400),
        (json!({"operations": [{"op": "patch", "scene_id": "1"}]}), 400),
        (json!({"operations": [{"op": "move", "scene_id": "1"}]}), 400),
        (json!({"operations": [{"op": "resize", "scene_id": "1"}]}), 400),
        (json!({"operations": [{"op": "patch", "scene_id": "1", "fields": {"notes": "ok"}}, {"op": "patch", "scene_id": "9", "fields": {"notes": "x"}}]}), 404),
    ];
    for (body, want) in bad {
        let (code, answer) = s.call(Method::POST, "/projects/Cards/scenes/bulk", Some(body.clone())).await;
        assert_eq!(code, want, "{body}: {answer}");
    }
    assert_eq!(s.revision().await, before);
    assert_eq!(s.scene(1).await["timeline_note"], "Open on the door");
}

#[tokio::test]
async fn a_location_save_keeps_the_fields_it_is_not_sent() {
    let s = start().await;
    s.project().await;
    s.ok(Method::PUT, "/projects/Cards/references/locations/foyer", json!({"name": "Foyer", "description": "Marble floor", "trigger_phrase": "in the foyer"})).await;
    let saved = s.ok(Method::PUT, "/projects/Cards/references/locations/foyer", json!({"image": {"path": "C:/plates/foyer.png"}})).await;
    let card = &saved["location"];
    assert_eq!((card["name"].as_str(), card["description"].as_str(), card["trigger_phrase"].as_str()), (Some("Foyer"), Some("Marble floor"), Some("in the foyer")));
    assert_eq!(card["image"]["path"], "C:/plates/foyer.png");
    let fresh = s.ok(Method::PUT, "/projects/Cards/references/locations/hall", json!({})).await;
    assert_eq!(fresh["location"]["name"], "Location");
}

#[tokio::test]
async fn a_card_is_bound_to_a_saved_refmod_and_the_render_list_shows_it() {
    let s = start().await;
    s.project().await;
    s.ok(Method::PATCH, "/projects/Cards/settings", json!({"minimax_h3": {"pipeline": "refmod"}})).await;

    let darrel = s
        .ok(Method::PUT, "/projects/Cards/references/subjects/darrel_bare", json!({
            "name": "Darrel Bare", "source": "refmod", "refmod": {"name": "identity/Darrel_noclothes", "strength": 0.8},
        }))
        .await;
    assert_eq!(darrel["subject"]["source"], "refmod");
    assert_eq!(darrel["subject"]["refmod"], json!({"name": "identity/Darrel_noclothes", "folder": "identity", "type": "identity", "kind": "video", "tokens": 48, "frames": 3, "strength": 0.8}));

    // The RefMod may be sent as its name, with full strength by default.
    let vette = s.ok(Method::PUT, "/projects/Cards/references/subjects/vette", json!({"name": "Corvette", "reference_type": "vehicle", "refmod": "vehicle/C8_Corvette"})).await;
    assert_eq!((vette["subject"]["source"].as_str(), vette["subject"]["refmod"]["strength"].as_f64()), (Some("refmod"), Some(1.0)));

    // Mapped to a scene, both are in that scene's render list.
    let first = s.scene(1).await["id"].as_str().unwrap().to_string();
    s.ok(Method::PUT, "/projects/Cards/references/scene-mapping", json!({"subjects": {first: ["darrel_bare", "vette"]}})).await;
    let list = s.get("/projects/Cards/scenes/1/minimax-references").await;
    let names: Vec<&str> = list["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap_or("")).collect();
    assert!(names.contains(&"Darrel Bare") && names.contains(&"Corvette"), "{list}");
    assert!(list["warnings"].as_array().unwrap().is_empty(), "{list}");

    // A save that does not mention the RefMod keeps it, the same RefMod keeps its strength, and `source: image` takes it off.
    let kept = s.ok(Method::PUT, "/projects/Cards/references/subjects/darrel_bare", json!({"description": "No shirt"})).await;
    assert_eq!((kept["subject"]["refmod"]["strength"].as_f64(), kept["subject"]["description"].as_str()), (Some(0.8), Some("No shirt")));
    let again = s.ok(Method::PUT, "/projects/Cards/references/subjects/darrel_bare", json!({"source": "refmod", "refmod": {"name": "identity/Darrel_noclothes"}})).await;
    assert_eq!(again["subject"]["refmod"]["strength"], 0.8);
    let off = s.ok(Method::PUT, "/projects/Cards/references/subjects/darrel_bare", json!({"source": "image"})).await;
    assert_eq!(off["subject"]["source"], "image");
    assert!(off["subject"].get("refmod").is_none(), "{off}");

    // A location is bound the same way.
    let garage = s.ok(Method::PUT, "/projects/Cards/references/locations/garage", json!({"name": "Garage", "source": "refmod", "refmod": {"name": "vehicle/C8_Corvette"}})).await;
    assert_eq!(garage["location"]["refmod"]["name"], "vehicle/C8_Corvette");
}

#[tokio::test]
async fn a_refmod_that_is_not_saved_is_refused_and_the_error_names_the_ones_that_are() {
    let s = start().await;
    s.project().await;
    let (code, body) = s.call(Method::PUT, "/projects/Cards/references/subjects/vette", Some(json!({"source": "refmod", "refmod": {"name": "vehicle/Camaro"}}))).await;
    assert_eq!((code, body["error"]["code"].as_str()), (400, Some("VALIDATION_ERROR")), "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("vehicle/Camaro") && message.contains("vehicle/C8_Corvette") && message.contains("identity/Darrel_noclothes"), "{message}");
    for payload in [
        json!({"source": "refmod"}),
        json!({"source": "paint"}),
        json!({"refmod": {"name": "vehicle/C8_Corvette", "strength": 3}}),
        json!({"refmod": {"name": "vehicle/C8_Corvette", "strength": "strong"}}),
    ] {
        let (code, body) = s.call(Method::PUT, "/projects/Cards/references/subjects/vette", Some(payload.clone())).await;
        assert_eq!(code, 400, "{payload}: {body}");
    }
    let refs = s.get("/projects/Cards/references").await;
    assert!(refs["subjects"].as_array().unwrap().is_empty(), "{refs}");
}
