//! The generated fixtures parse, match the manifest and have the shapes the ports rely on.

mod common;

use common::{cases, files_in, load};

#[test]
fn manifest_counts_match_files() {
    let manifest = load("MANIFEST.json");
    assert!(manifest["pack_commit"].as_str().is_some_and(|c| c.len() >= 7), "pack commit recorded");
    for folder in ["graphs", "core", "scene", "prompt", "project", "cases"] {
        let expected = manifest["files"][folder].as_u64().unwrap() as usize;
        let found = files_in(folder).len();
        assert_eq!(found, expected, "fixture count in {folder}");
    }
}

#[test]
fn table_fixtures_have_cases_with_an_output_or_error() {
    for folder in ["core", "scene", "prompt"] {
        for name in files_in(folder) {
            if matches!(name.as_str(), "session.json" | "segments.json" | "named_inputs.json") {
                continue;
            }
            let relative = format!("{folder}/{name}");
            let rows = cases(&relative);
            assert!(!rows.is_empty(), "{relative} is empty");
            for row in &rows {
                assert!(row.get("input").is_some(), "{relative}: case without input");
            }
        }
    }
}

#[test]
fn graph_fixtures_are_complete_graphs_or_recorded_errors() {
    let mut graphs = 0;
    let mut errors = 0;
    for name in files_in("graphs") {
        let record = load(&format!("graphs/{name}"));
        assert!(record["name"].is_string() && record["settings_overrides"].is_object() && record["scene"].is_object(), "{name}");
        match (record.get("result"), record.get("error")) {
            (Some(result), None) => {
                graphs += 1;
                let prompt = result["prompt"].as_object().unwrap_or_else(|| panic!("{name}: no prompt graph"));
                assert!(prompt.len() > 10, "{name}: graph looks too small");
                for (id, node) in prompt {
                    assert!(node["class_type"].is_string() && node["inputs"].is_object(), "{name}: node {id} malformed");
                }
                assert!(result["timing"]["h3_frame_count"].as_u64().unwrap() >= 5, "{name}");
            }
            (None, Some(error)) => {
                errors += 1;
                assert!(error["message"].is_string(), "{name}");
            }
            _ => panic!("{name}: needs exactly one of result or error"),
        }
    }
    assert!(graphs >= 60 && errors >= 4, "{graphs} graphs, {errors} errors");
}

#[test]
fn error_named_graph_fixtures_are_errors_and_others_are_not() {
    for name in files_in("graphs") {
        let record = load(&format!("graphs/{name}"));
        assert_eq!(name.starts_with("error_"), record.get("error").is_some(), "{name}");
    }
}

#[test]
fn pack_case_files_parse() {
    for name in files_in("cases") {
        load(&format!("cases/{name}"));
    }
}

#[test]
fn session_shape_keeps_only_types() {
    let shape = load("session_shape.json");
    assert!(shape["sessions_scanned"].as_u64().unwrap() > 0);
    let paths = shape["paths"].as_object().unwrap();
    assert!(paths.contains_key("$.segments") && paths.contains_key("$.minimax_h3_settings"));
    for (path, types) in paths {
        for type_name in types.as_object().unwrap().keys() {
            assert!(
                ["null", "bool", "int", "float", "string", "array", "object"].contains(&type_name.as_str()),
                "{path}: {type_name}"
            );
        }
    }
}
