//! The story steps an agent runs through the API: the story arc, the story brief, the scene beats and the Story Mode steps (book, chapter, split, convert,
//! cast, remove people, acts, scene plan, places). Port of `agent_api/orchestrator/storyboard_orchestrator.py` and `story_mode_orchestrator.py`.
//!
//! The steps themselves are the sibling modules, the same functions the Story Builder's buttons call. This module builds their request from the saved
//! project, runs them on the LLM the app is set to use, and saves what they return where the Builder keeps it (`builder_story_layer`, the scenes'
//! `story_beat`, the Reference Builder's location cards and scene maps). The Storyboard Builder's own copy of the project (`storyboard.json`) is not
//! written, because this app has none.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use h3_core::fresh_context::fresh_prompt_scene_context;
use h3_core::lyric_scenes::is_instrumental_lyric_text;
use h3_core::pyval::{text_or_empty, to_float, to_int, truthy, Obj};
use h3_core::scene_card::VIDEO_PROMPT_TYPES;
use h3_core::scene_defaults::normalize_scene_defaults;
use h3_core::story::apply::{apply_continuations_to_segments, apply_plan_to_segments, fill_cast_from_acts};
use h3_core::story::helpers::refmod_card_fields;
use h3_core::story::locations::apply_story_locations;
use h3_llm::prompts::text as constant;
use h3_project::scene_ops::OpError;
use h3_project::story::story_layer;
use serde_json::{json, Value};

use super::arc::{build_story_layer_arc, build_story_layer_brief, ArcError};
use super::beat::build_scene_beat;
use super::book;
use super::places::build_story_locations;
use super::plan::{build_story_acts, build_story_scene_plan};
use crate::prompt_writers::Llm;

/// The steps, in the order a story is written. `arc`, `brief` and `beats` are the song-section story. The rest are Story Mode.
pub const STEPS: [&str; 12] = ["arc", "brief", "beats", "book", "chapter", "split", "convert", "cast", "remove_people", "acts", "plan", "locations"];

/// Why a step did not finish.
#[derive(Debug)]
pub enum StepFailure {
    /// What was asked cannot be done with this project or these settings.
    Invalid(String),
    /// The step ran and failed, usually the LLM.
    Failed(String),
    Op(OpError),
}

impl From<OpError> for StepFailure {
    fn from(error: OpError) -> Self {
        StepFailure::Op(error)
    }
}

pub type StepResult = Result<Value, StepFailure>;

fn invalid<T>(message: impl Into<String>) -> Result<T, StepFailure> {
    Err(StepFailure::Invalid(message.into()))
}

/// A change to the saved project: it gets the session and answers with what the step reports.
pub type Change = Box<dyn FnOnce(&mut Obj) -> Result<Value, OpError> + Send>;
pub type SaveFuture<'a> = Pin<Box<dyn Future<Output = Result<(Value, i64), StepFailure>> + Send + 'a>>;

/// Where a step reads the project and saves its work. The API saves under the edit lock.
pub trait Store: Sync {
    fn folder(&self) -> &Path;
    /// The saved project as it is now.
    fn read(&self) -> Result<Obj, StepFailure>;
    /// Apply `change` to the saved project and save it. The answer is what `change` returned and the new revision.
    fn save(&self, change: Change) -> SaveFuture<'_>;
}

/// Tells how far a step is: `(done, total, label)`.
pub type Progress<'a> = &'a (dyn Fn(usize, usize, &str) + Sync);

// -- the project as the steps read it -----------------------------------------------------------------------------------------

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

fn dict(value: Option<&Value>) -> Obj {
    value.and_then(Value::as_object).cloned().unwrap_or_default()
}

/// `a ?? b`: the first key whose value is not missing or null.
fn first_present<'a>(map: &'a Obj, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().filter_map(|k| map.get(*k)).find(|v| !v.is_null())
}

/// `str(a or b or c).strip()`: the text of the first key that has any.
fn first_text(map: &Obj, keys: &[&str]) -> String {
    keys.iter().filter_map(|k| map.get(*k)).find(|v| truthy(v)).map(|v| text(Some(v))).unwrap_or_default()
}

/// `a or b` for text.
fn either(own: String, fallback: String) -> String {
    if own.is_empty() { fallback } else { own }
}

fn number(map: &Obj, key: &str) -> f64 {
    map.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

fn count(params: &Obj, key: &str, default: i64) -> i64 {
    params.get(key).filter(|v| truthy(v)).and_then(to_int).unwrap_or(default)
}

fn id_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(|item| text(Some(item))).filter(|id| !id.is_empty()).collect(),
        other => text(other).split(',').map(|part| part.trim().to_string()).filter(|part| !part.is_empty()).collect(),
    }
}

fn is_minimax(session: &Obj) -> bool {
    text(session.get("video_engine")).to_lowercase() == "minimax_h3"
}

/// The picture a card shows, as the Storyboard sends it: no file data, just the path and name. A RefMod card keeps its RefMod, so the
/// Storyboard shows its preview and labels it (`refmod_card_fields`).
fn slim_reference(card: Option<&Value>) -> Value {
    let Some(card) = card.and_then(Value::as_object) else { return Value::Null };
    let image = dict(card.get("image"));
    let mut slim = refmod_card_fields(card);
    for (key, value) in [
        ("id", json!(text(card.get("id")))),
        ("name", json!(text(card.get("name")))),
        ("description", json!(text(card.get("description")))),
        ("minimax_voice", if card.get("minimax_voice").is_some_and(Value::is_object) { card["minimax_voice"].clone() } else { json!({}) }),
        ("trigger_phrase", json!(text(card.get("trigger_phrase")))),
        ("trigger_position", json!(if text(card.get("trigger_position")) == "end" { "end" } else { "start" })),
        ("image", json!({"path": text(image.get("path")), "name": text(image.get("name")), "data": ""})),
    ] {
        slim.insert(key.to_string(), value);
    }
    Value::Object(slim)
}

/// The image prompt a card shows for the project's image model.
fn image_prompt_of(segment: &Obj, mode: &str) -> String {
    let own = match mode {
        "flow_gpt" => "flow_gpt_prompt",
        "flux_klein" => "flux_klein_prompt",
        "nano_banana" => "nb_prompt",
        "ernie_image" => "ernie_t2i_prompt",
        _ => "",
    };
    let order = [own, "flow_gpt_prompt", "t2i_prompt", "flux_klein_prompt", "nb_prompt", "ernie_t2i_prompt"];
    order.iter().filter(|k| !k.is_empty()).map(|k| text(segment.get(*k))).find(|t| !t.is_empty()).unwrap_or_default()
}

/// No-lip-sync scenes, else the project's video type. Story Mode scenes choose their own.
fn performance_mode(session: &Obj, segment: &Obj) -> String {
    if segment.get("lyric_no_lip_sync").is_some_and(truthy) {
        return "no_lip_sync".into();
    }
    let raw = first_text(session, &["video_type", "videoType"]).to_lowercase().replace(['-', ' '], "_");
    let raw = if matches!(raw.as_str(), "story" | "story_mode") {
        let own = text(segment.get("performance_mode")).to_lowercase();
        if own.is_empty() { "singing".to_string() } else { own }
    } else {
        raw
    };
    if matches!(raw.as_str(), "speaking" | "short_film" | "dialogue" | "dialog") { "speaking".into() } else { "singing".into() }
}

