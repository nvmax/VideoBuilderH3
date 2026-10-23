//! The RefMod library against `fixtures/timeline/refmod_library.json`: the pack's listing of the same folders.

use std::path::{Path, PathBuf};

use h3_pipeline::refmod_library::{find_refmod, list_refmods};
use serde_json::{json, Value};

fn write(root: &Path, name: &str, header: &Value) {
    let path = root.join(format!("{name}.safetensors"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let blob = serde_json::to_vec(header).unwrap();
    let mut bytes = (blob.len() as u64).to_le_bytes().to_vec();
    bytes.extend(blob);
    std::fs::write(path, bytes).unwrap();
}

fn meta(fields: Value) -> Value {
    json!({"__metadata__": {"refmod_meta": fields.to_string()}})
}

#[test]
fn the_listing_matches_the_pack() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/timeline/refmod_library.json");
    let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("one"), dir.path().join("two"));
    for (name, header) in data["files"].as_object().unwrap() {
        write(&first, name, header);
    }
    write(&second, "identity/darrel", &meta(json!({"kind": "image", "latent_h": 2, "latent_w": 2})));
    write(&second, "extra/more", &meta(json!({"kind": "image", "latent_h": 4, "latent_w": 4})));
    for name in data["previews"].as_array().unwrap() {
        std::fs::write(first.join(format!("{}.preview.png", name.as_str().unwrap())), b"png").unwrap();
    }
    let dirs: Vec<PathBuf> = vec![first, second, dir.path().join("missing")];
    let public = |entries: Vec<h3_pipeline::refmod_library::LibraryEntry>| Value::Array(entries.iter().map(|e| e.public()).collect());

    // The pack's size is the file's size, so only compare it for presence.
    let strip = |mut v: Value| {
        for item in v.as_array_mut().unwrap() {
            item.as_object_mut().unwrap().remove("size");
        }
        v
    };
    let answers = &data["answers"];
    assert_eq!(strip(public(list_refmods(&dirs, ""))), strip(answers["list"].clone()));
    for (folder, expected) in answers["folders"].as_object().unwrap() {
        assert_eq!(strip(public(list_refmods(&dirs, folder))), strip(expected.clone()), "folder {folder}");
    }
    for name in data["find"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        let got = find_refmod(&dirs, name).map(|e| {
            let mut v = e.public();
            v.as_object_mut().unwrap().remove("size");
            v
        });
        let want = answers["find"][name].as_object().map(|o| {
            let mut o = o.clone();
            o.remove("size");
            Value::Object(o)
        });
        assert_eq!(got, want, "find {name:?}");
    }
    let darrel = list_refmods(&dirs, "identity").into_iter().find(|e| e.name == "identity/darrel").unwrap();
    assert!(darrel.has_preview && darrel.directory.ends_with("one"), "the first folder wins a repeated name");
}
