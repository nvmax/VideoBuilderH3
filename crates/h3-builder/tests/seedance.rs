//! The ComfyUI graph for Seedance 2.5 against `fixtures/prompt/seedance_graph.json`. Every case is what the pack's
//! `_build_seedance25_reference_prompt` answered for the same request and the same ComfyUI nodes. The random names of the staged
//! references and of the output folder are `<ID>`, and the folder the files are made in is `{dir}`.

use std::path::PathBuf;

use h3_builder::seedance::reference_prompt;
use serde_json::Value;

fn fill(value: &Value, folder: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("{dir}", folder)),
        Value::Array(items) => Value::Array(items.iter().map(|item| fill(item, folder)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(key, item)| (key.clone(), fill(item, folder))).collect()),
        other => other.clone(),
    }
}

fn clean(value: &Value, folder: &str) -> Value {
    match value {
        Value::String(text) => {
            let mut text = text.replace(folder, "{dir}");
            // 32 hex digits are a random name.
            let mut out = String::new();
            let chars: Vec<char> = text.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let run = chars[i..].iter().take_while(|c| c.is_ascii_digit() || matches!(**c, 'a'..='f')).count();
                if run >= 32 {
                    out.push_str("<ID>");
                    i += 32;
                } else {
                    out.push(chars[i]);
                    i += 1;
                }
            }
            text = out;
            Value::String(text)
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| clean(item, folder)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(key, item)| (key.clone(), clean(item, folder))).collect()),
        other => other.clone(),
    }
}

#[test]
fn the_graph_matches_the_pack() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/prompt/seedance_graph.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() >= 40);
    let (mut built, mut refused) = (0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().display().to_string().replace(std::path::MAIN_SEPARATOR, "/");
        let input = dir.path().join("comfy_input");
        std::fs::create_dir_all(&input).unwrap();
        for file in case["files"].as_array().unwrap() {
            std::fs::write(dir.path().join(file.as_str().unwrap()), b"x").unwrap();
        }
        let nodes: Vec<&str> = case["nodes"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let has_node = |class: &str| nodes.contains(&class);
        let payload = fill(&case["payload"], &folder);
        let answer = reference_prompt(&has_node, &input, "<OUT>", payload.as_object().unwrap());
        match answer {
            Ok(result) => {
                assert_eq!(clean(&result, &folder), case["result"], "{name}");
                // The names are sorted after the random part is replaced, as the record is.
                let mut staged: Vec<String> = std::fs::read_dir(&input)
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| clean(&Value::String(entry.file_name().to_string_lossy().into_owned()), &folder).as_str().unwrap().to_string())
                    .collect();
                staged.sort();
                assert_eq!(serde_json::to_value(&staged).unwrap(), case["staged"], "{name}: staged files");
                built += 1;
            }
            Err(message) => {
                assert_eq!(clean(&Value::String(message), &folder), case["error"], "{name}");
                refused += 1;
            }
        }
    }
    assert!(built >= 20 && refused >= 20, "{built} built, {refused} refused");
}
