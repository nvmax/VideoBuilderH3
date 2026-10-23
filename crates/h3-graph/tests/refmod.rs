//! The RefMod rewire against `fixtures/timeline/refmod_graphs.json`: the pack's `apply_refmod_pipeline` run on golden graphs.

use std::path::Path;

use h3_graph::refmod::{apply_refmod_pipeline, RefmodEntry};
use serde_json::Value;

fn read(relative: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(relative);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn rewired_graphs_match_the_pack() {
    let data = read("timeline/refmod_graphs.json");
    let library: Vec<RefmodEntry> = data["library"]
        .as_object()
        .unwrap()
        .values()
        .map(|e| RefmodEntry { name: e["name"].as_str().unwrap().into(), kind: e["kind"].as_str().unwrap().into(), tokens: e["tokens"].as_i64().unwrap() })
        .collect();
    let find = |name: &str| library.iter().find(|e| e.name == name).cloned();
    let (mut ok, mut refused) = (0, 0);
    for case in data["cases"].as_array().unwrap() {
        let label = format!("{} / {}", case["graph"], case["payload_name"]);
        let graph = read(&format!("graphs/{}.json", case["graph"].as_str().unwrap()));
        let mut prompt = graph["result"]["prompt"].as_object().unwrap().clone();
        let payload = case["payload"].as_object().unwrap();
        match (apply_refmod_pipeline(&mut prompt, payload, &find), case["return"].get("error")) {
            (Ok(summary), None) => {
                assert_eq!(Value::Object(prompt), case["return"]["prompt"], "{label}: graph");
                assert_eq!(summary, case["return"]["summary"], "{label}: summary");
                ok += 1;
            }
            (Err(e), Some(error)) => {
                assert_eq!(e.to_string(), error.as_str().unwrap(), "{label}");
                refused += 1;
            }
            (got, want) => panic!("{label}: {got:?} vs {want:?}"),
        }
    }
    assert!(ok >= 50 && refused >= 50, "{ok} rewired, {refused} refused");
}
