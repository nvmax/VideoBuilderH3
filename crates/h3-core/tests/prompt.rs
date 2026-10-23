//! Prompt text builders against `fixtures/prompt`. Every case is a record the pack's Python produced.

mod common;

use h3_core::prompt::continuation::{continuation_task_text, location_continuity_contract, performance_line};
use h3_core::prompt::cues::{cue_cut_plan, cue_delivery, cue_header, cue_shot_contract, enforce_cue_shot, timed_cue_rows, CueRow};
use h3_core::prompt::frame::{
    character_budget, character_reference_bindings, compact_description, compact_reference_prompt, reference_frame,
    reference_labels, validate_prompt, wrap_reference_prompt,
};
use h3_core::prompt::lyrics::{ensure_quoted_lyrics, last_shot_text, lyric_chunks, lyric_lines};
use h3_core::prompt::plan::{
    apply_cut_style, cut_plan_instruction, cut_style_value, performance_craft_instruction, shot_plan, timecode,
};
use h3_core::prompt::task::{build_shot_task, ShotTaskArgs};
use h3_core::prompt::text::{
    assemble_prompt, continuous_cut_text, life_movements, literal_scene_instruction, motion_energy_text, normalize_description,
    opening_wrapper, parse_shot_descriptions, post_cut_text, scene_timing_instruction, shot_body, strip_negative_sentences,
};
use h3_core::pyval::Obj;
use h3_core::CoreError;
use serde_json::{json, Value};

fn named() -> Value {
    common::load("prompt/named_inputs.json")
}

fn plan(named: &Value, name: &str) -> Obj {
    named["plans"][name].as_object().unwrap_or_else(|| panic!("plan {name}")).clone()
}

fn items(named: &Value, name: &str) -> Vec<Obj> {
    named["items"][name].as_array().unwrap().iter().map(|v| v.as_object().unwrap().clone()).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect()
}

/// Compare a result with a record that holds `output` or `error` (`message` and optional `code`).
fn check(got: Result<Value, CoreError>, record: &Value, context: &str) {
    match (got, record.get("error")) {
        (Ok(v), None) => common::assert_json(&v, &record["output"], &json!(context)),
        (Err(e), Some(err)) => {
            assert_eq!(json!(e.message), err["message"], "{context}");
            if let Some(code) = err.get("code") {
                assert_eq!(json!(e.code), *code, "{context}");
            }
        }
        (got, _) => panic!("ok={} vs {record} in {context}", got.is_ok()),
    }
}

fn ok<T: serde::Serialize>(v: T) -> Result<Value, CoreError> {
    Ok(serde_json::to_value(v).unwrap())
}

fn conv<T: serde::Serialize>(r: Result<T, CoreError>) -> Result<Value, CoreError> {
    r.map(|v| serde_json::to_value(v).unwrap())
}

#[test]
fn timecodes() {
    for c in common::cases("prompt/timecode.json") {
        check(conv(timecode(Some(&c["input"]["seconds"]))), &c, "timecode");
    }
}

#[test]
fn plans_and_cut_styles() {
    let n = named();
    for c in common::cases("prompt/cut_style_value.json") {
        assert_eq!(json!(cut_style_value(Some(&c["input"]["value"]))), c["output"], "{c}");
    }
    for c in common::cases("prompt/plans.json") {
        let p = plan(&n, c["input"]["plan"].as_str().unwrap());
        let style = &c["input"]["cut_style"];
        common::assert_json(&json!(shot_plan(&p)), &c["shot_plan"], &c);
        let applied = apply_cut_style(&p, Some(style));
        common::assert_json(&Value::Object(applied.clone()), &c["applied"], &c);
        assert_eq!(json!(cut_plan_instruction(&applied)), c["instruction"], "{c}");
    }
}

