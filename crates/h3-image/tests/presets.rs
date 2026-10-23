//! Import, bindings, the store, and the parts of applying a preset that depend on files and the clock.

use h3_core::pyval::Obj;
use h3_image::{apply_preset, builtin_presets, choices_for, parse_api_workflow, suggest_bindings, ApplyCtx, BindingKind, ImageError, PresetOrigin, PresetStore};
use serde_json::{json, Value};

/// A small text to image workflow in ComfyUI's API format, the shape any exported workflow has.
fn workflow() -> Value {
    json!({
        "3": {"class_type": "KSampler", "inputs": {"seed": 5, "steps": 20, "cfg": 7.5, "sampler_name": "euler", "scheduler": "normal", "denoise": 1,
            "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0]}},
        "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "model.safetensors"}},
        "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 768, "batch_size": 1}},
        "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a house", "clip": ["4", 1]}},
        "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "blurry", "clip": ["4", 1]}},
        "8": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["4", 2]}},
        "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "x", "images": ["8", 0]}},
        "10": {"class_type": "LoadImage", "inputs": {"image": "start.png"}}
    })
}

fn ctx(dir: &std::path::Path) -> ApplyCtx {
    ApplyCtx { input_dir: dir.join("input"), random: Box::new(|| 123_456_789_012_345_678) }
}

fn values(pairs: &[(&str, Value)]) -> Obj {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[test]
fn the_editor_format_and_broken_files_are_refused_with_a_reason() {
    let editor = json!({"nodes": [], "links": [], "version": 0.4}).to_string();
    let err = parse_api_workflow(&editor).unwrap_err().to_string();
    assert!(err.contains("Export (API)"), "{err}");
    assert!(parse_api_workflow("not json").unwrap_err().to_string().contains("not valid JSON"));
    assert!(parse_api_workflow("{\"1\": {\"inputs\": {}}}").unwrap_err().to_string().contains("Node 1"));
    // A request body is accepted too.
    let body = json!({"prompt": workflow(), "client_id": "x"}).to_string();
    assert_eq!(parse_api_workflow(&body).unwrap().as_object().unwrap().len(), 8);
}

#[test]
fn bindings_are_guessed_from_the_graph() {
    let (bindings, outputs) = suggest_bindings(&workflow());
    let key = |k: &str| bindings.iter().find(|b| b.key == k).unwrap_or_else(|| panic!("no binding {k}: {:?}", bindings.iter().map(|b| &b.key).collect::<Vec<_>>()));
    assert_eq!(key("prompt").targets[0].node, "6");
    assert_eq!(key("negative_prompt").targets[0].node, "7");
    assert_eq!(key("negative_prompt").default, json!("blurry"));
    assert_eq!(key("width").default, json!(512));
    assert_eq!(key("seed").kind, BindingKind::Seed);
    assert_eq!(key("steps").default, json!(20));
    assert_eq!(key("cfg").kind, BindingKind::Float);
    assert_eq!(key("ckpt_name").kind, BindingKind::Choice);
    assert_eq!(key("image").kind, BindingKind::Image);
    assert_eq!(outputs, vec!["9".to_string()]);

    // The model list comes from ComfyUI's node info.
    let info = json!({"CheckpointLoaderSimple": {"input": {"required": {"ckpt_name": [["a.safetensors", "b.safetensors"]]}}}});
    assert_eq!(choices_for(&workflow(), key("ckpt_name"), &info), vec!["a.safetensors", "b.safetensors"]);
}

#[test]
fn an_imported_workflow_is_filled_from_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let store = PresetStore::new(&dir.path().join("presets"));
    let preset = store.import("My Model", "test", &workflow().to_string()).unwrap();
    assert_eq!(preset.id, "my_model");
    assert_eq!(preset.origin, PresetOrigin::Custom);
    // A second import with the same name gets its own id, and the store lists both after the built-ins.
    assert_eq!(store.import("My Model", "", &workflow().to_string()).unwrap().id, "my_model_2");
    let names: Vec<String> = store.list().into_iter().map(|p| p.id).collect();
    assert_eq!(names, ["zimage", "flux_klein", "ernie", "my_model", "my_model_2"]);

    let start = dir.path().join("start.png");
    std::fs::write(&start, b"png").unwrap();
    let v = values(&[
        ("prompt", json!("a red house")),
        ("width", json!(1024)),
        ("height", json!(100000)),
        ("seed", json!(-1)),
        ("steps", json!(30)),
        ("ckpt_name", json!("other.safetensors")),
        ("image", json!(start.display().to_string())),
    ]);
    let applied = apply_preset(&preset, &v, &mut ctx(dir.path())).unwrap();
    let g = &applied.graph;
    assert_eq!(g["6"]["inputs"]["text"], "a red house");
    assert_eq!(g["7"]["inputs"]["text"], "blurry");
    assert_eq!(g["5"]["inputs"]["width"], 1024);
    assert_eq!(g["5"]["inputs"]["height"], 8192, "clamped to the binding's range");
    assert_eq!(g["3"]["inputs"]["steps"], 30);
    assert_eq!(g["4"]["inputs"]["ckpt_name"], "other.safetensors");
    // A negative seed is replaced by a random one that fits a JSON number.
    let seed = applied.seed.unwrap();
    assert_eq!(g["3"]["inputs"]["seed"], json!(seed));
    assert!(seed < (1 << 53));
    // The image was copied into ComfyUI's input folder under a new name.
    let name = g["10"]["inputs"]["image"].as_str().unwrap();
    assert!(name.starts_with("vrgdg_img_") && dir.path().join("input").join(name).is_file(), "{name}");
    // The original workflow is untouched.
    assert_eq!(preset.workflow["6"]["inputs"]["text"], "a house");
}

