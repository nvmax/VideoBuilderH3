//! What a Story Mode plan does to the session: each scene's performance, the masked continuations between scenes, and the cast of each scene.

use h3_core::pyval::Obj;
use h3_core::story::apply::{apply_continuations_to_segments, apply_plan_to_segments, fill_cast_from_acts, story_mode_edit_fields};
use serde_json::{json, Value};

fn session() -> Obj {
    json!({
        "video_engine": "minimax_h3",
        "minimax_h3_settings": {"video_mode": "reference_to_video", "render_pass": "single", "continuity_mode": "off"},
        "segments": [
            {"id": "a", "start": 0.0, "end": 4.0},
            {"id": "b", "start": 4.0, "end": 8.0},
            {"id": "c", "start": 8.0, "end": 12.0}
        ],
        "flux_reference_builder": {"subjects": [{"id": "ava", "name": "Ava"}, {"id": "mara", "name": "Mara"}]}
    })
    .as_object()
    .cloned()
    .unwrap()
}

fn scene<'a>(session: &'a Obj, id: &str) -> &'a Value {
    session["segments"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap()
}

#[test]
fn a_plan_sets_each_scenes_performance_and_reports_only_what_changed() {
    let mut session = session();
    let plan = json!([
        {"scene_id": "a", "performance_mode": "singing"},
        {"scene_id": "b", "performance_mode": "instrumental"},
        {"scene_id": "nobody", "performance_mode": "speaking"}
    ]);
    assert_eq!(apply_plan_to_segments(&mut session, &plan), ["a", "b"]);
    assert_eq!((scene(&session, "a")["performance_mode"].as_str(), scene(&session, "a")["lyric_no_lip_sync"].as_bool()), (Some("singing"), Some(false)));
    assert_eq!((scene(&session, "b")["performance_mode"].as_str(), scene(&session, "b")["lyric_no_lip_sync"].as_bool()), (Some("no_lip_sync"), Some(true)));
    assert!(scene(&session, "c").get("performance_mode").is_none());
    assert!(apply_plan_to_segments(&mut session, &plan).is_empty(), "a second run changes nothing");
}

#[test]
fn a_continued_scene_is_a_masked_continuation_until_the_plan_stops_continuing_it() {
    let mut session = session();
    let continued = json!([
        {"scene_id": "a", "continues_previous": true, "continuation_direction": "ignored for the first scene"},
        {"scene_id": "b", "continues_previous": true, "continuation_direction": "She keeps walking."},
        {"scene_id": "c"}
    ]);
    assert_eq!(apply_continuations_to_segments(&mut session, &continued), ["b"], "the first scene never continues");
    let b = scene(&session, "b");
    assert_eq!(b["use_scene_minimax_h3_settings"], true);
    assert_eq!(b["minimax_h3_settings"]["continuity_mode"], "latent_continuation_masked");
    assert_eq!(b["minimax_h3_settings"]["location_transition_preset"], "masked");
    assert_eq!(b["minimax_h3_settings"]["video_mode"], "reference_to_video");
    assert_eq!(b["minimax_h3_continuation_direction"], "She keeps walking.");
    assert_eq!(b["story_masked_continuation"], true);
    assert!(scene(&session, "a").get("minimax_h3_settings").is_none());
    assert!(apply_continuations_to_segments(&mut session, &continued).is_empty(), "a second run changes nothing");

    // The plan lets go of the scene: it goes back to the project's settings.
    let alone = json!([{"scene_id": "b", "continues_previous": false}]);
    assert_eq!(apply_continuations_to_segments(&mut session, &alone), ["b"]);
    let b = scene(&session, "b");
    assert_eq!(b["use_scene_minimax_h3_settings"], false);
    assert!(b.get("minimax_h3_settings").is_none() && b.get("story_masked_continuation").is_none(), "{b}");
    assert_eq!(b["minimax_h3_continuation_direction"], "");
}

#[test]
fn only_a_minimax_project_gets_continuations() {
    let mut session = session();
    session.insert("video_engine".into(), json!("ltx"));
    assert!(apply_continuations_to_segments(&mut session, &json!([{"scene_id": "b", "continues_previous": true}])).is_empty());
    assert!(scene(&session, "b").get("minimax_h3_settings").is_none());
}

#[test]
fn a_plans_cast_is_mapped_onto_its_scenes() {
    let mut session = session();
    let acts = json!([{"id": "act_1", "chapter_id": "ch_1", "cast_ids": ["ava"], "scene_ids": ["a", "b", "c"]}]);
    let plan = json!([{"scene_id": "b", "cast_ids": ["mara"]}]);
    let mut changed = fill_cast_from_acts(&mut session, &acts, &plan);
    changed.sort();
    assert_eq!(changed, ["a", "b", "c"]);
    let map = &session["flux_reference_builder"]["subject_scene_map"];
    assert_eq!((map["a"].clone(), map["b"].clone()), (json!(["ava"]), json!(["mara"])), "the plan's cast wins, the act's cast fills the rest");
    assert_eq!(session["flux_reference_builder"]["use_subject_reference"], true);
}

#[test]
fn story_mode_fields_from_an_agent_are_checked_and_the_continuations_follow_the_mode() {
    let session = session();
    let layer = Obj::new();
    let book = "Title: T\n\nAct 1: The Walk\nSetting: Main Street\n\nAva walks.\n\nShe stops.";
    let given = |value: Value| value.as_object().cloned().unwrap();
    let fields = story_mode_edit_fields(&session, &layer, &given(json!({"story_book_text": book}))).unwrap();
    assert_eq!((fields["story_flow_version"].as_i64(), fields["story_book"]["chapters"].as_array().map(Vec::len)), (Some(2), Some(1)));

    // The chapter of the book is the one the acts must belong to, and the acts must cover every scene.
    let mut with_book = Obj::new();
    with_book.extend(fields);
    let act = |scenes: Value| json!([{"id": "act_1", "chapter_id": "ch_1", "scene_ids": scenes}]);
    let bad = story_mode_edit_fields(&session, &with_book, &given(json!({"story_acts": act(json!(["a", "b"]))})));
    assert!(bad.as_ref().is_err_and(|m| m.starts_with("story_acts are not valid")), "{bad:?}");
    assert!(story_mode_edit_fields(&session, &with_book, &given(json!({"story_acts": act(json!(["a", "b", "c"]))}))).is_ok());

    // `location` continues a scene that touches the one before it at the same spot in the same chapter.
    let acts = act(json!(["a", "b", "c"]));
    let plan = json!([
        {"scene_id": "a", "act_id": "act_1", "spot": "Street"},
        {"scene_id": "b", "act_id": "act_1", "spot": "Street"},
        {"scene_id": "c", "act_id": "act_1", "spot": "Garden"}
    ]);
    let mut layer = with_book.clone();
    layer.insert("story_acts".into(), acts);
    let resolved = story_mode_edit_fields(&session, &layer, &given(json!({"story_scene_plan": plan, "story_continuation_mode": "location"}))).unwrap();
    let continues: Vec<bool> = resolved["story_scene_plan"].as_array().unwrap().iter().map(|e| e["continues_previous"].as_bool().unwrap()).collect();
    assert_eq!(continues, [false, true, false]);
}