/// One Storyboard scene card per timeline scene, in timeline order, with every editable field. Twin of `storyboardScenePayload` in
/// `web/music_video_builder/scene_output.mjs`.
pub fn scene_cards(session: &Obj) -> Vec<Value> {
    let refs = dict(session.get("flux_reference_builder"));
    let by_id = |key: &str| -> std::collections::HashMap<String, Value> {
        refs.get(key).and_then(Value::as_array).into_iter().flatten().filter(|c| c.is_object()).map(|c| (text(c.get("id")), c.clone())).collect()
    };
    let (subjects, locations) = (by_id("subjects"), by_id("locations"));
    let (subject_map, location_map) = (dict(refs.get("subject_scene_map")), dict(refs.get("scene_map")));
    let locations_cleared = refs.get("locations_cleared").is_some_and(truthy);
    let project_defaults = dict(session.get("builder_storyboard_defaults"));
    let minimax = is_minimax(session);
    let engine = if minimax { "minimax_h3" } else { "ltx" };
    let image_mode = {
        let given = session.get("image_model_mode").filter(|v| truthy(v)).cloned().or_else(|| session.get("flux_klein_settings").and_then(|s| s.get("image_model_mode")).cloned());
        Some(text(given.as_ref())).filter(|m| !m.is_empty()).unwrap_or_else(|| "zimage".into())
    };
    let mut segments: Vec<Obj> = session.get("segments").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object).cloned().collect();
    segments.sort_by(|a, b| number(a, "start").total_cmp(&number(b, "start")));
    let mut cards = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let scoped = normalize_scene_defaults(segment.get("scene_defaults").unwrap_or(&Value::Null), false).unwrap_or_default();
        let mut defaults = project_defaults.clone();
        defaults.extend(scoped.clone());
        let no_character = segment.get("no_character_present").is_some_and(truthy);
        let scene_key = text(segment.get("id"));
        let mapped: Vec<Value> = if no_character {
            Vec::new()
        } else {
            id_list(subject_map.get(&scene_key)).iter().filter_map(|id| subjects.get(id)).map(|card| slim_reference(Some(card))).collect()
        };
        let location = if locations_cleared { Value::Null } else { slim_reference(locations.get(&text(location_map.get(&scene_key)))) };
        let (start, end) = (number(segment, "start"), number(segment, "end"));
        let lyric = first_text(segment, &["lyric_text", "lyric_note", "lyrics"]);
        let video_notes = text(first_present(segment, &["i2v_notes", "video_notes"]));
        let scene_notes = text(first_present(segment, &["notes", "director_note"]));
        let explicit_singers: Vec<String> = segment.get("lyric_singers").and_then(Value::as_array).into_iter().flatten().map(|n| text(Some(n))).filter(|n| !n.is_empty()).collect();
        let singers: Vec<String> = if no_character {
            Vec::new()
        } else if !explicit_singers.is_empty() {
            explicit_singers
        } else {
            mapped.iter().map(|r| text(r.get("name"))).filter(|n| !n.is_empty()).collect()
        };
        let raw_prompt = if minimax { text(segment.get("minimax_h3_prompt")) } else { first_text(segment, &["i2v_prompt", "t2v_prompt"]) };
        let origin = text(segment.get(if minimax { "minimax_h3_prompt_origin" } else { "i2v_prompt_origin" })).to_lowercase();
        let stored_type = text(segment.get("video_prompt_type"));
        let no_lip_sync = segment.get("lyric_no_lip_sync").is_some_and(truthy);
        let summary = [scene_notes.clone(), text(segment.get("timeline_note")), video_notes.clone(), text(location.get("description")), if no_lip_sync { String::new() } else { lyric.clone() }]
            .into_iter()
            .find(|part| !part.is_empty())
            .unwrap_or_default();
        let label = Some(text(segment.get("label"))).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("Scene {}", index + 1));
        let setting = Some(text(location.get("description"))).filter(|d| !d.is_empty()).unwrap_or_else(|| text(location.get("name")));
        let pick = |key: &str| text(segment.get(key));
        let list = |key: &str| if segment.get(key).is_some_and(Value::is_array) { segment[key].clone() } else { json!([]) };
        let mut card = Obj::new();
        for (key, value) in [
            ("id", json!(if truthy(segment.get("id").unwrap_or(&Value::Null)) { segment["id"].clone() } else { json!(format!("scene_{}", index + 1)) })),
            ("scene_defaults", json!(scoped)),
            ("scene_number", json!(index + 1)),
            ("label", json!(label)),
            ("lyrics", json!(lyric)),
            ("lyric_section", json!(pick("lyric_section"))),
            ("story_beat", json!(pick("story_beat"))),
            ("flf_start_state", json!(pick("flf_start_state"))),
            ("flf_transformation", json!(pick("flf_transformation"))),
            ("flf_end_state", json!(pick("flf_end_state"))),
            ("flf_carry_forward", json!(pick("flf_carry_forward"))),
            ("performance_mode", json!(performance_mode(session, segment))),
            ("prompt_summary", json!(summary)),
            ("motion_summary", json!(video_notes)),
            ("lyric_singers", json!(singers)),
            ("lyric_cue_map", json!(list("lyric_cue_map"))),
            ("lyric_shot_word_timing_enabled", json!(segment.get("lyric_shot_word_timing_enabled").is_some_and(truthy))),
            ("lyric_performance_mode", json!(pick("lyric_performance_mode"))),
            ("speaker_assignments", json!(list("minimax_speaker_assignments"))),
            ("lyric_no_lip_sync", json!(no_lip_sync)),
            ("lyric_instrumental", json!(is_instrumental_lyric_text(&lyric))),
            ("no_character_present", json!(no_character)),
            ("subjects", json!(mapped.iter().map(|r| r["name"].clone()).collect::<Vec<_>>())),
            ("subject_refs", json!(mapped)),
            ("setting", json!(setting)),
            ("location_ref", json!(location)),
            ("timeline_start", json!(start)),
            ("timeline_end", json!(end)),
            ("exact_duration", json!((end - start).max(0.0))),
            ("shot_type", json!(pick("shot_type"))),
            ("camera_motion", json!(first_text(segment, &["camera_motion", "motion_preset"]))),
            ("character_motion", json!(pick("character_motion"))),
            ("include_microphone", json!(segment.get("include_microphone").is_some_and(truthy))),
            ("performance_style", json!(either(pick("performance_style"), text(defaults.get("performance_style"))))),
            ("facial_performance", json!(pick("facial_performance"))),
            ("facial_performance_custom", json!(pick("facial_performance_custom"))),
            ("emotion_expression_tags", json!(pick("emotion_expression_tags"))),
            ("project_video_engine", json!(engine)),
            ("minimax_h3_mode", json!(Some(pick("minimax_h3_mode")).filter(|m| !m.is_empty()).unwrap_or_else(|| if minimax { "reference_to_video".into() } else { String::new() }))),
            ("video_style", json!(either(pick("minimax_h3_video_style"), text(defaults.get("video_style"))))),
            ("video_style_custom", json!(either(pick("minimax_h3_video_style_custom"), text(defaults.get("video_style_custom"))))),
            ("temporal_world_effect_override", json!(Some(pick("temporal_world_effect_override")).filter(|m| !m.is_empty()).unwrap_or_else(|| "global".into()))),
            ("temporal_world_effect_custom", json!(pick("temporal_world_effect_custom"))),
            ("video_prompt_type", json!(if VIDEO_PROMPT_TYPES.contains(&stored_type.as_str()) { stored_type } else if minimax { "rtv".into() } else { "i2v".into() })),
            ("image_prompt", json!(image_prompt_of(segment, &image_mode))),
            ("video_prompt", json!(raw_prompt)),
            // A saved prompt with no recorded origin was written by the LLM.
            ("video_prompt_origin", json!(if origin == "gemma" || (!raw_prompt.is_empty() && origin.is_empty()) { "gemma" } else { "manual" })),
            ("minimax_h3_pass2_prompt", json!(text_or_empty(segment.get("minimax_h3_pass2_prompt")))),
            ("image_path", json!(first_text(segment, &["approved_image_path", "image_path"]))),
            ("notes", json!(scene_notes)),
            ("timeline_note", json!(text_or_empty(segment.get("timeline_note")))),
            ("audio_direction", json!(pick("audio_direction"))),
            ("continuity", json!(pick("continuity"))),
            ("extra_subjects", json!([])),
        ] {
            card.insert(key.into(), value);
        }
        cards.push(Value::Object(card));
    }
    cards
}

