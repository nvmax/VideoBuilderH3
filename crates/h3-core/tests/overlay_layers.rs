//! Overlay data against `fixtures/cases/overlay_layer_cases.json`, the pack's own case file (copied by `tools/make_all.py`). The pack's JS twin
//! (`overlay_layers.mjs`) produced the expected values and they were checked by hand, so this port has to give the same ones.

mod common;

use common::{assert_json, load};
use h3_core::overlay_layers::{normalize_overlay_layers, normalize_overlay_layer_track, shift_overlay_layers, KINDS, MAX_CLIPS};
use serde_json::{json, Value};

fn run(function: &str, raw: &Value) -> Value {
    match function {
        "normalize_overlay_layers" => Value::Array(normalize_overlay_layers(raw).into_iter().map(Value::Object).collect()),
        "normalize_overlay_layer_track" => Value::Object(normalize_overlay_layer_track(raw)),
        other => panic!("unknown function {other}"),
    }
}

#[test]
fn every_shared_case_gives_the_recorded_result() {
    let cases = load("cases/overlay_layer_cases.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 30);
    for case in cases {
        let got = run(case["fn"].as_str().unwrap(), &case["raw"]);
        eprintln!("case: {}", case["name"]);
        assert_json(&got, &case["return"], &case["name"]);
    }
}

#[test]
fn normalizing_a_result_again_changes_nothing() {
    let cases = load("cases/overlay_layer_cases.json");
    for case in cases.as_array().unwrap() {
        let function = case["fn"].as_str().unwrap();
        let once = run(function, &case["raw"]);
        let twice = run(function, &once);
        assert_json(&twice, &once, &case["name"]);
    }
}

#[test]
fn a_list_longer_than_the_limit_is_cut_in_order() {
    let many: Vec<Value> = (0..MAX_CLIPS + 2).map(|i| json!({"kind": "grain", "id": format!("g{i}")})).collect();
    let got = normalize_overlay_layers(&Value::Array(many));
    assert_eq!(got.len(), MAX_CLIPS);
    assert_eq!(got[0]["id"], "g0");
    assert_eq!(got[MAX_CLIPS - 1]["id"], format!("g{}", MAX_CLIPS - 1));
}

#[test]
fn every_known_kind_ends_after_it_starts_and_its_fades_fit() {
    for kind in KINDS {
        let got = normalize_overlay_layers(&json!([{"kind": kind, "start": 5, "end": 1}]));
        let clip = &got[0];
        let (start, end) = (clip["start"].as_f64().unwrap(), clip["end"].as_f64().unwrap());
        assert!(end > start, "{kind}");
        assert!(clip["fade_in"].as_f64().unwrap() + clip["fade_out"].as_f64().unwrap() <= end - start + 1e-9, "{kind}");
    }
}

#[test]
fn numeric_text_is_read_like_javascript_reads_it() {
    let read = |text: &str| normalize_overlay_layers(&json!([{"kind": "grain", "strength": text}]))[0]["strength"].as_f64().unwrap();
    assert_eq!(read("42.5"), 42.5);
    assert_eq!(read(" 7 "), 7.0);
    assert_eq!(read("+7."), 7.0);
    assert_eq!(read(".5"), 0.5);
    assert_eq!(read("1e1"), 10.0);
    for refused in ["", "abc", "0x10", "1e", "e1", "--1", "1 2", "inf", "NaN", "1e999", "."] {
        assert_eq!(read(refused), 28.0, "{refused:?}");
    }
}

#[test]
fn clips_move_back_when_the_video_starts_part_way_into_the_timeline() {
    let clips = json!([
        {"kind": "grain", "id": "before", "start": 1, "end": 4},
        {"kind": "vignette", "id": "across", "start": 4, "end": 12, "fade_in": 1, "fade_out": 1},
        {"kind": "grain", "id": "inside", "start": 8, "end": 9, "fade_in": 0.2},
        {"kind": "text", "text": "x"},
        "not a clip"
    ]);
    // A video that starts 5 seconds into the timeline.
    let got = shift_overlay_layers(&clips, 5.0);
    let got = got.as_array().unwrap();
    assert_eq!(got.len(), 4, "the clip that ended before the start is dropped, the odd entries are left for the normalizer");
    assert_eq!((got[0]["id"].as_str(), got[0]["start"].as_f64(), got[0]["end"].as_f64(), got[0]["fade_in"].as_f64()), (Some("across"), Some(0.0), Some(7.0), Some(0.0)));
    assert_eq!(got[0]["fade_out"], 1, "the fade out is untouched");
    assert_eq!((got[1]["id"].as_str(), got[1]["start"].as_f64(), got[1]["end"].as_f64(), got[1]["fade_in"].as_f64()), (Some("inside"), Some(3.0), Some(4.0), Some(0.2)));
    // No offset, a bad offset and a list that is not a list change nothing or give nothing.
    assert_eq!(shift_overlay_layers(&clips, 0.0), clips);
    assert_eq!(shift_overlay_layers(&clips, f64::NAN), clips);
    assert_eq!(shift_overlay_layers(&json!("x"), 3.0), json!([]));
    // What is left normalizes as usual.
    assert_eq!(normalize_overlay_layers(&Value::Array(got.clone())).len(), 3);
}

#[test]
fn a_media_clip_that_began_before_the_video_starts_is_part_way_through_its_file() {
    let clips = json!([
        {"kind": "media", "id": "dust", "start": 2, "end": 20, "speed": 2, "source_start": 1},
        {"kind": "media", "id": "plain", "start": 3, "end": 20},
        {"kind": "media", "id": "later", "start": 8, "end": 20, "source_start": 4},
        {"kind": "grain", "id": "grain", "start": 1, "end": 20}
    ]);
    // A video that starts 5 s into the timeline.
    let got = shift_overlay_layers(&clips, 5.0);
    let got = got.as_array().unwrap();
    assert_eq!(got[0]["source_start"].as_f64(), Some(1.0 + 3.0 * 2.0), "3 s at double speed on top of the 1 s it began at");
    assert_eq!(got[1]["source_start"].as_f64(), Some(2.0), "a clip with no source start began at 0 and is 2 s in");
    assert_eq!(got[2]["source_start"], 4, "a clip that starts after the video starts is untouched");
    assert!(got[3].get("source_start").is_none(), "only media clips have a place in a file");
}

#[test]
fn lyric_captions_come_from_the_word_timed_lines_and_never_overlap() {
    use h3_core::overlay_layers::{caption_clips, caption_lines_from_scenes, caption_lines_from_timestamped, text_style, CaptionLine};
    let document = json!({"segments": [
        {"type": "instrumental", "start": 0, "end": 6.3, "text": "[instrumental]", "words": []},
        {"type": "vocal", "start": 6.34, "end": 8.82, "text": "Don\u{2019}t get it twisted.", "words": [{"start": 6.34, "end": 6.44, "text": "Don\u{2019}t"}]},
        {"type": "vocal", "start": 8.5, "end": 11.54, "text": "  I got   the\nkeys  ", "words": []},
        {"type": "vocal", "start": 12.0, "end": 12.1, "text": "tiny", "words": []},
        {"type": "vocal", "start": 13.0, "end": 14.0, "text": "   ", "words": []},
        {"type": "vocal", "start": 15.0, "end": 14.0, "text": "backwards", "words": []},
        {"type": "vocal", "start": 16.0, "end": 19.0, "text": "Last line"}
    ]});
    let lines = caption_lines_from_timestamped(&document);
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Don\u{2019}t get it twisted.", "I got the keys", "Last line"], "instrumental, too short, blank and backwards segments are left out and spaces are tidied");
    assert_eq!((lines[0].start, lines[0].end), (6.34, 8.5), "a line that ends after the next one starts is cut there");
    let scenes = caption_lines_from_scenes(&json!({"segments": [
        {"start": 0, "end": 4, "lyric_text": "First  line", "lyric_instrumental": false},
        {"start": 4, "end": 8, "lyric_text": "skipped", "lyric_instrumental": true},
        {"start": 8, "end": 12, "lyric_text": ""},
        {"start": 12, "end": 16, "lyric_text": "Last"}
    ]}));
    assert_eq!(scenes, vec![CaptionLine { start: 0.0, end: 4.0, text: "First line".into() }, CaptionLine { start: 12.0, end: 16.0, text: "Last".into() }]);
    let clips = caption_clips(&lines, "bold", None);
    assert_eq!(clips.len(), 3);
    assert_eq!((clips[0]["id"].as_str(), clips[0]["kind"].as_str(), clips[0]["font"].as_str(), clips[0]["group"].as_str(), clips[0]["locked"].as_bool()), (Some("caption_1"), Some("text"), Some("Anton"), Some("lyric_captions"), Some(false)));
    assert_eq!(caption_clips(&lines, "bold", Some((10.0, 17.0))).iter().map(|c| c["text"].as_str().unwrap()).collect::<Vec<_>>(), ["I got the keys", "Last line"], "only the lines inside the range");
    assert_eq!(text_style("nonsense"), text_style("clean"));
    assert_ne!(text_style("lower"), text_style("clean"));
    // What comes out is a set of clips the normalizer accepts as they are.
    let normalized = normalize_overlay_layers(&Value::Array(clips.clone()));
    assert_eq!(normalized.len(), 3);
    assert_eq!(normalized[1]["text"], "I got the keys");
    assert_eq!(normalized[1]["group"], "lyric_captions", "the group survives the normalizer");
}
