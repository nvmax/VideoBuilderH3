//! The clear memory graph, the Beat Mode graph and the MiniMax draft graphs against what the pack's own code made
//! (`tools/make_utility_fixtures.py`). Draft ids and folders are random, so both sides are compared with `<ID>` and `<FOLDER>`.

use std::fs;
use std::path::PathBuf;

use h3_builder::drafts::{build_draft, build_finalize};
use h3_core::pyval::Obj;
use h3_graph::utility::{beat_srt_graph, clear_memory_graph, BeatSrtRequest};
use serde_json::{json, Value};

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pipeline/utility_graphs.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn obj(value: &Value) -> Obj {
    value.as_object().cloned().unwrap()
}

fn mask(value: &mut Value, folder: &str, id: &str) {
    match value {
        Value::String(s) => *s = s.replace(folder, "<FOLDER>").replace(id, "<ID>"),
        Value::Array(items) => items.iter_mut().for_each(|v| mask(v, folder, id)),
        Value::Object(map) => map.values_mut().for_each(|v| mask(v, folder, id)),
        _ => {}
    }
}

#[test]
fn the_clear_memory_graph_is_the_packs() {
    let (_, prompt) = clear_memory_graph().unwrap();
    assert_eq!(Value::Object(prompt), fixture()["clear_memory"]);
}

#[test]
fn the_beat_graph_matches_the_pack() {
    let data = fixture();
    let cases = data["beat"].as_array().unwrap();
    assert_eq!(cases.len(), 6);
    for case in cases {
        let mut payload = obj(&case["request"]);
        payload.insert("project_folder".into(), json!("unused"));
        let request = BeatSrtRequest { audio_upload_name: "song.wav", staged_audio_path: "Z:\\h3fixture\\comfy\\input\\song.wav", stamp: "20260101_000000" };
        match beat_srt_graph(&payload, &request) {
            Ok(prompt) => assert_eq!(Value::Object(prompt), case["return"]["prompt"], "{}", case["request"]),
            Err(error) => assert_eq!(error.message, case["return"]["error"].as_str().unwrap(), "{}", case["request"]),
        }
    }
}

#[test]
fn drafts_match_the_pack() {
    let data = fixture();
    let cases = data["drafts"].as_array().unwrap();
    assert!(cases.len() >= 20, "{} cases", cases.len());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().display().to_string();
        let mut request = obj(&case["request"]);
        request.insert("project_folder".into(), json!(project));
        let draft = build_draft(&obj(&case["result"]), &request).unwrap();
        let id = draft["draft_id"].as_str().unwrap().to_string();
        let folder = dir.path().join("minimax_drafts").join(&id);
        let folder_text = folder.display().to_string();
        let mut got = draft.clone();
        mask(&mut got, &folder_text, &id);
        assert_eq!(got, case["draft"], "{name}: draft graph");
        let mut saved: Value = serde_json::from_str(&fs::read_to_string(folder.join("workflow.json")).unwrap()).unwrap();
        mask(&mut saved, &folder_text, &id);
        assert_eq!(saved, case["saved"], "{name}: saved workflow");

        fs::write(folder.join("pass1.pt"), b"latent").unwrap();
        let mut finish = obj(&json!({"project_folder": project, "draft_id": id, "scene_number": 4, "timeline_start_seconds": 12.5, "timeline_end_seconds": 18.0}));
        let mut done = build_finalize(&finish).unwrap();
        mask(&mut done, &folder_text, &id);
        assert_eq!(done, case["finalize"], "{name}: finalize graph");
        finish.insert("diffusion_model_name".into(), json!("C:\\other\\not_this_model.safetensors"));
        assert_eq!(build_finalize(&finish).unwrap(), case["stale"], "{name}: stale draft");
        let used = case["stale"]["draft_models"].as_array().cloned().unwrap_or_default();
        let model = used.first().and_then(Value::as_str).unwrap_or("none.safetensors");
        finish.insert("diffusion_model_name".into(), json!(format!("dir/{model}")));
        let mut same = build_finalize(&finish).unwrap();
        mask(&mut same, &folder_text, &id);
        assert_eq!(same, case["same_model"], "{name}: same model");
        finish.remove("diffusion_model_name");
        finish.insert("scene_number".into(), json!(5));
        assert_eq!(build_finalize(&finish).unwrap_err(), case["errors"]["scene"].as_str().unwrap(), "{name}");
        finish.insert("scene_number".into(), json!(4));
        finish.insert("timeline_end_seconds".into(), json!(19.0));
        assert_eq!(build_finalize(&finish).unwrap_err(), case["errors"]["timing"].as_str().unwrap(), "{name}");
    }
}
