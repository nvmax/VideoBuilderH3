//! The Story Layer's two texts: the Story Brief and the Story Arc. The arc is written in two passes. Pass 1 writes one paragraph for each lyric
//! section and is checked against the section headings of the lyrics. Pass 2 (Detailed and Rich levels) writes one short note for each scene.
//! Port of `_build_story_layer_brief` and `_build_story_layer_arc` in `storyboard/story_layer.py`.

use std::collections::{HashMap, HashSet};

use fancy_regex::Regex;
use h3_core::pyjson::dumps;
use h3_core::pyval::{py_type_name, text_or_empty, to_int, truthy, Obj};
use h3_core::story::arc::{cap_words, normalize_output, parse_lyric_sections, scene_entries_enabled, scene_map, section_word_limit, split_sections};
use h3_core::story::cast_guard::{build_cast_guard, cast_leaks, cast_wall_text, strip_cast_leaks, strip_story_arc_entry_leaks, CastGuard};
use h3_core::story::helpers::{dialogue_reference_catalog, lyric_story_strength_guidance, normalize_story_arc_detail, normalize_story_layer, speed_value, story_arc_detail_profile, story_arc_entry_words};
use h3_core::story::mode::{text, text_s};
use h3_core::story::notes::story_arc_timeline_notes;
use h3_core::story::scene::{normalize_script_import, normalize_storyboard_scene};
use h3_llm::arc_prompts::{
    arc_format_retry_instruction, arc_json_retry_instruction, arc_schema, scene_entry_instruction, script_story_instruction, story_arc_instruction, story_brief_instruction, timeline_note_story_instruction,
    ArcInput, EntryInput,
};
use h3_llm::runner::{display_name, runner_kind};
use serde_json::{json, Value};

use super::beat::feeling_word_hits;
use crate::prompt_writers::{number, whole, Llm, TextCall};

type Reply = Result<Value, String>;

/// The model's story arc did not keep the lyric sections, even after the automatic format retry. It carries what the model wrote so the Builder
/// can show it.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatError {
    pub message: String,
    pub raw_output: String,
    pub cleaned_output: String,
    pub expected_sections: Vec<String>,
    pub runner: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArcError {
    Format(FormatError),
    Failed(String),
}

impl From<String> for ArcError {
    fn from(message: String) -> Self {
        ArcError::Failed(message)
    }
}

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// The last `count` characters, as the pack keeps the end of long model output.
fn last_chars(value: &str, count: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect()
}

/// The first of the candidates that is not falsy.
fn pick<'a>(candidates: impl IntoIterator<Item = Option<&'a Value>>) -> Option<&'a Value> {
    candidates.into_iter().flatten().find(|v| truthy(v))
}

/// The first of the keys that is in the request, whatever its value, else the fallback. Python's nested `payload.get(a, payload.get(b, ...))`.
fn first_present<'a>(payload: &'a Obj, keys: &[&str], fallback: Option<&'a Value>) -> Option<&'a Value> {
    keys.iter().find_map(|k| payload.get(*k)).or(fallback)
}

/// The run information fields every Story Layer reply carries.
fn run_fields(info: &Obj) -> Obj {
    let mut out = Obj::new();
    out.insert("runner".into(), info.get("runner").cloned().unwrap_or_else(|| json!("builtin")));
    out.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    out.insert("unloaded".into(), info.get("unloaded").cloned().unwrap_or_else(|| json!(true)));
    out
}

/// `_authoritative_script_from_payload`: the imported script when one is switched on and has dialogue.
fn authoritative_script(payload: &Obj) -> Option<Value> {
    let storyboard = payload.get("storyboard").and_then(Value::as_object);
    let source = pick([payload.get("script_import"), payload.get("scriptImport")]).or_else(|| storyboard.and_then(|s| pick([s.get("script_import"), s.get("scriptImport")])));
    let normalized = normalize_script_import(source);
    let usable = normalized.get("enabled").is_some_and(truthy) && normalized.get("cues").is_some_and(truthy);
    usable.then_some(normalized)
}