/// The complete card for an LLM: saved output drafts left out, and pictures reduced to the fact that they exist.
pub fn scene_card_context(card: &Value) -> Value {
    let mut context = fresh_prompt_scene_context(card);
    let Some(map) = context.as_object_mut() else { return context };
    let has_image = map.shift_remove("image_data").is_some_and(|v| truthy(&v));
    map.insert("has_inline_image".into(), json!(has_image));
    let describe = |reference: &Value| -> Value {
        let Some(map) = reference.as_object() else { return Value::Null };
        let image = dict(map.get("image"));
        let mut described = map.clone();
        described.insert("image".into(), json!({"path": text(image.get("path")), "name": text(image.get("name")), "has_inline_image": image.get("data").is_some_and(truthy)}));
        Value::Object(described)
    };
    let refs: Vec<Value> = map.get("subject_refs").and_then(Value::as_array).into_iter().flatten().filter(|r| r.is_object()).map(describe).collect();
    map.insert("subject_refs".into(), Value::Array(refs));
    let location = map.get("location_ref").map(describe).unwrap_or(Value::Null);
    map.insert("location_ref".into(), location);
    context
}

/// The selected scene of a scene-beat request, with its complete card.
fn beat_scene(card: &Value) -> Value {
    let mut blank = card.clone();
    if let Some(map) = blank.as_object_mut() {
        map.insert("story_beat".into(), json!(""));
    }
    let scene = scene_card_context(&blank);
    let mut out = scene.as_object().cloned().unwrap_or_default();
    out.insert("scene_card".into(), scene_card_context(&scene));
    out.insert("scene_card_instruction".into(), json!(constant("storyboard", "_STORYBOARD_SCENE_CARD_CONTEXT_INSTRUCTIONS")));
    out.insert("director_note".into(), json!(text(card.get("timeline_note"))));
    Value::Object(out)
}

fn defaults_of(session: &Obj) -> Obj {
    dict(session.get("builder_storyboard_defaults"))
}

fn truthy_or(value: Option<&Value>, default: Value) -> Value {
    value.filter(|v| truthy(v)).cloned().unwrap_or(default)
}

/// The slim storyboard block the arc and brief steps read.
fn storyboard_summary(session: &Obj, cards: &[Value]) -> Obj {
    let defaults = defaults_of(session);
    let refs = dict(session.get("flux_reference_builder"));
    let slim = |key: &str| -> Vec<Value> { refs.get(key).and_then(Value::as_array).into_iter().flatten().filter(|c| c.is_object()).map(|c| slim_reference(Some(c))).collect() };
    let engine = text(session.get("video_engine"));
    let mut summary = Obj::new();
    for (key, value) in [
        ("mode", json!("video")),
        ("project_video_engine", json!(engine)),
        ("camera_flow", truthy_or(defaults.get("camera_flow"), json!("balanced"))),
        ("video_style", truthy_or(defaults.get("video_style"), json!(""))),
        ("video_style_custom", truthy_or(defaults.get("video_style_custom"), json!(""))),
        ("global_consistency_phrase", truthy_or(defaults.get("global_consistency_phrase"), json!(""))),
        ("camera_motion_speed", defaults.get("camera_motion_speed").cloned().unwrap_or(json!(4))),
        ("character_motion_speed", defaults.get("character_motion_speed").cloned().unwrap_or(json!(4))),
        ("story_arc_detail", truthy_or(defaults.get("story_arc_detail"), json!("standard"))),
        ("minimax_h3_cut_frequency", defaults.get("minimax_h3_cut_frequency").cloned().unwrap_or(json!(0))),
        ("minimax_h3_cut_style", truthy_or(defaults.get("minimax_h3_cut_style"), json!("free"))),
        ("performance_style_default", truthy_or(defaults.get("performance_style"), json!(""))),
        ("story_layer", Value::Object(story_layer(session))),
        ("reference_builder", json!({"subjects": slim("subjects"), "locations": slim("locations")})),
        ("scenes", Value::Array(cards.to_vec())),
    ] {
        summary.insert(key.into(), value);
    }
    summary
}

/// The lyrics of the cards, and the Line Mapping text.
fn lyrics_blocks(session: &Obj, cards: &[Value]) -> (String, String) {
    let mapper = dict(session.get("lyric_mapper"));
    let lyrics = cards.iter().map(|c| text_or_empty(c.get("lyrics"))).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    (lyrics, text(mapper.get("source_text")))
}

fn require_scenes(cards: &[Value]) -> Result<(), StepFailure> {
    if cards.is_empty() { invalid("The project has no scenes. Create the timeline first.") } else { Ok(()) }
}

/// What the LLM steps get: the step's own fields, then the caller's sampling settings. The model that is loaded is used and never unloaded.
fn llm_request(params: &Obj, extra: Vec<(&str, Value)>) -> Obj {
    let mut request = Obj::new();
    for (key, value) in extra {
        request.insert(key.into(), value);
    }
    for key in ["temperature", "top_p", "max_new_tokens", "seed"] {
        if let Some(value) = params.get(key) {
            request.insert(key.into(), value.clone());
        }
    }
    request.insert("unload_after".into(), json!(false));
    request
}

/// The request of the Story Mode steps: the story layer, the scenes, the Reference Builder and the lyrics, plus the step's own fields.
fn story_request(session: &Obj, params: &Obj, cards: &[Value], extra: Vec<(&str, Value)>) -> Obj {
    let summary = storyboard_summary(session, cards);
    let refs = dict(session.get("flux_reference_builder"));
    let mut reference_builder = dict(summary.get("reference_builder"));
    reference_builder.insert("extras".into(), refs.get("extras").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([])));
    let mut fields = vec![
        ("story_layer", Value::Object(story_layer(session))),
        ("scenes", Value::Array(cards.to_vec())),
        ("reference_builder", Value::Object(reference_builder)),
        ("lyrics", json!(lyrics_blocks(session, cards).0)),
    ];
    fields.extend(extra);
    llm_request(params, fields)
}

