//! The built-in presets against `fixtures/image`: the graph the pack's own patch function made for the same inputs.

use std::fs;
use std::path::PathBuf;

use h3_core::pyval::Obj;
use h3_image::{apply_preset, builtin_presets, ApplyCtx};
use serde_json::{json, Value};

fn cases(name: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/image").join(format!("{name}.json"));
    let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    value["cases"].as_array().cloned().unwrap()
}

/// Reference files only matter by name: `image_paths` becomes the list of base names, in either spelling.
fn normalize(graph: &mut Value) {
    let Some(nodes) = graph.as_object_mut() else { return };
    for node in nodes.values_mut() {
        let Some(paths) = node.get_mut("inputs").and_then(|i| i.get_mut("image_paths")) else { continue };
        let list: Vec<String> = serde_json::from_str(paths.as_str().unwrap_or("[]")).unwrap_or_default();
        let names: Vec<String> = list.iter().map(|p| p.rsplit(['\\', '/']).next().unwrap_or("").to_string()).collect();
        *paths = json!(names);
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.is_f64() == y.is_f64() && x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

fn run(preset_id: &str) -> usize {
    let preset = builtin_presets().into_iter().find(|p| p.id == preset_id).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut checked = 0;
    for (i, case) in cases(preset_id).iter().enumerate() {
        let mut values: Obj = case["values"].as_object().cloned().unwrap();
        // Reference files must exist for the app, so each fixture path becomes a real empty file of the same name.
        if let Some(Value::Array(refs)) = values.get("reference_images").cloned() {
            let real: Vec<Value> = refs
                .iter()
                .map(|r| {
                    let name = r.as_str().unwrap().rsplit('\\').next().unwrap().to_string();
                    let path = dir.path().join(&name);
                    fs::write(&path, b"x").unwrap();
                    json!(path.display().to_string())
                })
                .collect();
            values.insert("reference_images".into(), Value::Array(real));
        }
        let mut ctx = ApplyCtx { input_dir: dir.path().join("input"), random: Box::new(|| 1) };
        let mut got = apply_preset(&preset, &values, &mut ctx).unwrap_or_else(|e| panic!("{preset_id} case {i}: {e}")).graph;
        let mut want = case["graph"].clone();
        normalize(&mut got);
        normalize(&mut want);
        if !same(&got, &want) {
            let diffs: Vec<String> = want
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, v)| got.get(k.as_str()).is_none_or(|g| !same(g, v)))
                .map(|(k, v)| format!("node {k}: want {} got {}", v["inputs"], got.get(k.as_str()).map_or(Value::Null, |g| g["inputs"].clone())))
                .collect();
            panic!("{preset_id} case {i} differs: {}{}", diffs.join(" | "), if got.as_object().unwrap().len() != want.as_object().unwrap().len() { " (node count differs)" } else { "" });
        }
        checked += 1;
    }
    checked
}

#[test]
fn z_image_matches_the_pack() {
    assert_eq!(run("zimage"), 5);
}

#[test]
fn ernie_matches_the_pack() {
    assert_eq!(run("ernie"), 4);
}

#[test]
fn flux_klein_matches_the_pack() {
    assert_eq!(run("flux_klein"), 5);
}
