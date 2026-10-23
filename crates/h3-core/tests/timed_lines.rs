//! Scenes from timed lyric lines against `fixtures/timeline/timed_lines.json`, recorded from the pack.

mod common;

use common::load;
use h3_core::timed_lines::{build_scenes_from_payload, clean_timestamped_lyric_text, parse_timestamped_lyrics_output, BuildParams};
use serde_json::{json, Value};

const REFERENCE: &str = "[Verse 1]\nWalking down the empty street\nLights are fading under my feet\n\n[Chorus]\nSing it loud tonight\nHold the morning light\n\n[Bridge]\nQuiet now\n[End]";

fn masked(scenes: Vec<Value>) -> Vec<Value> {
    scenes
        .into_iter()
        .map(|mut s| {
            s["id"] = json!("ID");
            s
        })
        .collect()
}

/// The same payloads the generator built.
fn payload(name: &str) -> Value {
    let words = |text: &str, start: f64, end: f64| -> Vec<Value> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let step = (end - start) / parts.len().max(1) as f64;
        parts.iter().enumerate().map(|(i, w)| json!({"text": w, "start": round3(start + i as f64 * step), "end": round3(start + (i + 1) as f64 * step - 0.02)})).collect()
    };
    let line = |text: &str, start: f64, end: f64, kind: &str, with_words: bool| -> Value {
        let mut item = json!({"text": text, "start": start, "end": end, "type": kind});
        if with_words {
            item["words"] = Value::Array(words(text, start, end));
        }
        item
    };
    let v = |t: &str, s: f64, e: f64| line(t, s, e, "vocal", true);
    match name {
        "plain" => json!({"duration": 40.0, "segments": [v("Walking down the empty street", 2.0, 6.0), v("Lights are fading under my feet", 6.2, 10.0), v("Sing it loud tonight", 14.0, 18.5), v("Hold the morning light", 18.5, 23.0), v("Quiet now", 30.0, 32.0)]}),
        "long_chunk" => json!({"duration": 30.0, "segments": [v("one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen", 1.0, 25.0)]}),
        "long_no_words" => json!({"duration": 26.0, "segments": [line("a very long line without word timing at all", 0.0, 23.0, "vocal", false)]}),
        "tiny_lines" => json!({"duration": 12.0, "segments": [v("Hey", 0.5, 1.0), v("Oh", 1.2, 1.5), v("Walking down the empty street", 2.0, 6.0), v("Yeah", 6.1, 6.4), v("Lights are fading under my feet", 8.0, 12.0)]}),
        "unsorted_overlap" => json!({"duration": 20.0, "segments": [v("Sing it loud tonight", 8.0, 12.0), v("Walking down the empty street", 1.0, 5.0), v("Hold the morning light", 11.0, 16.0), {"text": "bad", "start": 3.0, "end": 3.0}, "junk"]}),
        "instrumental_and_brackets" => json!({"duration": 25.0, "segments": [v("[Verse 1] Walking down the empty street", 0.0, 5.0), line("", 5.0, 9.0, "instrumental", false), line("[Instrumental]", 9.0, 12.0, "vocal", false), v("Sing it loud tonight", 12.0, 17.0)]}),
        "bundled" => json!({"duration": 12.0, "segments": [v("Walking down the empty street Lights are fading under my feet", 0.0, 6.0), v("Sing it loud tonight", 6.0, 12.0)]}),
        "empty" => json!({"duration": 10.0, "segments": []}),
        "only_duration" => json!({"segments": [v("Walking down the empty street", 0.0, 4.0)]}),
        other => panic!("unknown payload {other}"),
    }
}

fn round3(x: f64) -> f64 {
    h3_core::pyval::round_to(x, 3)
}

fn params_from(p: &Value) -> BuildParams {
    BuildParams {
        segment_mode: p["segment_mode"].as_str().unwrap().into(),
        instrumental_text: p["instrumental_text"].as_str().unwrap().into(),
        min_scene_seconds: p["min_scene_seconds"].as_f64().unwrap(),
        max_scene_seconds: p["max_scene_seconds"].as_f64().unwrap(),
        min_gap_seconds: p["min_gap_seconds"].as_f64().unwrap(),
        include_instrumental_gaps: p["include_instrumental_gaps"].as_bool().unwrap(),
        enforce_lengths: p["enforce_lengths"].as_bool().unwrap(),
    }
}

#[test]
fn timed_lines_match_the_pack() {
    let cases = load("timeline/timed_lines.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 400);
    let mut built = 0;
    for (n, case) in cases.iter().enumerate() {
        match case["op"].as_str().unwrap() {
            "build" => {
                let payload = payload(case["payload"].as_str().unwrap());
                let reference = case.get("reference").and_then(Value::as_str).unwrap_or(REFERENCE);
                let mut counter = 0;
                let mut new_id = || {
                    counter += 1;
                    format!("id{counter}")
                };
                let got = build_scenes_from_payload(payload.as_object().unwrap(), &params_from(&case["params"]), reference, &mut new_id);
                let label = format!("case {n}: {} {}", case["payload"], case["params"]);
                match (case["return"].get("scenes"), case["return"].get("error")) {
                    (Some(expected), _) => {
                        let scenes = masked(got.unwrap_or_else(|e| panic!("{label}: {e}")));
                        assert_eq!(&Value::Array(scenes), expected, "{label}");
                        built += 1;
                    }
                    (_, Some(error)) => assert_eq!(got.unwrap_err(), error.as_str().unwrap(), "{label}"),
                    _ => panic!("{label}: no result"),
                }
            }
            "clean" => assert_eq!(clean_timestamped_lyric_text(case["text"].as_str().unwrap()), case["return"].as_str().unwrap(), "case {n}: {}", case["text"]),
            "parse" => {
                let got = parse_timestamped_lyrics_output(case["text"].as_str().unwrap());
                match (case["return"].get("value"), case["return"].get("error")) {
                    (Some(value), _) => assert_eq!(&Value::Object(got.unwrap()), value, "case {n}"),
                    (_, Some(error)) => assert_eq!(got.unwrap_err(), error.as_str().unwrap(), "case {n}"),
                    _ => panic!("case {n}"),
                }
            }
            other => panic!("unknown op {other}"),
        }
    }
    assert!(built > 200, "{built} scene lists compared");
}