fn failed(message: String) -> StepFailure {
    StepFailure::Failed(message)
}

fn used_model(result: &Value) -> Value {
    result.get("used_model").cloned().unwrap_or_else(|| json!(""))
}

// -- saving ------------------------------------------------------------------------------------------------------------------

/// Save what a Story Mode step changed in the story layer. A scene plan also sets each scene's performance and continuations and gives scenes their cast.
async fn save_fields(store: &dyn Store, fields: Obj) -> Result<Value, StepFailure> {
    let (_, revision) = store
        .save(Box::new(move |session| {
            let mut layer = story_layer(session);
            layer.extend(fields.clone());
            session.insert("builder_story_layer".into(), Value::Object(layer.clone()));
            session.shift_remove("builderStoryLayer");
            if let Some(plan) = fields.get("story_scene_plan").filter(|p| p.as_array().is_some_and(|a| !a.is_empty())) {
                apply_plan_to_segments(session, plan);
                fill_cast_from_acts(session, layer.get("story_acts").unwrap_or(&json!([])), plan);
                apply_continuations_to_segments(session, plan);
            }
            Ok(Value::Null)
        }))
        .await?;
    Ok(json!(revision))
}

fn layer_fields(result: &Value) -> Obj {
    dict(result.get("story_layer"))
}

// -- the steps ---------------------------------------------------------------------------------------------------------------

/// Run one story step. `params` is what the caller sent.
pub async fn run_step(llm: &dyn Llm, store: &dyn Store, step: &str, params: &Obj, progress: Progress<'_>) -> StepResult {
    match step {
        "arc" => create_story_arc(llm, store, params).await,
        "brief" => create_story_brief(llm, store, params).await,
        "beats" => create_scene_beats(llm, store, params, progress).await,
        "book" => create_story_book(llm, store, params).await,
        "chapter" => rewrite_story_chapter(llm, store, params).await,
        "split" => split_story(llm, store, params).await,
        "convert" => convert_story_arc(llm, store, params).await,
        "remove_people" => remove_story_people(llm, store, params).await,
        "cast" => scan_story_cast(llm, store, params).await,
        "acts" => create_story_acts(llm, store, params).await,
        "plan" => create_story_scene_plan(llm, store, params).await,
        "locations" => create_story_locations(llm, store, params).await,
        other => invalid(format!("step must be one of: {}. Got '{other}'.", STEPS.join(", "))),
    }
}

/// Write the story arc from the story idea and the lyrics, and save it.
async fn create_story_arc(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    require_scenes(&cards)?;
    let mut layer = story_layer(&session);
    let idea = Some(text(params.get("story_idea"))).filter(|i| !i.is_empty()).unwrap_or_else(|| text(layer.get("overall_story_idea")));
    if idea.is_empty() {
        return invalid("A story idea is required. Pass story_idea or save one with the story settings.");
    }
    layer.insert("overall_story_idea".into(), json!(idea));
    let mut storyboard = storyboard_summary(&session, &cards);
    let mut arc_layer = layer.clone();
    arc_layer.insert("user_story_arc".into(), json!(""));
    storyboard.insert("story_layer".into(), Value::Object(arc_layer.clone()));
    let defaults = defaults_of(&session);
    let (lyrics, line_mapping) = lyrics_blocks(&session, &cards);
    let request = llm_request(
        params,
        vec![
            ("max_new_tokens", json!(count(params, "max_new_tokens", 2400))),
            ("story_layer", Value::Object(arc_layer)),
            ("storyboard", Value::Object(storyboard)),
            ("story_idea", json!(idea)),
            ("previous_story_arc", json!(text(params.get("previous_story_arc")))),
            ("timeline_markers", truthy_or(session.get("timeline_markers"), json!([]))),
            ("project_folder", json!(store.folder().display().to_string())),
            ("scenes", Value::Array(cards.clone())),
            ("camera_flow", truthy_or(defaults.get("camera_flow"), json!("balanced"))),
            ("camera_motion_speed", defaults.get("camera_motion_speed").cloned().unwrap_or(json!(4))),
            ("character_motion_speed", defaults.get("character_motion_speed").cloned().unwrap_or(json!(4))),
            ("story_arc_detail", truthy_or(params.get("story_arc_detail"), truthy_or(defaults.get("story_arc_detail"), json!("standard")))),
            ("performance_style", truthy_or(defaults.get("performance_style"), json!(""))),
            ("lyrics", json!(lyrics)),
            ("line_mapping_lyrics", json!(line_mapping)),
        ],
    );
    let result = match build_story_layer_arc(llm, &request).await {
        Ok(result) => result,
        Err(ArcError::Failed(message)) => return Err(failed(message)),
        Err(ArcError::Format(found)) => return Err(failed(format!("{} The model's answer did not keep the lyric sections ({}).", found.message, found.expected_sections.join(", ")))),
    };
    let arc = text(result.get("story_arc"));
    if arc.is_empty() {
        return invalid("The LLM returned an empty story arc.");
    }
    let saved = arc.clone();
    let (_, revision) = store
        .save(Box::new(move |session| {
            let mut layer = story_layer(session);
            layer.insert("overall_story_idea".into(), json!(idea));
            layer.insert("user_story_arc".into(), json!(saved));
            session.insert("builder_story_layer".into(), Value::Object(layer));
            session.shift_remove("builderStoryLayer");
            Ok(Value::Null)
        }))
        .await?;
    Ok(json!({"story_arc": arc, "model": used_model(&result), "revision": revision}))
}

/// Write the song story brief from the lyrics and the story arc, and save it.
async fn create_story_brief(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    require_scenes(&cards)?;
    let summary = storyboard_summary(&session, &cards);
    let request = llm_request(
        params,
        vec![
            ("max_new_tokens", json!(count(params, "max_new_tokens", 1200))),
            ("story_layer", Value::Object(story_layer(&session))),
            ("reference_builder", summary.get("reference_builder").cloned().unwrap_or(Value::Null)),
            ("storyboard", Value::Object(summary)),
            ("scenes", Value::Array(cards.clone())),
            ("lyrics", json!(lyrics_blocks(&session, &cards).0)),
        ],
    );
    let result = build_story_layer_brief(llm, &request).await.map_err(failed)?;
    let brief = text(result.get("story_brief"));
    if brief.is_empty() {
        return invalid("The LLM returned an empty story brief.");
    }
    let saved = brief.clone();
    let (_, revision) = store
        .save(Box::new(move |session| {
            let mut layer = story_layer(session);
            layer.insert("song_story_brief".into(), json!(saved));
            session.insert("builder_story_layer".into(), Value::Object(layer));
            session.shift_remove("builderStoryLayer");
            Ok(Value::Null)
        }))
        .await?;
    Ok(json!({"story_brief": brief, "model": used_model(&result), "revision": revision}))
}

/// The first or the last non-empty line of a lyric.
fn adjacent_line(lyric: &str, last: bool) -> String {
    let lines: Vec<&str> = lyric.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    (if last { lines.last() } else { lines.first() }).map(|l| l.to_string()).unwrap_or_default()
}

