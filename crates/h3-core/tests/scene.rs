//! Scene input resolution against `fixtures/scene`.

mod common;

use h3_core::pyval::Obj;
use h3_core::scene::{
    media_path_key, ordered_reference_items, reference_choices, render_reference_image_paths, resolve_scene_inputs,
    scene_map_value, segment_image_path, SceneInputsArgs,
};
use serde_json::{json, Value};

/// The fixtures' fake folders count as existing files, like the generator's patched `os.path.isfile`.
fn fake_is_file(path: &str) -> bool {
    let name = path.rsplit(['\\', '/']).next().unwrap_or("");
    path.to_lowercase().starts_with("z:\\h3fixture") && name.contains('.')
}

fn segment_of(input: &Value, segments: &Value) -> Obj {
    match &input["segment"] {
        Value::String(name) => segments[name.as_str()].as_object().unwrap_or_else(|| panic!("segment {name}")).clone(),
        Value::Object(o) => o.clone(),
        _ => Obj::new(),
    }
}

fn session() -> Value {
    common::load("scene/session.json")
}

#[test]
fn path_keys_and_scene_maps() {
    for c in common::cases("scene/media_path_key.json") {
        assert_eq!(json!(media_path_key(Some(&c["input"]["path"]))), c["output"], "{c}");
    }
    for c in common::cases("scene/scene_map_value.json") {
        let i = &c["input"];
        let segment = i["segment"].as_object().unwrap();
        let got = scene_map_value(Some(&i["mapping"]), segment, i["index"].as_u64().unwrap() as usize);
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn segment_images() {
    let segments = common::load("scene/segments.json");
    for c in common::cases("scene/segment_image_path.json") {
        let segment = segment_of(&c["input"], &segments);
        assert_eq!(json!(segment_image_path(&segment)), c["output"], "{c}");
    }
}

#[test]
fn ordered_references() {
    let (segments, session) = (common::load("scene/segments.json"), session());
    let cases = common::cases("scene/ordered_reference_items.json");
    assert_eq!(cases.len(), 100);
    for c in cases {
        let i = &c["input"];
        let segment = segment_of(i, &segments);
        let got = ordered_reference_items(&session, &segment, i["mode"].as_str().unwrap(), i["index"].as_u64().unwrap() as usize);
        common::assert_json(&json!(got), &c["output"], &c);
    }
}

#[test]
fn reference_picker_choices() {
    let (segments, session) = (common::load("scene/segments.json"), session());
    for c in common::cases("scene/reference_choices.json") {
        let i = &c["input"];
        let segment = segment_of(i, &segments);
        let got = reference_choices(&session, &segment, i["index"].as_u64().unwrap() as usize, i["mode"].as_str().unwrap());
        common::assert_json(&got, &c["output"], &c);
    }
}

#[test]
fn render_image_paths() {
    let (segments, session) = (common::load("scene/segments.json"), session());
    for c in common::cases("scene/render_reference_image_paths.json") {
        let i = &c["input"];
        let segment = segment_of(i, &segments);
        let configured = i["configured"].as_array();
        let got = render_reference_image_paths(
            &session,
            &segment,
            i["mode"].as_str().unwrap(),
            i["index"].as_u64().unwrap() as usize,
            configured.map(Vec::as_slice),
        );
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn scene_inputs() {
    let (segments, session) = (common::load("scene/segments.json"), session());
    let cases = common::cases("scene/resolve_scene_inputs.json");
    assert_eq!(cases.len(), 360);
    for c in cases {
        let i = &c["input"];
        let segment = segment_of(i, &segments);
        let scene_number = i["scene_number"].as_i64().unwrap();
        let args = SceneInputsArgs {
            continuity_mode: &i["continuity"],
            has_previous_segment: scene_number > 1,
            scene_number,
            configured_image_paths: None,
            configured_video_references: None,
            render_pass: i["render_pass"].as_str().unwrap(),
        };
        let got = resolve_scene_inputs(&session, &segment, i["mode"].as_str().unwrap(), (scene_number - 1) as usize, &args, &fake_is_file);
        match (got, c.get("error")) {
            (Ok(out), None) => common::assert_json(&Value::Object(out), &c["output"], &c),
            (Err(e), Some(err)) => assert_eq!(json!(e.message), err["message"], "{c}"),
            (got, _) => panic!("ok={} vs {c}", got.is_ok()),
        }
    }
}