#[test]
fn custom_presets_are_saved_changed_and_deleted_but_builtins_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let store = PresetStore::new(dir.path());
    let mut preset = store.import("Mine", "", &workflow().to_string()).unwrap();
    preset.bindings.retain(|b| b.key != "negative_prompt");
    store.save(&preset).unwrap();
    assert!(store.get("mine").unwrap().bindings.iter().all(|b| b.key != "negative_prompt"));
    store.delete("mine").unwrap();
    assert!(matches!(store.delete("mine"), Err(ImageError::NotFound(_))));
    assert!(store.delete("zimage").is_err());
    let mut copy = builtin_presets().remove(0);
    copy.origin = PresetOrigin::Custom;
    assert!(store.save(&copy).is_err(), "a built-in id cannot be overwritten");
    assert!(store.import("No prompt", "", &json!({"1": {"class_type": "VAEDecode", "inputs": {}}}).to_string()).is_err());
}

#[test]
fn z_image_to_image_copies_the_file_and_switches_the_branch_on() {
    let dir = tempfile::tempdir().unwrap();
    let preset = builtin_presets().into_iter().find(|p| p.id == "zimage").unwrap();
    let start = dir.path().join("pic.jpg");
    std::fs::write(&start, b"jpg").unwrap();
    let on = apply_preset(&preset, &values(&[("prompt", json!("x")), ("image_to_image", json!(start.display().to_string()))]), &mut ctx(dir.path())).unwrap();
    assert_eq!(on.graph["978"]["inputs"]["switch"], true);
    assert_eq!(on.graph["981"]["inputs"]["switch"], true);
    assert!(on.graph["979"]["inputs"]["image"].as_str().unwrap().ends_with(".jpg"));
    let off = apply_preset(&preset, &values(&[("prompt", json!("x"))]), &mut ctx(dir.path())).unwrap();
    assert_eq!(off.graph["978"]["inputs"]["switch"], false);
    assert_eq!(off.graph["979"]["inputs"]["image"], "vrgdg_placeholder_i2i.png");
    assert!(dir.path().join("input/vrgdg_placeholder_i2i.png").is_file());
}

#[test]
fn bad_input_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let flux = builtin_presets().into_iter().find(|p| p.id == "flux_klein").unwrap();
    let empty = apply_preset(&flux, &values(&[("prompt", json!("  "))]), &mut ctx(dir.path())).unwrap_err().to_string();
    assert_eq!(empty, "Prompt text is empty.");
    let missing = apply_preset(&flux, &values(&[("prompt", json!("x")), ("reference_images", json!(["nope.png"]))]), &mut ctx(dir.path())).unwrap_err().to_string();
    assert!(missing.contains("Reference image not found"), "{missing}");
    let nodes = h3_image::missing_nodes(&flux, &json!({"KSamplerSelect": {}}));
    assert!(nodes.contains(&"VRGDG_MultiReferenceConditioningFromPaths".to_string()));
    assert!(h3_image::missing_nodes(&flux, &Value::Null).is_empty(), "no node list means no check");
}