/// Write a story beat for each scene in timeline order, saving after every scene. Only scenes with no beat are written unless `replace_existing`.
/// `scene_ids` (ids or numbers) and `limit` narrow the set.
async fn create_scene_beats(llm: &dyn Llm, store: &dyn Store, params: &Obj, progress: Progress<'_>) -> StepResult {
    let session = store.read()?;
    let mut cards = scene_cards(&session);
    require_scenes(&cards)?;
    let layer = story_layer(&session);
    if layer.get("story_flow_version").and_then(to_int).unwrap_or(0) >= 2 {
        if !layer.get("story_scene_plan").is_some_and(truthy) {
            return invalid("Story Mode: create the scene plan (story step 'plan') before scene beats.");
        }
    } else if !["user_story_arc", "song_story_brief", "overall_story_idea"].iter().any(|k| layer.get(*k).is_some_and(truthy)) {
        return invalid("Create the story arc (and brief) before scene beats.");
    }
    let wanted: Vec<String> = params.get("scene_ids").and_then(Value::as_array).into_iter().flatten().map(|v| text_or_empty(Some(v))).collect();
    let replace = params.get("replace_existing").is_some_and(truthy);
    let mut targets: Vec<usize> = (0..cards.len())
        .filter(|&i| {
            let named = wanted.is_empty() || wanted.contains(&text_or_empty(cards[i].get("id"))) || wanted.contains(&(i + 1).to_string());
            named && (replace || !wanted.is_empty() || text_or_empty(cards[i].get("story_beat")).is_empty())
        })
        .collect();
    let limit = count(params, "limit", 0);
    if limit > 0 {
        targets.truncate(limit as usize);
    }
    if targets.is_empty() {
        return Ok(json!({"created": 0, "skipped": cards.len(), "beats": [], "revision": session.get("revision").cloned().unwrap_or(Value::Null)}));
    }
    let refs = dict(session.get("flux_reference_builder"));
    let all_subjects: Vec<Value> = refs
        .get("subjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s.is_object())
        .map(|s| json!({"name": text(s.get("name")), "description": text(s.get("description"))}))
        .collect();
    let (mut beats, mut revision) = (Vec::new(), session.get("revision").cloned().unwrap_or(Value::Null));
    for (done, &index) in targets.iter().enumerate() {
        let number = index + 1;
        progress(done, targets.len(), &format!("Scene {number}"));
        let (previous, following) = (index.checked_sub(1).map(|i| &cards[i]), cards.get(index + 1));
        let request = llm_request(
            params,
            vec![
                ("max_new_tokens", json!(count(params, "max_new_tokens", 360))),
                ("story_layer", Value::Object(layer.clone())),
                ("all_subjects", Value::Array(all_subjects.clone())),
                ("storyboard_payload", json!({"scenes": [beat_scene(&cards[index])], "selected_scene_number": number, "story_layer": Value::Object(layer.clone())})),
                ("previous_beat", json!(previous.map(|c| text(c.get("story_beat"))).unwrap_or_default())),
                ("previous_lyrics", json!(previous.map(|c| adjacent_line(&text(c.get("lyrics")), true)).unwrap_or_default())),
                ("current_lyrics", cards[index].get("lyrics").cloned().unwrap_or(json!(""))),
                ("next_lyrics", json!(following.map(|c| adjacent_line(&text(c.get("lyrics")), false)).unwrap_or_default())),
                ("temperature", json!(params.get("temperature").filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.35))),
                ("top_p", json!(params.get("top_p").filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.90))),
            ],
        );
        let result = build_scene_beat(llm, &request).await.map_err(failed)?;
        let beat = text(result.get("story_beat"));
        if beat.is_empty() {
            return invalid(format!("The LLM returned an empty story beat for scene {number}."));
        }
        let scene_id = text(cards[index].get("id"));
        let (id, written) = (scene_id.clone(), beat.clone());
        let (_, saved) = store
            .save(Box::new(move |session| {
                for segment in session.get_mut("segments").and_then(Value::as_array_mut).into_iter().flatten() {
                    if segment.get("id").and_then(Value::as_str) == Some(id.as_str()) {
                        segment["story_beat"] = json!(written);
                    }
                }
                Ok(Value::Null)
            }))
            .await?;
        revision = json!(saved);
        cards[index]["story_beat"] = json!(beat);
        beats.push(json!({"scene_id": scene_id, "scene_number": number, "story_beat": beat}));
    }
    progress(targets.len(), targets.len(), "done");
    let created = beats.len();
    Ok(json!({"created": created, "skipped": cards.len() - created, "beats": beats, "revision": revision}))
}

/// Write the story book from the premise (`story_idea`, else the saved idea) and save it.
async fn create_story_book(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    require_scenes(&cards)?;
    let previous = Some(text_or_empty(params.get("previous_book"))).filter(|p| !p.is_empty()).unwrap_or_else(|| text_or_empty(story_layer(&session).get("story_book_text")));
    let request = story_request(
        &session,
        params,
        &cards,
        vec![
            ("story_idea", json!(text_or_empty(params.get("story_idea")))),
            ("tone_notes", json!(text_or_empty(params.get("tone_notes")))),
            ("previous_book", json!(previous)),
            ("max_new_tokens", json!(count(params, "max_new_tokens", 6000))),
        ],
    );
    let result = book::draft_story_book(llm, &request).await.map_err(failed)?;
    let fields = layer_fields(&result);
    let revision = save_fields(store, fields.clone()).await?;
    Ok(json!({
        "story_book": result.get("story_book").cloned().unwrap_or(Value::Null),
        "story_book_text": fields.get("story_book_text").cloned().unwrap_or(Value::Null),
        "story_settings": fields.get("story_settings").cloned().unwrap_or(Value::Null),
        "removed_people": result.get("removed_people").cloned().unwrap_or(json!([])),
        "model": used_model(&result),
        "revision": revision,
    }))
}

/// Rewrite one chapter (`chapter_id`) following `direction` and save it.
async fn rewrite_story_chapter(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let chapter_id = text(params.get("chapter_id"));
    if chapter_id.is_empty() {
        return invalid("chapter_id is required (see story_book.chapters[].id from GET /projects/{pid}/story).");
    }
    let session = store.read()?;
    let cards = scene_cards(&session);
    let request = story_request(
        &session,
        params,
        &cards,
        vec![("chapter_id", json!(chapter_id)), ("direction", json!(text_or_empty(params.get("direction")))), ("max_new_tokens", json!(count(params, "max_new_tokens", 2400)))],
    );
    let result = book::rewrite_story_chapter(llm, &request).await.map_err(StepFailure::Invalid)?;
    let fields = layer_fields(&result);
    let revision = save_fields(store, fields.clone()).await?;
    Ok(json!({
        "chapter": result.get("chapter").cloned().unwrap_or(Value::Null),
        "story_book_text": fields.get("story_book_text").cloned().unwrap_or(Value::Null),
        "removed_people": result.get("removed_people").cloned().unwrap_or(json!([])),
        "model": used_model(&result),
        "revision": revision,
    }))
}