/// `_authoritative_script_text`: the script as the user pasted it, or one "Speaker: line" for each cue.
fn authoritative_script_text(script_import: &Value) -> String {
    let raw = text(script_import.get("raw_text"), 100000);
    if !raw.is_empty() {
        return raw;
    }
    array(script_import.get("cues"))
        .iter()
        .filter(|cue| cue.get("text").is_some_and(truthy))
        .map(|cue| {
            let speaker = pick([cue.get("speaker_alias"), cue.get("speaker_name")]).map_or_else(|| "Speaker".to_string(), |v| text_or_empty(Some(v)));
            format!("{speaker}: {}", text_or_empty(cue.get("text")))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `_build_short_film_script_story_text`: the brief or the premise of a short film from the imported script, whose dialogue is never changed.
async fn short_film_story_text(llm: &dyn Llm, payload: &Obj, script_import: &Value, purpose: &str) -> Result<(String, Obj), String> {
    let story_layer = normalize_story_layer(pick([payload.get("story_layer"), payload.get("storyLayer")]));
    let (subjects, locations) = dialogue_reference_catalog(payload);
    let planned = array(script_import.get("scene_plan").and_then(|plan| plan.get("scenes")));
    let compact_plan: Vec<Value> = planned
        .iter()
        .take(240)
        .map(|scene| {
            let dialogue: Vec<Value> = array(scene.get("speaker_assignments"))
                .iter()
                .map(|cue| {
                    let speaker = pick([cue.get("speaker_name"), cue.get("speaker_alias")]).map_or_else(|| "Speaker".to_string(), |v| text_or_empty(Some(v)));
                    json!({"speaker_id": cue.get("speaker_id").cloned().unwrap_or_else(|| json!("")), "speaker": speaker, "exact_text": cue.get("text").cloned().unwrap_or_else(|| json!(""))})
                })
                .collect();
            json!({
                "segment": scene.get("index").cloned().unwrap_or(Value::Null),
                "duration_seconds": scene.get("duration_seconds").cloned().unwrap_or(Value::Null),
                "continuation_of_previous": scene.get("continuation_of_previous").is_some_and(truthy),
                "dialogue": dialogue,
            })
        })
        .collect();
    let (max_tokens, label) = if purpose == "brief" { (1000, "Storyboard Authoritative Script Film Brief") } else { (1100, "Storyboard Authoritative Script Film Premise") };
    let listed = |items: &[Value]| if items.is_empty() { "[none]".to_string() } else { dumps(&Value::Array(items.to_vec())) };
    let mut instruction = script_story_instruction(
        purpose,
        &text_or_empty(story_layer.get("overall_story_idea")),
        &listed(&subjects),
        &listed(&locations),
        &authoritative_script_text(script_import),
        &dumps(&Value::Array(compact_plan)),
    );
    let timeline_notes = story_arc_timeline_notes(payload);
    if purpose == "premise" && !timeline_notes.is_empty() {
        instruction.push_str(&timeline_note_story_instruction(&dumps(&Value::Array(timeline_notes))));
    }
    let call = TextCall::new(instruction, number(payload, "temperature", 0.35)?, number(payload, "top_p", 0.90)?, whole(payload, "max_new_tokens", max_tokens)?, label).paragraphs();
    let (reply, info) = llm.text(call).await?;
    let reply = text_s(&reply, 10000);
    if reply.is_empty() {
        return Err("The LLM returned an empty short-film story result.".into());
    }
    Ok((reply, info))
}

/// `_build_story_layer_brief`: a compact story brief that guides the scene prompts without sending the lyrics every time.
pub async fn build_story_layer_brief(llm: &dyn Llm, payload: &Obj) -> Reply {
    let lyrics = text(pick([payload.get("lyrics"), payload.get("lyrics_text")]), 16000);
    let story_layer = normalize_story_layer(pick([payload.get("story_layer"), payload.get("storyLayer")]));
    if let Some(script) = authoritative_script(payload) {
        let (brief, info) = short_film_story_text(llm, payload, &script, "brief").await?;
        let mut out = Obj::new();
        out.insert("story_brief".into(), json!(brief));
        out.extend(run_fields(&info));
        out.insert("authoritative_script_used".into(), json!(true));
        return Ok(Value::Object(out));
    }
    let scenes = array(payload.get("scenes"));
    let mut compact_scenes: Vec<Value> = Vec::new();
    for (index, scene) in scenes.iter().take(160).enumerate() {
        if !scene.is_object() {
            continue;
        }
        let normalized = normalize_storyboard_scene(scene, index as i64 + 1);
        let extras: Vec<Value> = array(normalized.get("extra_subjects"))
            .iter()
            .filter(|item| item.get("name").is_some_and(truthy))
            .map(|item| json!({"name": item.get("name").cloned().unwrap_or(Value::Null), "count": item.get("count").cloned().unwrap_or_else(|| json!(1)), "interaction": item.get("interaction").cloned().unwrap_or_else(|| json!("background"))}))
            .collect();
        compact_scenes.push(json!({
            "scene_number": normalized.get("scene_number").cloned().unwrap_or(Value::Null),
            "label": normalized.get("label").cloned().unwrap_or(Value::Null),
            "lyric_section": normalized.get("lyric_section").cloned().unwrap_or_else(|| json!("")),
            "lyrics": text_or_empty(normalized.get("lyrics")).chars().take(500).collect::<String>(),
            "mapped_extras": extras,
        }));
    }
    let user_arc = text_or_empty(story_layer.get("user_story_arc"));
    if lyrics.is_empty() && compact_scenes.is_empty() && user_arc.is_empty() {
        return Err("Lyrics, scene lyrics, or a user story arc are required to create a story brief.".into());
    }
    let instruction = story_brief_instruction(&lyric_story_strength_guidance(&story_layer), &user_arc, &lyrics, &dumps(&Value::Array(compact_scenes)));
    let call = TextCall::new(instruction, number(payload, "temperature", 0.35)?, number(payload, "top_p", 0.90)?, whole(payload, "max_new_tokens", 800)?, "Storyboard Story Brief Gemma").paragraphs();
    let (reply, info) = llm.text(call).await?;
    let brief = text_s(&reply, 4000);
    if brief.is_empty() {
        return Err("Gemma returned an empty story brief.".into());
    }
    let mut out = Obj::new();
    out.insert("story_brief".into(), json!(brief));
    out.extend(run_fields(&info));
    Ok(Value::Object(out))
}

/// A subject or location card with the name and description cut like the pack does, added once for each name.
fn add_card(list: &mut Vec<Value>, seen: &mut HashSet<String>, card: &Obj) {
    let name = text(card.get("name"), 120);
    let description = text(card.get("description"), 500);
    let key = name.to_lowercase();
    if !key.is_empty() && seen.insert(key) {
        list.push(json!({"name": name, "description": description}));
    }
}

/// `StoryArcFormatError` and the other failures of the arc, as one value.
fn format_error(message: String, raw: &str, labels: &[String], runner: &str) -> ArcError {
    ArcError::Format(FormatError { message, raw_output: last_chars(raw, 12000), cleaned_output: last_chars(raw, 12000), expected_sections: labels.to_vec(), runner: runner.to_string() })
}

/// Why the reply of a schema-constrained call is not usable: it is not JSON, or JSON that is not an object of sections.
enum Sections {
    Syntax(String),
    Shape(String),
}

/// The reply of a schema-constrained call as "Label: paragraph" text blocks, one for every required heading.
fn schema_text(reply: &str, labels: &[String]) -> Result<String, Sections> {
    let parsed: Value = serde_json::from_str(reply).map_err(|e| Sections::Syntax(e.to_string()))?;
    let Value::Object(sections) = parsed else { return Err(Sections::Shape(format!("'{}' object has no attribute 'get'", py_type_name(&parsed)))) };
    Ok(labels.iter().map(|label| format!("{label}:\n{}", text_or_empty(sections.get(label)).trim())).collect::<Vec<_>>().join("\n\n"))
}

/// Pass 2: one short note for every scene, written under its section. A scene whose call fails is skipped, and a section without any note keeps
/// its pass 1 paragraph.
#[allow(clippy::too_many_arguments)]
async fn expand_scene_entries(
    llm: &dyn Llm,
    payload: &Obj,
    arc_text: &str,
    section_scenes: &[(String, Vec<Value>)],
    scene_casts: &HashMap<i64, Vec<Value>>,
    cast_guards: &HashMap<i64, CastGuard>,
    camera_flow: &str,
    camera_motion_speed: i64,
    character_motion: i64,
    entry_words: i64,
    seed: &str,
    runner_label: &str,
) -> String {
    let labels: Vec<String> = section_scenes.iter().map(|(label, _)| label.clone()).collect();
    let sections = split_sections(arc_text, &labels);
    if sections.len() != labels.len() {
        return arc_text.to_string();
    }
    let timeline_notes = story_arc_timeline_notes(payload);
    let leading_label = rx(r"(?i)^(?:scene\s*\d+\s*(?:\([^)]*\))?\s*[:\x{2014}\x{2013}-]\s*)");
    let mut blocks: Vec<String> = Vec::new();
    for (label, rows) in section_scenes {
        let paragraph = sections.iter().find(|(l, _)| l == label).map(|(_, body)| body.clone()).unwrap_or_default();
        let mut entries: Vec<String> = Vec::new();
        let mut previous_entry = String::new();
        for row in rows {
            let number = row.get("scene_number").and_then(Value::as_i64).unwrap_or_default();
            let cast_names: Vec<String> = scene_casts.get(&number).map(|cast| cast.iter().map(|item| text_or_empty(item.get("name"))).collect()).unwrap_or_default();
            let guard = cast_guards.get(&number);
            let context = guard.map_or_else(|| paragraph.clone(), |g| strip_cast_leaks(&paragraph, g, false));
            let mut base_instruction = scene_entry_instruction(&EntryInput {
                section_label: label,
                section_text: &context,
                scene_number: number,
                location_name: &text_or_empty(row.get("location")),
                location_description: &text_or_empty(row.get("location_description")),
                cast_wall: &guard.map(cast_wall_text).unwrap_or_default(),
                cast_names: &cast_names,
                lyric: &text_or_empty(row.get("lyric")),
                previous_entry: &previous_entry,
                camera_flow,
                camera_motion_speed,
                character_motion_speed: character_motion,
                word_limit: entry_words,
                story_arc_seed: seed,
            });
            let scene_notes: Vec<Value> = timeline_notes.iter().filter(|note| array(note.get("scene_numbers")).iter().any(|n| n.as_i64() == Some(number))).cloned().collect();
            if !scene_notes.is_empty() {
                base_instruction.push_str(&timeline_note_story_instruction(&dumps(&Value::Array(scene_notes))));
            }
            let mut body = String::new();
            let mut bad_terms: Vec<String> = Vec::new();
            let mut failed = false;
            for _attempt in 0..2 {
                let mut instruction = base_instruction.clone();
                if !bad_terms.is_empty() {
                    instruction.push_str(&format!(
                        "\n\nThe previous note wrongly used these words: {}. Do not use them. Do not add any person who is not in the people list. Describe only what the camera sees.",
                        bad_terms.join(", ")
                    ));
                }
                let call = TextCall::new(instruction, 0.4, 0.9, (entry_words * 3).max(200), &format!("Storyboard Story Arc Scene {number} {runner_label}")).paragraphs();
                let reply = match llm.text(call).await {
                    Ok((reply, _)) => reply,
                    Err(error) => {
                        eprintln!("[VRGDG-RS Story Layer] Story Arc scene {number} note failed and was skipped: {error}");
                        failed = true;
                        break;
                    }
                };
                let one_line = h3_core::pyval::collapse_ws(&text_s(&reply, 1500));
                body = leading_label.replace(&one_line, "").trim().to_string();
                let mut terms = guard.map(|g| cast_leaks(&body, g, true)).unwrap_or_default();
                terms.extend(feeling_word_hits(&body, 2));
                terms.sort();
                terms.dedup();
                bad_terms = terms;
                if bad_terms.is_empty() {
                    break;
                }
            }
            if failed {
                continue;
            }
            if let Some(g) = guard.filter(|g| !cast_leaks(&body, g, true).is_empty()) {
                let mut stripped = strip_cast_leaks(&body, g, true);
                if stripped.split_whitespace().count() < 12 {
                    stripped = strip_cast_leaks(&body, g, false);
                }
                body = stripped;
            }
            if body.is_empty() {
                continue;
            }
            let body = cap_words(&body, entry_words);
            let location = text_or_empty(row.get("location"));
            let prefix = if location.is_empty() { format!("Scene {number}") } else { format!("Scene {number} ({location})") };
            previous_entry = body.clone();
            entries.push(format!("{prefix} \u{2014} {body}"));
        }
        blocks.push(format!("{label}:\n{}", if entries.is_empty() { paragraph } else { entries.join("\n") }));
    }
    blocks.join("\n\n")
}

/// The song's own text for the arc: the project's `full_lyrics.txt`, else the Line Mapping lyrics, else the timeline's.
fn project_lyrics(payload: &Obj) -> String {
    let given = pick([payload.get("project_folder"), payload.get("projectFolder")]);
    let Some(raw) = given.map(|v| text_or_empty(Some(v)).trim().trim_matches('"').to_string()) else { return String::new() };
    let path = h3_core::abspath::absolute(&raw).unwrap_or_else(|_| raw.into()).join("project_context").join("full_lyrics.txt");
    if !path.is_file() {
        return String::new();
    }
    std::fs::read(&path).map(|bytes| text_s(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}'), 40000)).unwrap_or_default()
}

/// `_build_story_layer_arc`: the story arc, one paragraph for each lyric section and, at the Detailed and Rich levels, one note for each scene.
pub async fn build_story_layer_arc(llm: &dyn Llm, payload: &Obj) -> Result<Value, ArcError> {
    let seed_value = pick([payload.get("story_arc_seed"), payload.get("storyArcSeed"), payload.get("seed")]);
    if let Some(script) = authoritative_script(payload) {
        let (premise, info) = short_film_story_text(llm, payload, &script, "premise").await?;
        let mut out = Obj::new();
        out.insert("story_arc".into(), json!(premise));
        out.insert("lyrics_source".into(), json!("Authoritative Script Mapper import"));
        out.insert("story_arc_seed".into(), json!(text(seed_value, 80)));
        out.extend(run_fields(&info));
        out.insert("authoritative_script_used".into(), json!(true));
        return Ok(Value::Object(out));
    }
    let storyboard = payload.get("storyboard").and_then(Value::as_object).cloned().unwrap_or_default();
    let timeline_lyrics = text(pick([payload.get("lyrics"), payload.get("lyrics_text")]), 40000);
    let line_mapping_lyrics = text(pick([payload.get("line_mapping_lyrics"), payload.get("lineMappingLyrics")]), 40000);
    let prompt_creator_lyrics = project_lyrics(payload);
    // project_context/full_lyrics.txt is the song the user pasted. The timeline's lyrics hold detected instrumental gaps, so they are the last resort.
    let (lyrics, lyrics_source) = if !prompt_creator_lyrics.is_empty() {
        (prompt_creator_lyrics.clone(), "project full_lyrics.txt")
    } else if !line_mapping_lyrics.is_empty() {
        (line_mapping_lyrics.clone(), "Line Mapping reference lyrics")
    } else {
        (timeline_lyrics, "timeline scene lyrics")
    };
    let lyric_sections = parse_lyric_sections(&lyrics, line_mapping_lyrics.is_empty() && prompt_creator_lyrics.is_empty());
    let labels: Vec<String> = lyric_sections.iter().map(|(label, _)| label.clone()).collect();
    let detail = normalize_story_arc_detail(pick([payload.get("story_arc_detail"), payload.get("storyArcDetail"), storyboard.get("story_arc_detail")]));
    let (per_section_max, total_budget) = story_arc_detail_profile(&detail);
    let word_limit = section_word_limit(labels.len(), per_section_max, total_budget);
    // Larger detail levels need more output tokens and a higher raw text cap than the defaults.
    let word_estimate = word_limit * labels.len().max(1) as i64;
    let arc_tokens = whole(payload, "max_new_tokens", 2400)?.max((word_estimate as f64 * 1.6) as i64 + 400);
    let char_cap = 14000.max((word_estimate * 9) as usize);
    let story_layer = normalize_story_layer(pick([payload.get("story_layer"), payload.get("storyLayer"), storyboard.get("story_layer")]));
    let story_idea = text(pick([payload.get("story_idea"), payload.get("storyIdea"), story_layer.get("overall_story_idea")]), 4000);
    let seed = text(seed_value, 80);
    let previous_arc = text(pick([payload.get("previous_story_arc"), payload.get("previousStoryArc")]), 5000);
    let style_theme = text(pick([payload.get("style_theme"), payload.get("styleTheme"), payload.get("theme")]), 1600);
    let performance_style = text(pick([payload.get("performance_style"), payload.get("performanceStyle"), storyboard.get("performance_style_default")]), 200);
    let facial_performance = text(pick([payload.get("facial_performance"), payload.get("facialPerformance"), storyboard.get("facial_performance_default")]), 200);
    let camera_flow = text(pick([payload.get("camera_flow"), payload.get("cameraFlow"), storyboard.get("camera_flow")]), 200);
    let camera_motion_speed = speed_value(first_present(payload, &["camera_motion_speed", "cameraMotionSpeed"], storyboard.get("camera_motion_speed")), 4);
    let character_motion = speed_value(first_present(payload, &["character_motion", "characterMotion", "character_motion_speed", "characterMotionSpeed"], storyboard.get("character_motion_speed")), 7);

    let mut compact_scenes: Vec<Value> = Vec::new();
    let mut scene_rows: Vec<Value> = Vec::new();
    let mut scene_casts: HashMap<i64, Vec<Value>> = HashMap::new();
    let mut scene_extras: HashMap<i64, Vec<String>> = HashMap::new();
    let (mut subjects, mut locations): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
    let (mut seen_subjects, mut seen_locations): (HashSet<String>, HashSet<String>) = (HashSet::new(), HashSet::new());
    for (index, scene) in array(payload.get("scenes")).iter().take(160).enumerate() {
        if !scene.is_object() {
            continue;
        }
        let normalized = normalize_storyboard_scene(scene, index as i64 + 1);
        let number = normalized.get("scene_number").and_then(Value::as_i64).unwrap_or_default();
        let scene_cast: Vec<Value> = if normalized.get("no_character_present").is_some_and(truthy) {
            Vec::new()
        } else {
            array(normalized.get("subject_refs"))
                .iter()
                .filter_map(Value::as_object)
                .filter(|subject| !text(subject.get("name"), 120).is_empty())
                .map(|subject| json!({"name": text(subject.get("name"), 120), "description": text(subject.get("description"), 500)}))
                .collect()
        };
        let cast_names: Vec<Value> = scene_cast.iter().map(|item| item["name"].clone()).collect();
        scene_extras.insert(number, array(normalized.get("extra_subjects")).iter().filter_map(Value::as_object).map(|item| text_or_empty(item.get("name")).trim().to_string()).filter(|n| !n.is_empty()).collect());
        compact_scenes.push(json!({
            "scene_number": number,
            "label": normalized.get("label").cloned().unwrap_or(Value::Null),
            "lyric_section": normalized.get("lyric_section").cloned().unwrap_or_else(|| json!("")),
            "cast": if cast_names.is_empty() { json!("no characters") } else { Value::Array(cast_names.clone()) },
        }));
        scene_casts.insert(number, scene_cast);
        for subject in array(normalized.get("subject_refs")).iter().filter_map(Value::as_object) {
            add_card(&mut subjects, &mut seen_subjects, subject);
        }
        let location = normalized.get("location_ref").and_then(Value::as_object);
        if let Some(card) = location {
            add_card(&mut locations, &mut seen_locations, card);
        }
        scene_rows.push(json!({
            "scene_number": number,
            "lyric_section": normalized.get("lyric_section").cloned().unwrap_or_else(|| json!("")),
            "location": location.map_or_else(String::new, |l| text(l.get("name"), 120)),
            "location_description": location.map_or_else(String::new, |l| text(l.get("description"), 400)),
            "lyric": text(normalized.get("lyrics"), 90),
            "cast": cast_names,
        }));
    }
    if let Some(builder) = pick([payload.get("reference_builder"), payload.get("referenceBuilder")]).and_then(Value::as_object) {
        for subject in array(builder.get("subjects")).iter().filter_map(Value::as_object) {
            add_card(&mut subjects, &mut seen_subjects, subject);
        }
        for location in array(builder.get("locations")).iter().filter_map(Value::as_object) {
            add_card(&mut locations, &mut seen_locations, location);
        }
    }
    // A scene note may only name the people selected for that scene, plus its mapped extras.
    let cast_guards: HashMap<i64, CastGuard> = scene_casts.iter().map(|(number, cast)| (*number, build_cast_guard(&subjects, cast, scene_extras.get(number).map_or(&[][..], Vec::as_slice)))).collect();
    // Pass 2 needs the timeline scenes matched to the lyric sections.
    let section_scenes = scene_map(&scene_rows, &labels);
    let scenes_enabled = !section_scenes.is_empty() && scene_entries_enabled(payload, &storyboard, &detail);
    let listed = |items: &[Value], most: usize| if items.is_empty() { "[not provided]".to_string() } else { dumps(&Value::Array(items.iter().take(most).cloned().collect())) };
    let compact_json = listed(&compact_scenes, usize::MAX);
    let (subjects_json, locations_json) = (listed(&subjects, 24), listed(&locations, 40));
    let strength_guidance = lyric_story_strength_guidance(&story_layer);
    let brief = text_or_empty(story_layer.get("song_story_brief"));
    let mut instruction = story_arc_instruction(&ArcInput {
        required_section_labels: &labels,
        section_word_limit: word_limit,
        story_arc_seed: &seed,
        camera_flow: &camera_flow,
        camera_motion_speed,
        character_motion_speed: character_motion,
        performance_style: &performance_style,
        facial_performance: &facial_performance,
        lyric_story_strength_guidance_text: &strength_guidance,
        story_idea: &story_idea,
        previous_story_arc: &previous_arc,
        style_theme: &style_theme,
        lyrics_source,
        lyrics: &lyrics,
        compact_scenes_json: &compact_json,
        subjects_json: &subjects_json,
        locations_json: &locations_json,
        song_story_brief: &brief,
    });
    let timeline_notes = story_arc_timeline_notes(payload);
    if !timeline_notes.is_empty() {
        instruction.push_str(&timeline_note_story_instruction(&dumps(&Value::Array(timeline_notes))));
    }
    let runner_label = display_name(payload, false);
    let (temperature, top_p) = (number(payload, "temperature", 0.45)?, number(payload, "top_p", 0.92)?);
    let call_label = format!("Storyboard Story Arc {runner_label}");
    // Runners that take a schema put every required heading into a JSON key, so the headings cannot drift. When that reply is unusable the plain
    // text call below runs as before.
    let structured = runner_kind(payload) != "llm_api";
    let mut text_out = String::new();
    let mut run_info = Obj::new();
    if !labels.is_empty() && structured {
        let mut call = TextCall::new(arc_json_retry_instruction(&instruction), temperature, top_p, arc_tokens, &call_label).paragraphs();
        call.json_schema = Some(arc_schema(&labels));
        match llm.text(call).await {
            Ok((reply, info)) => match schema_text(reply.trim(), &labels) {
                Ok(joined) => {
                    text_out = joined;
                    run_info = info;
                }
                Err(Sections::Syntax(error) | Sections::Shape(error)) => eprintln!("[VRGDG-RS Story Layer] Structured Story Arc reply was not usable, using plain text: {error}"),
            },
            Err(error) => eprintln!("[VRGDG-RS Story Layer] Structured Story Arc reply was not usable, using plain text: {error}"),
        }
    }
    if text_out.is_empty() {
        let (reply, info) = llm.text(TextCall::new(instruction.clone(), temperature, top_p, arc_tokens, &call_label).paragraphs()).await?;
        text_out = reply;
        run_info = info;
    }
    let mut text_out = text_s(&text_out, char_cap);
    if text_out.is_empty() {
        return Err(ArcError::Failed(format!("{runner_label} returned an empty story arc.")));
    }
    text_out = match normalize_output(&text_out, &labels, word_limit, &runner_label) {
        Ok(normalized) => normalized,
        Err(first_error) => {
            if labels.is_empty() {
                return Err(ArcError::Failed(first_error));
            }
            let retry_instruction = if structured { arc_json_retry_instruction(&instruction) } else { arc_format_retry_instruction(&labels, &instruction) };
            let retry_seed = payload.get("seed").filter(|v| truthy(v)).map_or(Some(0), to_int).map_or(1, |current| (current + 1).rem_euclid(2147483647));
            let mut call = TextCall::new(retry_instruction, 0.2, 0.85, arc_tokens, &format!("{call_label} format retry")).paragraphs();
            call.seed = Some(retry_seed);
            if structured {
                call.json_schema = Some(arc_schema(&labels));
            }
            let (retry_reply, retry_info) = llm.text(call).await?;
            let lost = |message: &str, shown: &str| {
                let shown = if shown.is_empty() { text_out.as_str() } else { shown };
                format_error(format!("{runner_label} could not preserve the lyric-section structure after an automatic format retry. {message}"), shown, &labels, &runner_label)
            };
            let joined = if structured {
                match schema_text(&retry_reply, &labels) {
                    Ok(joined) => joined,
                    Err(Sections::Syntax(message)) => return Err(lost(&message, &retry_reply)),
                    Err(Sections::Shape(message)) => return Err(ArcError::Failed(message)),
                }
            } else {
                retry_reply
            };
            let cleaned = text_s(&joined, char_cap);
            match normalize_output(&cleaned, &labels, word_limit, &runner_label) {
                Ok(normalized) => {
                    run_info = retry_info;
                    normalized
                }
                Err(message) => return Err(lost(&message, &cleaned)),
            }
        }
    };
    if scenes_enabled {
        text_out = expand_scene_entries(llm, payload, &text_out, &section_scenes, &scene_casts, &cast_guards, &camera_flow, camera_motion_speed, character_motion, story_arc_entry_words(&detail), &seed, &runner_label).await;
    }
    let guards: HashMap<i64, CastGuard> = cast_guards;
    let names: HashMap<i64, Vec<String>> = scene_casts.iter().map(|(number, cast)| (*number, cast.iter().map(|item| text_or_empty(item.get("name"))).collect())).collect();
    let text_out = strip_story_arc_entry_leaks(&text_out, &guards, &names);
    let mut out = Obj::new();
    out.insert("story_arc".into(), json!(text_out));
    out.insert("lyrics_source".into(), json!(lyrics_source));
    out.insert("story_arc_seed".into(), json!(seed));
    out.extend(run_fields(&run_info));
    Ok(Value::Object(out))
}