#[test]
fn craft_and_static_texts() {
    let n = named();
    for c in common::cases("prompt/performance_craft.json") {
        let i = &c["input"];
        let got = performance_craft_instruction(
            i["style"].as_str().unwrap(),
            i["has_cast"].as_bool().unwrap(),
            i["has_vocals"].as_bool().unwrap(),
            i["continuation"].as_bool().unwrap(),
        );
        assert_eq!(json!(got), c["output"], "{c}");
    }
    for c in common::cases("prompt/static_texts.json") {
        let i = &c["input"];
        let got = match i["fn"].as_str().unwrap() {
            "literal_scene_instruction" => literal_scene_instruction(),
            "scene_timing_instruction" => {
                scene_timing_instruction(i["duration"].as_f64().unwrap(), &plan(&n, i["plan"].as_str().unwrap()))
            }
            "opening_wrapper" => {
                opening_wrapper(i["style"].as_str().unwrap(), i["continuous"].as_bool().unwrap(), i["guidance"].as_str().unwrap())
            }
            "motion_energy_text" => motion_energy_text(i["camera"].as_f64().unwrap(), i["character"].as_f64().unwrap()),
            other => panic!("{other}"),
        };
        assert_eq!(json!(got), c["output"], "{c}");
    }
    for c in common::cases("prompt/life_movements.json") {
        let i = &c["input"];
        let got = life_movements(i["character_speed"].as_f64().unwrap(), i["seed"].as_str().unwrap());
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn description_cleanup() {
    for c in common::cases("prompt/description_cleanup.json") {
        let text = c["input"]["text"].as_str().unwrap();
        assert_eq!(json!(strip_negative_sentences(text)), c["strip_negative_sentences"], "{c}");
        assert_eq!(json!(normalize_description(text)), c["normalize_description"], "{c}");
        assert_eq!(json!(post_cut_text(text)), c["post_cut_text"], "{c}");
        assert_eq!(json!(continuous_cut_text(text, 2)), c["continuous_cut_text_2"], "{c}");
        assert_eq!(json!(continuous_cut_text(text, 5)), c["continuous_cut_text_5"], "{c}");
    }
}

#[test]
fn llm_json_parsing() {
    for c in common::cases("prompt/parse_shot_descriptions.json") {
        let i = &c["input"];
        let got = parse_shot_descriptions(i["raw"].as_str().unwrap(), i["expected"].as_u64().unwrap() as usize);
        check(conv(got), &c, &i["raw"].to_string());
    }
}

#[test]
fn assembly_budget_and_validation() {
    let n = named();
    for c in common::cases("prompt/assemble_and_validate.json") {
        let i = &c["input"];
        let descs = strings(&n["descriptions"][i["descriptions"].as_str().unwrap()]);
        let p = plan(&n, i["plan"].as_str().unwrap());
        let style = i["style"].as_str().unwrap();
        let guidance = i["guidance"].as_str().unwrap();
        check(conv(shot_body(&descs, &p)), &c["shot_body"], "shot_body");
        let assembled = assemble_prompt(&descs, &p, style, guidance);
        check(conv(assembled.clone()), &c["assemble_prompt"], "assemble_prompt");
        check(ok(character_budget(&p, style, 6500)), &c["character_budget"], "character_budget");
        if let (Ok(text), Some(record)) = (assembled, c.get("validate_prompt")) {
            check(conv(validate_prompt(&text, &p)), record, "validate_prompt");
        }
    }
    for c in common::cases("prompt/validate_prompt_edges.json") {
        let i = &c["input"];
        let p = plan(&n, i["plan"].as_str().unwrap());
        let got = validate_prompt(i["prompt"].as_str().unwrap(), &p);
        check(conv(got), &c, "validate_prompt_edges");
    }
}

#[test]
fn reference_frames() {
    let n = named();
    for c in common::cases("prompt/reference_frame.json") {
        let i = &c["input"];
        let frame = reference_frame(
            &items(&n, i["items"].as_str().unwrap()),
            &plan(&n, i["plan"].as_str().unwrap()),
            i["style"].as_str().unwrap(),
            i["audio_mode"].as_str().unwrap(),
            i["audio_direction"].as_str().unwrap(),
        );
        common::assert_json(&json!(frame), &c["frame"]["output"], &c);
        let wrapped = wrap_reference_prompt("detailed_description:\n[Shot 1] A shot.", &frame);
        assert_eq!(json!(wrapped), c["wrapped"], "{c}");
        assert_eq!(json!(wrap_reference_prompt(&wrapped, &frame)), c["wrapped_again"], "{c}");
    }
}

#[test]
fn reference_labels_and_bindings() {
    let n = named();
    for c in common::cases("prompt/reference_labels.json") {
        let i = &c["input"];
        let list = items(&n, i["items"].as_str().unwrap());
        let text = i["text"].as_str().unwrap();
        common::assert_json(&json!(reference_labels(&list)), &c["labels"]["output"], &c);
        assert_eq!(json!(character_reference_bindings(text, &list)), c["bindings"]["output"], "{c}");
        assert_eq!(json!(compact_reference_prompt(text, &list)), c["compact"]["output"], "{c}");
    }
    for c in common::cases("prompt/compact_description.json") {
        let i = &c["input"];
        let got = compact_description(Some(&i["text"]), i["limit"].as_u64().unwrap() as usize);
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn lyrics_and_last_shot() {
    for c in common::cases("prompt/lyrics.json") {
        let i = &c["input"];
        let lyrics = i["lyrics"].as_str().unwrap();
        if let Some(lines) = c.get("lines") {
            assert_eq!(json!(lyric_lines(lyrics)), *lines, "{c}");
            let chunks = lyric_chunks(lyrics, i["shots"].as_i64().unwrap());
            assert_eq!(json!(chunks), c["chunks"]["output"], "{c}");
        }
        let descs: Vec<String> = match i.get("descriptions") {
            Some(d) => strings(d),
            None => ["A wide shot.", "A close-up.", "A last shot."]
                .iter()
                .take(i["shots"].as_u64().unwrap() as usize)
                .map(|s| s.to_string())
                .collect(),
        };
        let performer = i.get("performer").and_then(Value::as_str).unwrap_or("the singer");
        assert_eq!(json!(ensure_quoted_lyrics(&descs, lyrics, performer)), c["quoted"]["output"], "{c}");
    }
    for c in common::cases("prompt/last_shot_text.json") {
        let i = &c["input"];
        let got = last_shot_text(i["saved"].as_str().unwrap(), i["limit"].as_u64().unwrap() as usize);
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn timed_cues() {
    let n = named();
    for c in common::cases("prompt/cues.json") {
        let segment = n["cue_segments"][c["input"]["segment"].as_str().unwrap()].as_object().unwrap().clone();
        let rows = timed_cue_rows(&segment);
        common::assert_json(&json!(rows), &c["rows"], &c);
        if rows.is_empty() {
            continue;
        }
        let labeled: Vec<CueRow> = rows
            .iter()
            .map(|r| CueRow {
                label: Some(if r.kind == "vocal" { "<Subject 1> (Mara)".into() } else { String::new() }),
                ..r.clone()
            })
            .collect();
        assert_eq!(json!(cue_shot_contract(&labeled)), c["contract"], "{c}");
        let base = json!({"exact_duration_seconds": 6.5, "cut_times_seconds": [], "frequency": 0}).as_object().unwrap().clone();
        common::assert_json(&Value::Object(cue_cut_plan(&base, &rows)), &c["cut_plan"], &c);
        let mut expected = c["enforced"].as_array().unwrap().iter();
        for row in &labeled {
            for text in [
                "A wide shot of the singer on stage, singing the line.",
                "<Subject 1> sings <d>[English, tender, wistful] <i>hold</i> on tight</d> softly.",
            ] {
                let record = expected.next().unwrap();
                assert_eq!(json!(enforce_cue_shot(text, row)), record["output"], "{c}");
            }
        }
    }
    for c in common::cases("prompt/cue_header.json") {
        assert_eq!(json!(cue_header(Some(&c["input"]["header"]))), c["output"], "{c}");
    }
    for c in common::cases("prompt/cue_delivery.json") {
        let i = &c["input"];
        let got = cue_delivery(i["cue_text"].as_str().unwrap(), i["llm_body"].as_str().unwrap());
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn location_and_continuation_text() {
    let n = named();
    let place = |name: &str| n["places"][name].as_object().cloned();
    for c in common::cases("prompt/location_contract.json") {
        let i = &c["input"];
        let got = location_continuity_contract(
            place(i["current"].as_str().unwrap()).as_ref(),
            place(i["previous"].as_str().unwrap()).as_ref(),
            i["preset"].as_str().unwrap(),
            i["custom"].as_str().unwrap(),
            i["hold"].as_f64().unwrap(),
            i["has_vocals"].as_bool().unwrap(),
        );
        assert_eq!(json!(got), c["output"], "{c}");
    }
    for c in common::cases("prompt/performance_line.json") {
        let i = &c["input"];
        let got = performance_line(i["has_vocals"].as_bool().unwrap(), i["part"].as_str().unwrap());
        assert_eq!(json!(got), c["output"], "{c}");
    }
    for c in common::cases("prompt/continuation_task.json") {
        let i = &c["input"];
        let continuation = n["continuations"][i["continuation"].as_str().unwrap()].as_object().unwrap().clone();
        let got = continuation_task_text(
            &continuation,
            i["previous_shot"].as_str().unwrap(),
            i["has_vocals"].as_bool().unwrap(),
            i["with_picture"].as_bool().unwrap(),
            i["location_contract"].as_str().unwrap(),
        );
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn shot_task_text() {
    let n = named();
    for c in common::cases("prompt/build_shot_task.json") {
        let mut merged = n["build_shot_task_base"].as_object().unwrap().clone();
        for (k, v) in c["input"]["kwargs_extra"].as_object().unwrap() {
            merged.insert(k.clone(), v.clone());
        }
        let args: ShotTaskArgs = serde_json::from_value(Value::Object(merged)).unwrap_or_else(|e| panic!("{e}: {c}"));
        assert_eq!(json!(build_shot_task(&args)), c["output"], "{}", c["input"]["variant"]);
    }
}