/// Turn a story the user wrote (`text`) into chapters with settings, keeping every word, and save it.
async fn split_story(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    if text_or_empty(params.get("text")).trim().is_empty() {
        return invalid("text is required: the full story to split into chapters.");
    }
    let session = store.read()?;
    let cards = scene_cards(&session);
    let request = story_request(&session, params, &cards, vec![("text", params["text"].clone()), ("title", json!(text_or_empty(params.get("title"))))]);
    let result = book::split_pasted_story(llm, &request).await.map_err(StepFailure::Invalid)?;
    let fields = layer_fields(&result);
    let revision = save_fields(store, fields.clone()).await?;
    Ok(json!({
        "story_book": result.get("story_book").cloned().unwrap_or(Value::Null),
        "story_book_text": fields.get("story_book_text").cloned().unwrap_or(Value::Null),
        "story_settings": fields.get("story_settings").cloned().unwrap_or(Value::Null),
        "model": used_model(&result),
        "revision": revision,
    }))
}

/// Turn the project's old song-section story arc into a Story Mode book and save it.
async fn convert_story_arc(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    let request = story_request(&session, params, &cards, vec![]);
    let result = book::convert_story_arc(llm, &request).await.map_err(StepFailure::Invalid)?;
    let fields = layer_fields(&result);
    let revision = save_fields(store, fields.clone()).await?;
    Ok(json!({
        "story_book": result.get("story_book").cloned().unwrap_or(Value::Null),
        "story_book_text": fields.get("story_book_text").cloned().unwrap_or(Value::Null),
        "story_settings": fields.get("story_settings").cloned().unwrap_or(Value::Null),
        "model": used_model(&result),
        "revision": revision,
    }))
}

/// Rewrite the book without the people in `names` and save it. Acts and the scene plan are cleared.
async fn remove_story_people(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let names = params.get("names").and_then(Value::as_array).cloned().unwrap_or_default();
    if !names.iter().any(|n| !text(Some(n)).is_empty()) {
        return invalid("names is required: the people to remove (from the cast step's characters[].name).");
    }
    let session = store.read()?;
    let cards = scene_cards(&session);
    let request = story_request(&session, params, &cards, vec![("names", Value::Array(names))]);
    let result = book::remove_story_people(llm, &request).await.map_err(StepFailure::Invalid)?;
    let fields = layer_fields(&result);
    let revision = save_fields(store, fields.clone()).await?;
    Ok(json!({
        "story_book": result.get("story_book").cloned().unwrap_or(Value::Null),
        "story_book_text": fields.get("story_book_text").cloned().unwrap_or(Value::Null),
        "removed_people": result.get("removed_people").cloned().unwrap_or(json!([])),
        "revision": revision,
    }))
}

/// The people in the story who have no character card. Nothing is saved: add them with the reference tools.
async fn scan_story_cast(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    let request = story_request(&session, params, &cards, vec![]);
    let result = book::scan_story_cast(llm, &request).await.map_err(StepFailure::Invalid)?;
    Ok(json!({"characters": result.get("characters").cloned().unwrap_or(json!([])), "model": used_model(&result)}))
}

/// Split every chapter (or `chapter_id`) into acts with the LLM and save them.
async fn create_story_acts(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    require_scenes(&cards)?;
    let request = story_request(&session, params, &cards, vec![("chapter_id", json!(text_or_empty(params.get("chapter_id")))), ("timeline_markers", truthy_or(session.get("timeline_markers"), json!([])))]);
    let result = build_story_acts(llm, &request).await.map_err(StepFailure::Invalid)?;
    let fields = layer_fields(&result);
    let revision = save_fields(store, fields.clone()).await?;
    Ok(json!({"story_acts": fields.get("story_acts").cloned().unwrap_or(json!([])), "model": used_model(&result), "revision": revision}))
}

/// Write every scene's story slice, spot, mood and performance (or only `act_id`'s) and save them. Each scene's performance is also written to its
/// timeline scene.
async fn create_story_scene_plan(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    require_scenes(&cards)?;
    let request = story_request(
        &session,
        params,
        &cards,
        vec![
            ("act_id", json!(text_or_empty(params.get("act_id")))),
            ("spot_min_scenes", params.get("spot_min_scenes").cloned().unwrap_or(Value::Null)),
            ("timeline_markers", truthy_or(session.get("timeline_markers"), json!([]))),
        ],
    );
    let result = build_story_scene_plan(llm, &request).await.map_err(StepFailure::Invalid)?;
    let fields = layer_fields(&result);
    let revision = save_fields(store, fields.clone()).await?;
    Ok(json!({"story_scene_plan": fields.get("story_scene_plan").cloned().unwrap_or(json!([])), "model": used_model(&result), "revision": revision}))
}

/// The project's theme file and the location notes, as the location steps get them.
pub fn location_style_theme_text(folder: &Path, session: &Obj, local_notes: &str) -> String {
    let use_context = session.get("use_vrgdg_text_context").is_none_or(truthy);
    let theme = if use_context { std::fs::read_to_string(folder.join("project_context").join("themestyle.txt")).map(|t| t.trim().to_string()).unwrap_or_default() } else { String::new() };
    let mut parts = Vec::new();
    if !theme.is_empty() {
        parts.push(format!("Global theme/style:\n{theme}"));
    }
    if !local_notes.trim().is_empty() {
        parts.push(format!("Location extraction notes:\n{}", local_notes.trim()));
    }
    parts.join("\n\n").trim().to_string()
}

fn new_location_id(taken: &mut std::collections::HashSet<String>) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let mut salt = now.subsec_nanos() as u64;
    loop {
        salt = salt.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let id = format!("loc_{}_{}", now.as_millis(), (salt >> 33) % 10000);
        if taken.insert(id.clone()) {
            return id;
        }
    }
}

/// Give each chapter setting a look and each place a location card, then map every scene to its spot. A card with the same name keeps its image, and its
/// description unless `replace_descriptions`. Before any scene is planned the places are found inside each chapter setting (the Places tab). With `map_only`
/// after the plan nothing is written by the LLM: each scene is only mapped to a card that exists.
async fn create_story_locations(llm: &dyn Llm, store: &dyn Store, params: &Obj) -> StepResult {
    let session = store.read()?;
    let cards = scene_cards(&session);
    require_scenes(&cards)?;
    let refs = dict(session.get("flux_reference_builder"));
    let notes = Some(text(params.get("style_theme"))).filter(|n| !n.is_empty()).unwrap_or_else(|| text(refs.get("location_style_theme")));
    let request = story_request(
        &session,
        params,
        &cards,
        vec![
            ("style_theme", json!(location_style_theme_text(store.folder(), &session, &notes))),
            ("regenerate_looks", json!(params.get("regenerate_looks").is_some_and(truthy))),
            ("map_only", json!(params.get("map_only").is_some_and(truthy))),
        ],
    );
    let result = build_story_locations(llm, &request).await.map_err(StepFailure::Invalid)?;
    let found: Vec<Value> = result.get("locations").and_then(Value::as_array).cloned().unwrap_or_default();
    let (replace, layer_update) = (params.get("replace_descriptions").is_some_and(truthy), layer_fields(&result));
    let to_save = found.clone();
    let (answer, revision) = store
        .save(Box::new(move |session| {
            let refs = session.get("flux_reference_builder").cloned().unwrap_or_else(|| json!({}));
            let (merged, counts) = apply_story_locations(&refs, &to_save, &mut new_location_id, replace);
            let names: std::collections::HashMap<String, String> =
                merged.get("locations").and_then(Value::as_array).into_iter().flatten().map(|c| (text_or_empty(c.get("id")), text_or_empty(c.get("name")))).collect();
            let scene_map: Obj = merged
                .get("scene_map")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(scene, location)| (scene.clone(), json!(names.get(&text_or_empty(Some(location))).cloned().unwrap_or_else(|| text_or_empty(Some(location))))))
                .collect();
            session.insert("flux_reference_builder".into(), merged);
            let mut layer = story_layer(session);
            layer.extend(layer_update);
            session.insert("builder_story_layer".into(), Value::Object(layer));
            session.shift_remove("builderStoryLayer");
            Ok(json!({"added": counts.added, "updated": counts.updated, "scene_map": scene_map}))
        }))
        .await?;
    let places: Vec<Value> = found.iter().map(|item| json!({"name": text(item.get("name")), "scenes": item.get("scene_ids").and_then(Value::as_array).map_or(0, Vec::len)})).collect();
    Ok(json!({
        "locations": places,
        "added": answer["added"],
        "updated": answer["updated"],
        "scene_map": answer["scene_map"],
        "model": used_model(&result),
        "revision": revision,
    }))
}

