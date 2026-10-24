//! The Story Arc helpers against what the pack's own functions returned for the same inputs (`fixtures/story/arc_helpers.json`, made by
//! `tools/make_story_arc_fixtures.py`).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use h3_core::pyval::Obj;
use h3_core::story::arc::{cap_words, normalize_output, parse_lyric_sections, scene_entries_enabled, scene_map, section_word_limit, split_sections};
use h3_core::story::cast_guard::{build_cast_guard, strip_story_arc_entry_leaks};
use serde_json::{json, Value};

fn cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/story/arc_helpers.json");
    serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["cases"].as_array().cloned().unwrap()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().map(|items| items.iter().map(|i| i.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

fn object(value: &Value) -> Obj {
    value.as_object().cloned().unwrap_or_default()
}

fn pairs(items: Vec<(String, String)>) -> Value {
    Value::Array(items.into_iter().map(|(a, b)| json!([a, b])).collect())
}

fn run(name: &str, args: &Value) -> Result<Value, String> {
    let list = args.as_array();
    let arg = |index: usize| list.map_or(&Value::Null, |items| &items[index]);
    Ok(match name {
        "_parse_story_arc_lyric_sections" => pairs(parse_lyric_sections(arg(0).as_str().unwrap(), arg(1).as_bool().unwrap())),
        "_cap_story_arc_words" => json!(cap_words(arg(0).as_str().unwrap(), arg(1).as_i64().unwrap())),
        "_story_arc_section_word_limit" => json!(section_word_limit(arg(0).as_i64().unwrap() as usize, arg(1).as_i64().unwrap(), arg(2).as_i64().unwrap())),
        "_story_arc_scene_map" => {
            let rows = arg(0).as_array().cloned().unwrap_or_default();
            Value::Array(scene_map(&rows, &strings(arg(1))).into_iter().map(|(label, group)| json!([label, group])).collect())
        }
        "_normalize_story_arc_output" => json!(normalize_output(arg(0).as_str().unwrap(), &strings(arg(1)), arg(2).as_i64().unwrap(), arg(3).as_str().unwrap_or(""))?),
        "_split_story_arc_sections" => {
            let found = split_sections(arg(0).as_str().unwrap(), &strings(arg(1)));
            Value::Object(found.into_iter().map(|(label, body)| (label, json!(body))).collect())
        }
        "_story_arc_scene_entries_enabled" => json!(scene_entries_enabled(&object(arg(0)), &object(arg(1)), arg(2).as_str().unwrap())),
        "strip_story_arc_entry_leaks" => {
            let guards = object(&args["guards"])
                .iter()
                .map(|(number, spec)| {
                    let cards = |key: &str| spec[key].as_array().cloned().unwrap_or_default();
                    (number.parse::<i64>().unwrap(), build_cast_guard(&cards("all"), &cards("cast"), &strings(&spec["extras"])))
                })
                .collect();
            let names: HashMap<i64, Vec<String>> = object(&args["names"]).iter().map(|(number, list)| (number.parse().unwrap(), strings(list))).collect();
            json!(strip_story_arc_entry_leaks(args["text"].as_str().unwrap(), &guards, &names))
        }
        other => panic!("unknown function {other}"),
    })
}

#[test]
fn the_arc_helpers_match_the_pack() {
    let all = cases();
    assert!(all.len() > 140, "{} cases", all.len());
    let mut failures = Vec::new();
    for (number, case) in all.iter().enumerate() {
        let name = case["fn"].as_str().unwrap();
        let got = run(name, &case["args"]);
        let ok = match (&got, case.get("error")) {
            (Ok(value), None) => value == &case["return"],
            (Err(message), Some(want)) => want.as_str() == Some(message.as_str()),
            _ => false,
        };
        if !ok {
            failures.push(format!("case {number} {name}\n  args {}\n  got  {got:?}\n  want {}", case["args"], case.get("error").unwrap_or(&case["return"])));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ\n{}", failures.len(), all.len(), failures.iter().take(4).cloned().collect::<Vec<_>>().join("\n"));
}