#[cfg(test)]
mod tests {
    use h3_project::story::edit_session;
    use h3_project::{load_session, Stamp};

    use super::*;
    use crate::prompt_writers::testing::Canned;

    struct TempStore {
        _dir: tempfile::TempDir,
        folder: std::path::PathBuf,
    }

    impl TempStore {
        fn new(session: Value) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("Demo");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("vrgdg_builder_session.json"), serde_json::to_string_pretty(&session).unwrap()).unwrap();
            Self { _dir: dir, folder }
        }

        fn session(&self) -> Obj {
            load_session(&self.folder).unwrap().session
        }
    }

    impl Store for TempStore {
        fn folder(&self) -> &Path {
            &self.folder
        }

        fn read(&self) -> Result<Obj, StepFailure> {
            load_session(&self.folder).map(|loaded| loaded.session).map_err(|e| StepFailure::Failed(e.to_string()))
        }

        fn save(&self, change: Change) -> SaveFuture<'_> {
            Box::pin(async move { edit_session(&self.folder, None, &Stamp::now(), change).map_err(StepFailure::from) })
        }
    }

    fn session() -> Value {
        json!({
            "project_name": "Demo",
            "revision": 1,
            "video_engine": "minimax_h3",
            "video_type": "story",
            "minimax_h3_settings": {"video_mode": "reference_to_video", "render_pass": "single"},
            "segments": [
                {"id": "b", "start": 4.0, "end": 8.0, "label": "Scene 2", "lyric_text": "the street is empty"},
                {"id": "a", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "walking home alone", "performance_mode": "singing"},
                {"id": "c", "start": 8.0, "end": 12.0, "label": "Scene 3", "lyric_text": "I count the lights", "no_character_present": true}
            ],
            "builder_story_layer": {"overall_story_idea": "A woman walks home through a quiet town."},
            "flux_reference_builder": {
                "subjects": [{"id": "ava", "name": "Ava", "description": "dark hair", "image": {"path": "C:\\refs\\ava.png", "data": "AAAA"}}],
                "locations": [{"id": "street", "name": "Main Street", "description": "wet asphalt and warm windows"}],
                "subject_scene_map": {"a": ["ava"], "b": "ava", "c": ["ava"]},
                "scene_map": {"a": "street", "b": "street"},
                "use_subject_reference": true
            }
        })
    }

    fn quiet(_done: usize, _total: usize, _label: &str) {}

    fn params(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_refmod_card_keeps_its_refmod_and_a_picture_card_has_none() {
        let mut value = session();
        let refmod = params(json!({"source": "refmod", "refmod": {"name": "background/Main Street", "kind": "image", "strength": 0.8}}));
        value["flux_reference_builder"]["locations"][0].as_object_mut().unwrap().extend(refmod);
        let cards = scene_cards(value.as_object().unwrap());
        let location = &cards[0]["location_ref"];
        assert_eq!((location["source"].as_str(), location["refmod"]["name"].as_str()), (Some("refmod"), Some("background/Main Street")));
        assert_eq!(location["refmod"]["strength"], 0.8);
        assert_eq!(location["name"], "Main Street", "the card keeps its other fields");
        let subject = cards[0]["subject_refs"][0].as_object().unwrap();
        assert!(!subject.contains_key("source") && !subject.contains_key("refmod"), "{subject:?}");
    }

    #[test]
    fn the_cards_follow_the_timeline_and_carry_what_the_steps_read() {
        let cards = scene_cards(session().as_object().unwrap());
        let ids: Vec<&str> = cards.iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "b", "c"], "timeline order, not file order");
        assert_eq!(cards[1]["scene_number"], 2);
        assert_eq!(cards[0]["lyrics"], "walking home alone");
        assert_eq!(cards[0]["subjects"], json!(["Ava"]));
        assert_eq!(cards[0]["subject_refs"][0]["image"], json!({"path": "C:\\refs\\ava.png", "name": "", "data": ""}), "a card carries the path, never the file");
        assert_eq!(cards[1]["subjects"], json!(["Ava"]), "a map may hold one id as text");
        assert_eq!(cards[0]["location_ref"]["name"], "Main Street");
        assert_eq!(cards[0]["setting"], "wet asphalt and warm windows");
        assert_eq!(cards[2]["subjects"], json!([]), "a scene with no character present has no cast");
        assert_eq!(cards[2]["location_ref"], Value::Null);
        assert_eq!(cards[0]["project_video_engine"], "minimax_h3");
        assert_eq!(cards[0]["minimax_h3_mode"], "reference_to_video");
        assert_eq!(cards[0]["video_prompt_type"], "rtv");
        assert_eq!(cards[0]["performance_mode"], "singing");
        assert_eq!(cards[0]["exact_duration"], 4.0);
        let context = scene_card_context(&cards[0]);
        assert_eq!(context["subject_refs"][0]["image"]["has_inline_image"], false);
        assert_eq!(context["has_inline_image"], false);
        assert!(beat_scene(&cards[0])["scene_card_instruction"].as_str().unwrap().contains("Read every populated field"));
    }

    #[tokio::test]
    async fn scene_beats_are_written_in_order_and_saved_after_each_scene() {
        let store = TempStore::new(session());
        let llm = Canned::new(&[
            "Ava stands at the edge of the bus stop under a flickering streetlight holding a folded letter while headlights sweep across the wall behind her and she turns toward the empty road.",
            "Ava crosses the wet road with her coat swinging as the streetlights ahead blink out one by one and the shop windows go dark behind her along Main Street.",
        ]);
        let done = run_step(&llm, &store, "beats", &params(json!({"limit": 2})), &quiet).await.unwrap();
        assert_eq!((done["created"].as_i64(), done["skipped"].as_i64()), (Some(2), Some(1)));
        let saved = store.session();
        let beat = |id: &str| saved["segments"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap()["story_beat"].as_str().unwrap_or("").to_string();
        assert!(beat("a").starts_with("Ava stands at the edge"), "{}", beat("a"));
        assert!(beat("b").starts_with("Ava crosses the wet road"));
        assert_eq!(beat("c"), "", "the limit left scene 3 alone");
        // The second scene was asked with the first one's beat.
        let asked = llm.asked.lock().unwrap();
        assert!(format!("{:?}", asked[1]).contains("Ava stands at the edge"), "the next beat follows the one before it");
        assert_eq!(done["beats"][0]["scene_id"], "a");
    }

    #[tokio::test]
    async fn scene_beats_name_scenes_by_number_and_skip_written_ones() {
        let mut with_beat = session();
        with_beat["segments"][0]["story_beat"] = json!("Ava waits at the corner.");
        let store = TempStore::new(with_beat);
        let llm = Canned::new(&["Ava walks past the closed shops with her hands in her coat pockets and her breath showing in the cold air as the street lamps buzz above the empty road."]);
        let named = run_step(&llm, &store, "beats", &params(json!({"scene_ids": ["2"]})), &quiet).await.unwrap();
        assert_eq!((named["created"].as_i64(), named["beats"][0]["scene_id"].as_str()), (Some(1), Some("b")), "scene 2 on the timeline is the scene with id b, though the file lists it first, and a named scene is written again");
        let skipped = run_step(&Canned::new(&[]), &store, "beats", &params(json!({"limit": 1, "scene_ids": ["9"]})), &quiet).await.unwrap();
        assert_eq!(skipped["created"], 0, "a number that names no scene writes nothing");
    }

    #[tokio::test]
    async fn a_step_refuses_what_the_project_cannot_do() {
        let mut no_story = session();
        no_story["builder_story_layer"] = json!({});
        let store = TempStore::new(no_story);
        let llm = Canned::new(&[]);
        for (step, body, expect) in [
            ("beats", json!({}), "Create the story arc"),
            ("arc", json!({}), "A story idea is required"),
            ("chapter", json!({}), "chapter_id is required"),
            ("split", json!({}), "text is required"),
            ("remove_people", json!({}), "names is required"),
            ("dance", json!({}), "step must be one of"),
        ] {
            match run_step(&llm, &store, step, &params(body), &quiet).await {
                Err(StepFailure::Invalid(message)) => assert!(message.contains(expect), "{step}: {message}"),
                other => panic!("{step}: {other:?}"),
            }
        }
        let empty = TempStore::new(json!({"project_name": "Demo", "revision": 1, "segments": []}));
        match run_step(&llm, &empty, "acts", &params(json!({})), &quiet).await {
            Err(StepFailure::Invalid(message)) => assert!(message.contains("no scenes"), "{message}"),
            other => panic!("{other:?}"),
        }
        assert!(llm.asked.lock().unwrap().is_empty(), "nothing was asked of the LLM");
    }

    const BOOK: &str = "Title: The Walk Home\nPremise: A woman walks home alone and finds the town changed.\n\nChapter 1: The Walk\nSetting: Main Street\n\nAva leaves the station and walks down Main Street. The windows are warm and the road is wet.\n\nShe counts the streetlights as they flicker, one by one, until the last one goes out.";

    #[tokio::test]
    async fn a_story_is_written_step_by_step_and_saved_where_the_builder_keeps_it() {
        let mut plain = session();
        plain["builder_story_layer"] = json!({});
        plain["flux_reference_builder"]["locations"] = json!([]);
        plain["flux_reference_builder"]["scene_map"] = json!({});
        let store = TempStore::new(plain);

        // The book: one reply for the story, one for the check that no one outside the cast is in it.
        let llm = Canned::new(&[BOOK, "{\"characters\": []}"]);
        let book = run_step(&llm, &store, "book", &params(json!({"story_idea": "A woman walks home alone."})), &quiet).await.unwrap();
        assert_eq!(book["story_book"]["chapters"].as_array().unwrap().len(), 1, "{book}");
        let layer = story_layer(&store.session());
        assert_eq!(layer["story_flow_version"], 2);
        assert_eq!(layer["overall_story_idea"], "A woman walks home alone.");
        assert!(layer["story_book_text"].as_str().unwrap().contains("Act 1: The Walk"), "{layer:?}");
        assert_eq!(layer["story_settings"][0]["name"], "Main Street");

        // Acts, the scene plan and the places have a plain answer when the LLM has none, so each step still gives the project what the next one needs.
        let none = Canned::new(&[]);
        let acts = run_step(&none, &store, "acts", &params(json!({})), &quiet).await.unwrap();
        let act_scenes: Vec<&str> = acts["story_acts"].as_array().unwrap().iter().flat_map(|a| a["scene_ids"].as_array().unwrap().iter().map(|s| s.as_str().unwrap())).collect();
        assert_eq!(act_scenes, ["a", "b", "c"], "the acts cover the timeline in order: {acts}");

        let plan = run_step(&none, &store, "plan", &params(json!({})), &quiet).await.unwrap();
        let entries = plan["story_scene_plan"].as_array().unwrap();
        assert_eq!(entries.len(), 3, "{plan}");
        let saved = store.session();
        for segment in saved["segments"].as_array().unwrap() {
            assert!(segment.get("performance_mode").is_some_and(|m| !m.is_null()), "the plan sets each scene's performance: {segment}");
        }

        let places = run_step(&none, &store, "locations", &params(json!({"style_theme": "small town, night"})), &quiet).await.unwrap();
        assert!(places["added"].as_i64().unwrap() >= 1, "{places}");
        let saved = store.session();
        let cards = saved["flux_reference_builder"]["locations"].as_array().unwrap();
        assert!(cards.iter().any(|c| c["story_setting_id"].as_str().is_some_and(|id| !id.is_empty())), "the location cards belong to a story setting: {cards:?}");
        assert!(places["scene_map"].as_object().unwrap().len() >= 3, "every scene is mapped to a place: {places}");

        // The beats follow the plan. Each answer stays on Main Street, the place the story gave every scene.
        let on_street = "Ava walks along Main Street beside the warm shop windows with her collar turned up and her reflection sliding across the glass as she passes each door on the wet road.";
        let beats = Canned::new(&[on_street; 9]);
        let written = run_step(&beats, &store, "beats", &params(json!({})), &quiet).await.unwrap();
        assert_eq!(written["created"], 3, "{written}");
    }

    #[tokio::test]
    async fn the_cast_step_saves_nothing_and_a_failed_llm_saves_nothing() {
        let store = TempStore::new(session());
        let before = store.session();
        let llm = Canned::new(&["{\"characters\": [{\"name\": \"Mara\", \"reason\": \"mentioned in the story\"}]}"]);
        // No book yet: the step says so and the project is as it was.
        let refused = run_step(&llm, &store, "cast", &params(json!({})), &quiet).await;
        assert!(matches!(refused, Err(StepFailure::Invalid(_))), "{refused:?}");
        let down = run_step(&Canned::new(&[]), &store, "arc", &params(json!({})), &quiet).await;
        assert!(matches!(down, Err(StepFailure::Failed(_) | StepFailure::Invalid(_))), "{down:?}");
        assert_eq!(store.session(), before);
    }
}
