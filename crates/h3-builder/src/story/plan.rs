//! Story Mode acts and scene plan with the selected LLM runner. Port of `storyboard/story_plan.py`.
//!
//! `build_story_acts` gives each chapter a run of timeline scenes (proportional to its paragraphs) and asks the LLM to split that run into
//! acts. `build_story_scene_plan` asks, act by act, for each scene's story slice, spot, mood and performance. Code checks and repairs every
//! reply, so a weak reply never leaves a scene without an act or a plan entry. Both return the story layer fields to save. Saving is the
//! caller's job.

use std::collections::{HashMap, HashSet};

use h3_core::pyval::{round_half_even, text_or_empty, to_float, to_int, truthy, Obj};
use h3_core::story::acts::{
    allocate_chapter_scenes, chapter_ranges, enforce_spot_runs, fallback_act_ranges, keep_notes_inside_acts, repair_act_ranges, validate_story_acts, DEFAULT_SPOT_MIN_SCENES,
};
use h3_core::story::helpers::dialogue_reference_catalog;
use h3_core::story::mode::{
    act_user_text, normalize_plan_performance, normalize_story_acts, normalize_story_book, normalize_story_video_style, plan_to_value, resolve_story_continuations, text,
};
use h3_core::story::notes::story_arc_timeline_notes;
use h3_llm::locations::extract_json_object;
use h3_llm::prompts::constant;
use h3_llm::story_prompts::{story_act_summary_instruction, story_acts_instruction, story_mode_lyric_guidance, story_scene_plan_instruction, ActsInput, PlanInput};
use serde_json::{json, Value};

use super::book::run;
use super::context::{cast_lines, layer as request_layer, made_place_cards, made_places_text, main_cast, match_user_place, scenes as request_scenes, story_cast, story_template, user_places_text};
use crate::prompt_writers::Llm;

type Reply = Result<Value, String>;

const PLAN_NOTE_FIELDS: [(&str, &str); 3] = [("director", "timeline_note"), ("video", "motion_summary"), ("planning", "notes")];

fn schema(name: &str) -> Option<Value> {
    Some(constant("story_mode", name).clone())
}

fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn s(value: Option<&Value>) -> String {
    text_or_empty(value)
}

fn scene_id(scene: &Value, index: usize) -> String {
    let id = scene.get("id").filter(|v| truthy(v));
    match id {
        Some(v) => text_or_empty(Some(v)),
        None => format!("scene_{}", index + 1),
    }
}

fn seconds(value: Option<&Value>) -> f64 {
    value.and_then(to_float).unwrap_or(0.0)
}

fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    format!("{}:{:04.1}", (seconds / 60.0).floor() as i64, seconds % 60.0)
}

fn pick<'a>(scene: &'a Value, key: &str, other: &str) -> Option<&'a Value> {
    scene.get(key).or_else(|| scene.get(other))
}

/// `_scene_notes`: the notes a scene carries, by where they came from.
fn scene_notes(scene: &Value) -> Vec<(&'static str, String)> {
    let mut notes = Vec::new();
    for (source, key) in PLAN_NOTE_FIELDS {
        let first = scene.get(key).filter(|v| truthy(v));
        let second = if source == "video" { scene.get("i2v_notes").filter(|v| truthy(v)) } else { None };
        let note = text(first.or(second), 800);
        if !note.is_empty() {
            notes.push((source, note));
        }
    }
    notes
}

fn number_in(note: &Value, number: i64) -> bool {
    array(note.get("scene_numbers")).iter().any(|n| to_float(n).is_some_and(|f| f.trunc() as i64 == number))
}

/// `_scene_line`: one line for a scene in a prompt: its time, length, section, lyric and (with notes) the user's notes.
fn scene_line(number: i64, scene: &Value, timed_notes: &[Value], with_notes: bool) -> String {
    let start = seconds(pick(scene, "timeline_start", "start"));
    let end = seconds(pick(scene, "timeline_end", "end"));
    let lyric = text(scene.get("lyrics").filter(|v| truthy(v)).or_else(|| scene.get("lyric_text")), 300).replace('\n', " / ");
    let instrumental = scene.get("lyric_instrumental").is_some_and(truthy) || lyric.is_empty();
    let mut head = format!("Scene {number} ({}-{}, {:.1} s", clock(start), clock(end), (end - start).max(0.0));
    if scene.get("lyric_section").is_some_and(truthy) {
        head.push_str(&format!(", {}", text(scene.get("lyric_section"), 60)));
    }
    head.push_str(if instrumental { ", instrumental)" } else { ")" });
    let mut parts = vec![head];
    if !lyric.is_empty() && !instrumental {
        parts.push(format!("lyric mood: \"{lyric}\""));
    }
    if with_notes {
        let override_text = text(scene.get("scene_override"), 800);
        if !override_text.is_empty() {
            parts.push(format!("OVERRIDE, always true for this scene: {override_text} (plan the scene to match it, changing any detail of the story, spot or mood that disagrees)"));
        }
        for (source, note) in scene_notes(scene) {
            parts.push(format!("{source} note: {note}"));
        }
        for note in timed_notes {
            if number_in(note, number) {
                parts.push(format!("timed note: {}", s(note.get("note"))));
            }
        }
    }
    parts.join(" | ")
}

/// `_names`: lowercased names from an LLM list of names.
fn names_of(value: Option<&Value>) -> Vec<String> {
    array(value).iter().map(|n| text_or_empty(Some(n)).trim().to_lowercase()).filter(|n| !n.is_empty()).collect()
}

/// `_user_direction`: the user's own direction for a scene (its note and a cast they set), added to its scene line.
fn user_direction(entry: &Obj, names_by_id: &[(String, String)]) -> String {
    let mut parts = Vec::new();
    let ids = array(entry.get("cast_ids"));
    if text_or_empty(entry.get("cast_source")) == "manual" && !ids.is_empty() {
        let names: Vec<String> = ids.iter().map(|id| {
            let id = text_or_empty(Some(id));
            names_by_id.iter().rev().find(|(i, _)| *i == id).map_or(id.clone(), |(_, n)| n.clone())
        }).collect();
        parts.push(format!("user direction, characters in this scene: {}", names.join(", ")));
    }
    if entry.get("user_note").is_some_and(truthy) {
        parts.push(format!("user direction: {}", s(entry.get("user_note"))));
    }
    parts.iter().map(|p| format!(" | {p}")).collect()
}

/// `_note_ranges`: the first and last scene of each timed note.
fn note_ranges(timed_notes: &[Value]) -> Vec<(i64, i64)> {
    let mut ranges = Vec::new();
    for note in timed_notes {
        let mut numbers: Vec<i64> = array(note.get("scene_numbers")).iter().filter_map(to_int).collect();
        numbers.sort_unstable();
        if let (Some(first), Some(last)) = (numbers.first(), numbers.last()) {
            ranges.push((*first, *last));
        }
    }
    ranges
}

fn settings_by_id(layer: &Value) -> HashMap<String, Value> {
    array(layer.get("story_settings")).into_iter().filter(|i| i.is_object()).map(|i| (s(i.get("id")), i)).collect()
}

fn setting_name(settings: &HashMap<String, Value>, id: &str) -> String {
    settings.get(id).map(|i| s(i.get("name"))).unwrap_or_default()
}

/// `_cast_ids_by_name`: card id by lowercased name, for every card that has a name.
fn cast_ids_by_name(payload: &Obj) -> HashMap<String, String> {
    let (subjects, _) = dialogue_reference_catalog(payload);
    subjects
        .iter()
        .filter(|subject| subject.get("name").is_some_and(truthy))
        .map(|subject| {
            let (name, id) = (s(subject.get("name")), s(subject.get("id")));
            (name.to_lowercase(), if id.is_empty() { name } else { id })
        })
        .collect()
}

/// `_parse`: the objects under `key` in a JSON reply. Nothing when the reply is not usable.
fn parse_items(raw: &str, key: &str) -> Vec<Value> {
    match extract_json_object(raw) {
        Ok(Value::Object(map)) => array(map.get(key)).into_iter().filter(Value::is_object).collect(),
        _ => Vec::new(),
    }
}

fn chapter_scene_ranges(layer: &Value, chapters: &[Value], scene_ids: &[String]) -> Vec<Option<(i64, i64)>> {
    let number_by_id: HashMap<&String, i64> = scene_ids.iter().enumerate().map(|(i, id)| (id, i as i64 + 1)).collect();
    let mut by_chapter: HashMap<String, (i64, i64)> = HashMap::new();
    let acts = array(layer.get("story_acts"));
    for act in &acts {
        let numbers: Vec<i64> = array(act.get("scene_ids")).iter().filter_map(|sid| number_by_id.get(&text_or_empty(Some(sid))).copied()).collect();
        if let (Some(min), Some(max)) = (numbers.iter().min(), numbers.iter().max()) {
            let key = s(act.get("chapter_id"));
            let (low, high) = by_chapter.get(&key).copied().unwrap_or((*min, *max));
            by_chapter.insert(key, (low.min(*min), high.max(*max)));
        }
    }
    if !chapters.is_empty() && chapters.iter().all(|c| by_chapter.contains_key(&s(c.get("id")))) && validate_story_acts(&acts, chapters, scene_ids).is_empty() {
        return chapters.iter().map(|c| by_chapter.get(&s(c.get("id"))).copied()).collect();
    }
    let counts: Vec<i64> = chapters.iter().map(|c| array(c.get("paragraphs")).len() as i64).collect();
    chapter_ranges(&allocate_chapter_scenes(&counts, scene_ids.len() as i64))
}

fn timed_notes_of(payload: &Obj, scenes: &[Value]) -> Vec<Value> {
    let numbered: Vec<Value> = scenes
        .iter()
        .enumerate()
        .map(|(index, scene)| {
            let mut map = scene.as_object().cloned().unwrap_or_default();
            map.insert("scene_number".into(), json!(index + 1));
            Value::Object(map)
        })
        .collect();
    let mut input = Obj::new();
    input.insert("scenes".into(), Value::Array(numbered));
    input.insert("timeline_markers".into(), payload.get("timeline_markers").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([])));
    story_arc_timeline_notes(&input)
}

fn run_fields(info: &Obj) -> Obj {
    let mut fields = Obj::new();
    fields.insert("runner".into(), info.get("runner").cloned().unwrap_or_else(|| json!("")));
    fields.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    fields
}

/// `build_story_acts`: acts for every chapter (or only `chapter_id`). The scene plan of rebuilt chapters is cleared.
pub async fn build_story_acts(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = request_layer(payload);
    let chapters = array(normalize_story_book(layer.get("story_book")).get("chapters"));
    if chapters.is_empty() {
        return Err("Write the story first; acts are made from its chapters.".into());
    }
    let scenes = request_scenes(payload);
    if scenes.is_empty() {
        return Err("Create the timeline scenes first.".into());
    }
    let scene_ids: Vec<String> = scenes.iter().enumerate().map(|(i, sc)| scene_id(sc, i)).collect();
    let ranges = chapter_scene_ranges(&layer, &chapters, &scene_ids);
    let only = text_or_empty(payload.get("chapter_id").filter(|v| truthy(v))).trim().to_string();
    if !only.is_empty() && !chapters.iter().any(|c| s(c.get("id")) == only) {
        return Err(format!("Chapter '{only}' is not in the story book."));
    }
    let timed_notes = timed_notes_of(payload, &scenes);
    let settings = settings_by_id(&layer);
    let cast_text = cast_lines(payload);
    let cast_ids = cast_ids_by_name(payload);
    let mut kept: HashMap<String, Vec<Value>> = HashMap::new();
    if !only.is_empty() {
        // A full rebuild replaces every chapter's acts.
        for act in array(layer.get("story_acts")) {
            kept.entry(s(act.get("chapter_id"))).or_default().push(act);
        }
    }
    let mut built: HashMap<String, Vec<Value>> = HashMap::new();
    let mut info = Obj::new();
    let todo: Vec<usize> = chapters.iter().enumerate().filter(|(i, c)| ranges[*i].is_some() && (only.is_empty() || s(c.get("id")) == only)).map(|(i, _)| i).collect();
    for index in todo {
        let chapter = &chapters[index];
        let (first, last) = ranges[index].expect("a chapter in todo has a range");
        let chapter_notes: Vec<Value> = timed_notes
            .iter()
            .filter(|note| array(note.get("scene_numbers")).iter().any(|n| to_int(n).is_some_and(|n| first <= n && n <= last)))
            .map(|note| {
                let mut map = note.as_object().cloned().unwrap_or_default();
                let inside: Vec<Value> = array(note.get("scene_numbers")).into_iter().filter(|n| to_int(n).is_some_and(|n| first <= n && n <= last)).collect();
                map.insert("scene_numbers".into(), Value::Array(inside));
                Value::Object(map)
            })
            .collect();
        let paragraphs: Vec<String> = array(chapter.get("paragraphs")).iter().map(|p| text_or_empty(Some(p))).collect();
        let mut proposals: Vec<Value> = Vec::new();
        if last > first {
            let scenes_text = (first..=last).map(|n| scene_line(n, &scenes[n as usize - 1], &chapter_notes, false)).collect::<Vec<_>>().join("\n");
            let notes_text = chapter_notes
                .iter()
                .filter_map(|note| {
                    let numbers = array(note.get("scene_numbers"));
                    let (low, high) = (numbers.first()?, numbers.last()?);
                    Some(format!("- scenes {}-{}: {}", text_or_empty(Some(low)), text_or_empty(Some(high)), s(note.get("note"))))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let setting_id = s(chapter.get("setting_id"));
            let template = story_template(payload, "story_mode_acts");
            let (places, made) = (user_places_text(payload), made_places_text(payload, &setting_id));
            let chapter_title = s(chapter.get("title"));
            let chapter_text = paragraphs.join("\n\n");
            let instruction = story_acts_instruction(&ActsInput {
                chapter_number: index as i64 + 1,
                chapter_title: &chapter_title,
                setting_name: &setting_name(&settings, &setting_id),
                chapter_text: &chapter_text,
                scenes_text: &scenes_text,
                cast_text: &cast_text,
                notes_text: &notes_text,
                target_acts: 1.max(round_half_even((last - first + 1) as f64 / 4.0)),
                template: Some(&template),
                places_text: &places,
                made_places_text: &made,
            });
            // A failed chapter falls back to one act per paragraph.
            match run(llm, payload, instruction, "Story Mode Acts", 2400, 0.4, schema("STORY_ACTS_SCHEMA")).await {
                Ok((raw, run_info)) => {
                    info = run_info;
                    proposals = parse_items(&raw, "acts");
                }
                Err(error) => eprintln!("[VRGDG-RS Story Mode] Acts for chapter {} fell back to paragraphs: {error}", index + 1),
            }
        }
        let mut by_start: Vec<(i64, Value)> = Vec::new();
        for item in &proposals {
            if let Some(start) = item.get("first_scene").and_then(to_int) {
                if !by_start.iter().any(|(s, _)| *s == start) {
                    by_start.push((start, item.clone()));
                }
            }
        }
        let act_ranges = if by_start.is_empty() {
            fallback_act_ranges(paragraphs.len() as i64, first, last)
        } else {
            let starts: Vec<Option<i64>> = by_start.iter().map(|(start, _)| Some(*start)).collect();
            repair_act_ranges(&starts, first, last)
        };
        let act_ranges = keep_notes_inside_acts(&act_ranges, &note_ranges(&chapter_notes));
        let mut acts = Vec::new();
        for (part, (a, b)) in act_ranges.iter().enumerate() {
            let item = by_start.iter().find(|(start, _)| start == a).map(|(_, item)| item.clone()).unwrap_or_else(|| json!({}));
            let fallback_text = if paragraphs.is_empty() { String::new() } else { paragraphs[part.min(paragraphs.len() - 1)].clone() };
            let names = names_of(item.get("cast"));
            let title = match item.get("title").filter(|v| truthy(v)) {
                Some(v) => text(Some(v), 200),
                None => text(Some(&json!(format!("{} - part {}", s(chapter.get("title")), part + 1))), 200),
            };
            let summary = match item.get("summary").filter(|v| truthy(v)) {
                Some(v) => text(Some(v), 2000),
                None => text(Some(&json!(fallback_text)), 2000),
            };
            let ids: Vec<Value> = names.iter().filter_map(|n| cast_ids.get(n)).map(|id| json!(id)).collect();
            let slice: Vec<Value> = scene_ids.iter().skip((*a - 1).max(0) as usize).take((*b - (*a - 1)).max(0) as usize).map(|id| json!(id)).collect();
            acts.push(json!({"chapter_id": s(chapter.get("id")), "title": title, "summary": summary, "cast_ids": ids, "scene_ids": slice}));
        }
        built.insert(s(chapter.get("id")), acts);
    }
    let mut ordered: Vec<Value> = Vec::new();
    for chapter in &chapters {
        let id = s(chapter.get("id"));
        match built.get(&id) {
            Some(acts) => ordered.extend(acts.iter().cloned()),
            None => ordered.extend(kept.get(&id).cloned().unwrap_or_default()),
        }
    }
    let story_acts: Vec<Value> = ordered
        .into_iter()
        .enumerate()
        .map(|(index, act)| {
            let mut map = act.as_object().cloned().unwrap_or_default();
            map.insert("id".into(), json!(format!("act_{}", index + 1)));
            Value::Object(map)
        })
        .collect();
    let rebuilt: HashSet<String> = built.values().flatten().flat_map(|act| array(act.get("scene_ids"))).map(|sid| text_or_empty(Some(&sid))).collect();
    let mut act_by_scene: HashMap<String, String> = HashMap::new();
    for act in &story_acts {
        for sid in array(act.get("scene_ids")) {
            act_by_scene.insert(text_or_empty(Some(&sid)), s(act.get("id")));
        }
    }
    let plan: Vec<Value> = array(layer.get("story_scene_plan"))
        .into_iter()
        .filter(|entry| !rebuilt.contains(&s(entry.get("scene_id"))))
        .map(|entry| {
            let mut map = entry.as_object().cloned().unwrap_or_default();
            let act = act_by_scene.get(&s(map.get("scene_id"))).cloned().unwrap_or_default();
            map.insert("act_id".into(), json!(act));
            Value::Object(map)
        })
        .collect();
    let mut out = Obj::new();
    out.insert("story_layer".into(), json!({"story_acts": story_acts, "story_scene_plan": plan}));
    out.extend(run_fields(&info));
    Ok(Value::Object(out))
}

fn default_performance(scene: &Value) -> &'static str {
    let lyric = text(scene.get("lyrics").filter(|v| truthy(v)).or_else(|| scene.get("lyric_text")), 300);
    if scene.get("lyric_instrumental").is_some_and(truthy) || lyric.is_empty() { "instrumental" } else { "singing" }
}

/// `style_performance`: a scene performance that fits the video style. Spoken never sings, story only never sings or speaks.
pub fn style_performance(performance: &str, style: &str) -> String {
    if style == "spoken" && performance == "singing" {
        return "speaking".into();
    }
    if style == "story_only" && (performance == "singing" || performance == "speaking") {
        return "no_lip_sync".into();
    }
    performance.to_string()
}

/// `_snap_to_made_place`: the made place card a spot names. When place cards exist and none fits, the spot is left blank, so the scene takes
/// the spot before it: the plan never adds a place of its own.
fn snap_to_made_place(made_cards: &[(String, String)], spot: &str) -> String {
    let place = if !spot.is_empty() && !made_cards.is_empty() { match_user_place(made_cards, &[spot]) } else { None };
    match place {
        Some((name, _)) => name,
        None if made_cards.is_empty() => spot.to_string(),
        None => String::new(),
    }
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|i| seen.insert(i.clone())).collect()
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|i| json!(i)).collect())
}

fn plan_text(map: &Obj, key: &str) -> String {
    text_or_empty(map.get(key))
}

/// `build_story_scene_plan`: a plan entry for every scene of every act (or only `act_id`), with spots kept for at least `spot_min_scenes`
/// scenes in a row.
pub async fn build_story_scene_plan(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = request_layer(payload);
    let chapters = array(normalize_story_book(layer.get("story_book")).get("chapters"));
    let scenes = request_scenes(payload);
    let scene_ids: Vec<String> = scenes.iter().enumerate().map(|(i, sc)| scene_id(sc, i)).collect();
    let acts = array(layer.get("story_acts"));
    if acts.is_empty() {
        return Err("Create the acts first; the scene plan is written act by act.".into());
    }
    let problems = validate_story_acts(&acts, &chapters, &scene_ids);
    if !problems.is_empty() {
        return Err(format!("The acts do not match the timeline. Rebuild or fix the acts first: {}", problems.join(" ")));
    }
    let only = text_or_empty(payload.get("act_id").filter(|v| truthy(v))).trim().to_string();
    if !only.is_empty() && !acts.iter().any(|a| s(a.get("id")) == only) {
        return Err(format!("Act '{only}' does not exist."));
    }
    // Replan only these scenes (Scenes tab, scene by scene). The LLM still sees the whole act. Other scenes keep their plan.
    let only_scenes: HashSet<String> = array(payload.get("scene_ids")).iter().map(|i| text_or_empty(Some(i))).filter(|i| !i.trim().is_empty()).collect();
    let spot_min = payload
        .get("spot_min_scenes")
        .filter(|v| truthy(v))
        .or_else(|| layer.get("story_spot_min_scenes").filter(|v| truthy(v)))
        .map_or(Some(DEFAULT_SPOT_MIN_SCENES), |v| to_float(v).filter(|f| f.is_finite()).map(|f| f.trunc() as i64))
        .map_or(DEFAULT_SPOT_MIN_SCENES, |n| n.max(1));
    let number_by_id: HashMap<&String, i64> = scene_ids.iter().enumerate().map(|(i, id)| (id, i as i64 + 1)).collect();
    let timed_notes = timed_notes_of(payload, &scenes);
    let settings = settings_by_id(&layer);
    let chapter_by_id: HashMap<String, &Value> = chapters.iter().map(|c| (s(c.get("id")), c)).collect();
    let guidance = story_mode_lyric_guidance(Some(layer.get("lyric_story_strength").unwrap_or(&json!(5))));
    let cast_text = cast_lines(payload);
    // The scene cast comes only from the story cast. Names are matched without case.
    let mut names_by_id: Vec<(String, String)> = Vec::new();
    for subject in story_cast(payload) {
        if subject.get("id").is_some_and(truthy) {
            let (id, name) = (s(subject.get("id")), s(subject.get("name")));
            match names_by_id.iter_mut().find(|(i, _)| *i == id) {
                Some(entry) => entry.1 = name,
                None => names_by_id.push((id, name)),
            }
        }
    }
    let ids_by_name: HashMap<String, String> = names_by_id.iter().map(|(id, name)| (name.to_lowercase(), id.clone())).collect();
    let name_of = |id: &str| names_by_id.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone());
    let main = main_cast(payload);
    let (main_id, main_name) = main.as_ref().map_or((String::new(), String::new()), |m| (s(m.get("id")), s(m.get("name"))));
    let style = normalize_story_video_style(layer.get("story_video_style"));
    let mut plan: HashMap<String, Obj> = array(layer.get("story_scene_plan")).into_iter().filter_map(|e| e.as_object().cloned()).map(|e| (plan_text(&e, "scene_id"), e)).collect();
    let mut info = Obj::new();
    let todo: Vec<&Value> = acts.iter().filter(|a| only.is_empty() || s(a.get("id")) == only).collect();
    for act in &todo {
        let chapter = chapter_by_id.get(&s(act.get("chapter_id"))).map(|c| (*c).clone()).ok_or_else(|| format!("'{}'", s(act.get("chapter_id"))))?;
        let setting_id = s(chapter.get("setting_id"));
        let setting = setting_name(&settings, &setting_id);
        let made_cards = made_place_cards(payload, &setting_id);
        let mut numbers: Vec<i64> = array(act.get("scene_ids")).iter().filter_map(|sid| number_by_id.get(&text_or_empty(Some(sid))).copied()).collect();
        numbers.sort_unstable();
        let act_names: Vec<String> = array(act.get("cast_ids")).iter().filter_map(|cid| name_of(&text_or_empty(Some(cid)))).collect();
        let mut previous_spot = String::new();
        if numbers[0] > 1 {
            previous_spot = plan.get(&scene_ids[numbers[0] as usize - 2]).map(|e| plan_text(e, "spot")).unwrap_or_default();
        }
        let chapter_scene_ids: HashSet<String> = acts.iter().filter(|o| s(o.get("chapter_id")) == s(act.get("chapter_id"))).flat_map(|o| array(o.get("scene_ids"))).map(|sid| text_or_empty(Some(&sid))).collect();
        let mut known_spots: Vec<String> = dedupe(plan.iter().filter(|(sid, entry)| chapter_scene_ids.contains(*sid) && entry.get("spot").is_some_and(truthy)).map(|(_, entry)| plan_text(entry, "spot")).collect());
        known_spots.sort();
        let scenes_text = numbers
            .iter()
            .map(|n| {
                let entry = plan.get(&scene_ids[*n as usize - 1]).cloned().unwrap_or_default();
                scene_line(*n, &scenes[*n as usize - 1], &timed_notes, true) + &user_direction(&entry, &names_by_id)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let act_summary = [s(act.get("summary")), act_user_text(&normalize_story_acts(Some(&json!([act]))).remove(0))].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n");
        let template = story_template(payload, "story_mode_scene_plan");
        let (places, made) = (user_places_text(payload), made_places_text(payload, &setting_id));
        let chapter_text = array(chapter.get("paragraphs")).iter().map(|p| text_or_empty(Some(p))).collect::<Vec<_>>().join("\n\n");
        let act_title = s(act.get("title"));
        let known = known_spots.join(", ");
        let act_cast = act_names.join(", ");
        let instruction = story_scene_plan_instruction(&PlanInput {
            act_title: &act_title,
            act_summary: &act_summary,
            setting_name: &setting,
            chapter_text: &chapter_text,
            scenes_text: &scenes_text,
            cast_text: &cast_text,
            spot_min_scenes: spot_min,
            previous_spot: &previous_spot,
            lyric_guidance: &guidance,
            known_spots: &known,
            video_style: &style,
            act_cast: &act_cast,
            main_name: &main_name,
            template: Some(&template),
            places_text: &places,
            made_places_text: &made,
        });
        let mut replies: Vec<(i64, Value)> = Vec::new();
        // The act keeps a plan built from its summary when the LLM fails.
        match run(llm, payload, instruction, "Story Mode Scene Plan", 3000, 0.5, schema("STORY_SCENE_PLAN_SCHEMA")).await {
            Ok((raw, run_info)) => {
                info = run_info;
                for item in parse_items(&raw, "scenes") {
                    if let Some(number) = item.get("scene").and_then(to_int) {
                        if !replies.iter().any(|(n, _)| *n == number) {
                            replies.push((number, item));
                        }
                    }
                }
            }
            Err(error) => eprintln!("[VRGDG-RS Story Mode] Scene plan for {} fell back to the act summary: {error}", s(act.get("id"))),
        }
        for number in &numbers {
            let sid = &scene_ids[*number as usize - 1];
            if !only_scenes.is_empty() && !only_scenes.contains(sid) {
                continue;
            }
            let scene = &scenes[*number as usize - 1];
            let item = replies.iter().find(|(n, _)| n == number).map(|(_, i)| i.clone()).unwrap_or_else(|| json!({}));
            let asked = item.get("performance").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!(default_performance(scene)));
            let mut performance = style_performance(&normalize_plan_performance(Some(&asked)), &style);
            if default_performance(scene) == "instrumental" {
                performance = "instrumental".into();
            }
            let mut sources: Vec<String> = scene_notes(scene).iter().map(|(source, _)| source.to_string()).collect();
            if timed_notes.iter().any(|note| number_in(note, *number)) {
                sources.push("timeline".into());
            }
            // A continuation the user set by hand survives a replan, with its direction.
            let old = plan.get(sid).cloned().unwrap_or_default();
            let manual = plan_text(&old, "continuation_source") == "manual";
            // So does a cast the user set by hand. Otherwise the story's cast: names from the cast list only.
            let (scene_cast, cast_source): (Vec<String>, &str) = if plan_text(&old, "cast_source") == "manual" {
                (array(old.get("cast_ids")).iter().map(|c| text_or_empty(Some(c))).collect(), "manual")
            } else {
                let from_reply = dedupe(names_of(item.get("cast")).iter().filter_map(|n| ids_by_name.get(n).cloned()).collect());
                let chosen = if !from_reply.is_empty() {
                    from_reply
                } else {
                    let from_act: Vec<String> = array(act.get("cast_ids")).iter().map(|c| text_or_empty(Some(c))).filter(|c| name_of(c).is_some()).collect();
                    if !from_act.is_empty() { from_act } else if !main_id.is_empty() { vec![main_id.clone()] } else { Vec::new() }
                };
                (chosen, "story")
            };
            let wanted = text_or_empty(item.get("performer").filter(|v| truthy(v))).trim().to_lowercase();
            let mut performer = ids_by_name.get(&wanted).cloned().unwrap_or_default();
            if !scene_cast.contains(&performer) {
                performer = String::new();
            }
            // Singers the user ticked survive a replan, as long as they are still in the scene.
            let (performers, performer_source): (Vec<String>, &str) = if plan_text(&old, "performer_source") == "manual" {
                (array(old.get("performer_ids")).iter().map(|c| text_or_empty(Some(c))).filter(|c| scene_cast.contains(c)).collect(), "manual")
            } else {
                (if performer.is_empty() { Vec::new() } else { vec![performer.clone()] }, "")
            };
            let mut entry = Obj::new();
            entry.insert("scene_id".into(), json!(sid));
            entry.insert("act_id".into(), json!(s(act.get("id"))));
            entry.insert("story_slice".into(), json!(match item.get("story_slice").filter(|v| truthy(v)) { Some(v) => text(Some(v), 1200), None => text(act.get("summary"), 1200) }));
            entry.insert("spot".into(), json!(snap_to_made_place(&made_cards, &text(item.get("spot"), 160))));
            entry.insert("mood".into(), json!(text(item.get("mood"), 200)));
            entry.insert("performance_mode".into(), json!(performance));
            entry.insert("note_sources".into(), strings(&sources));
            entry.insert("continues_previous".into(), json!(manual && old.get("continues_previous").is_some_and(truthy)));
            entry.insert("continuation_source".into(), json!(if manual { "manual" } else { "" }));
            entry.insert("llm_continues".into(), json!(matches!(item.get("continues"), Some(Value::Bool(true)))));
            entry.insert("continuation_direction".into(), if manual { old.get("continuation_direction").cloned().unwrap_or_else(|| json!("")) } else { json!(text(item.get("continuation"), 600)) });
            entry.insert("cast_ids".into(), strings(&scene_cast));
            entry.insert("performer_ids".into(), strings(&performers));
            entry.insert("performer_id".into(), json!(performers.first().cloned().unwrap_or_default()));
            entry.insert("performer_source".into(), json!(performer_source));
            entry.insert("interaction".into(), json!(if scene_cast.len() > 1 { text(item.get("interaction"), 600) } else { String::new() }));
            entry.insert("cast_source".into(), json!(cast_source));
            entry.insert("user_note".into(), old.get("user_note").cloned().unwrap_or_else(|| json!("")));
            plan.insert(sid.clone(), entry);
        }
    }
    // Spots are kept for a minimum run inside each chapter. Scenes with a user note, and (when only one act was rebuilt) scenes outside that
    // act, keep their spot.
    let rebuilt: HashSet<String> = todo.iter().flat_map(|act| array(act.get("scene_ids"))).map(|sid| text_or_empty(Some(&sid))).filter(|sid| only_scenes.is_empty() || only_scenes.contains(sid)).collect();
    for chapter in &chapters {
        let chapter_id = s(chapter.get("id"));
        let mut ids: Vec<String> = acts.iter().filter(|a| s(a.get("chapter_id")) == chapter_id).flat_map(|a| array(a.get("scene_ids"))).map(|sid| text_or_empty(Some(&sid))).collect();
        ids.sort_by_key(|sid| number_by_id.get(sid).copied());
        ids.retain(|sid| plan.contains_key(sid));
        if ids.is_empty() {
            continue;
        }
        let locked: HashSet<usize> = ids.iter().enumerate().filter(|(_, sid)| plan[*sid].get("note_sources").is_some_and(truthy) || !rebuilt.contains(*sid)).map(|(i, _)| i).collect();
        let spots: Vec<String> = ids.iter().map(|sid| plan_text(&plan[sid], "spot")).collect();
        let spots = enforce_spot_runs(&spots, spot_min, &locked);
        // A scene with no usable spot takes the setting, else the first place already made (loaded or built), and only then the act title, so a
        // card is never named "Act 1" while real place cards exist.
        let setting_id = s(chapter.get("setting_id"));
        let made = made_place_cards(payload, &setting_id);
        let fallback_spot = {
            let named = setting_name(&settings, &setting_id);
            if !named.is_empty() { named } else if let Some((first, _)) = made.first().filter(|(first, _)| !first.is_empty()) { first.clone() } else { s(chapter.get("title")) }
        };
        for (sid, spot) in ids.iter().zip(spots) {
            if let Some(entry) = plan.get_mut(sid) {
                entry.insert("spot".into(), json!(if spot.is_empty() { fallback_spot.clone() } else { spot }));
            }
        }
    }
    let timeline: Vec<Value> = scene_ids.iter().zip(&scenes).map(|(sid, scene)| json!({"id": sid, "start": seconds(pick(scene, "timeline_start", "start")), "end": seconds(pick(scene, "timeline_end", "end"))})).collect();
    let entries: Vec<Value> = scene_ids.iter().filter_map(|sid| plan.get(sid)).map(|e| Value::Object(e.clone())).collect();
    let ordered = resolve_story_continuations(Some(&Value::Array(entries)), &timeline, layer.get("story_acts"), layer.get("story_continuation_mode"));
    let mut out = Obj::new();
    out.insert("story_layer".into(), json!({"story_scene_plan": plan_to_value(&ordered)}));
    out.extend(run_fields(&info));
    Ok(Value::Object(out))
}

/// `_extract_summary`: the `summary` text from a JSON reply, or the plain reply when the model skipped the JSON.
fn extract_summary(raw: &str) -> Result<String, String> {
    let data = extract_json_object(raw)?;
    if let Value::Object(map) = &data {
        if !text_or_empty(map.get("summary").filter(|v| truthy(v))).trim().is_empty() {
            return Ok(text_or_empty(map.get("summary")));
        }
    }
    Ok(if raw.trim_start().starts_with('{') { String::new() } else { raw.to_string() })
}

/// `rewrite_act_summary`: rewrite one act's description (`act_id`) so it matches the scenes planned for it and the user's own words. Returns
/// the story layer's `story_acts` with only that act's summary changed. Saving is the caller's job.
pub async fn rewrite_act_summary(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = request_layer(payload);
    let acts = array(layer.get("story_acts"));
    let act_id = text_or_empty(payload.get("act_id").filter(|v| truthy(v))).trim().to_string();
    let Some(act) = acts.iter().find(|a| s(a.get("id")) == act_id).cloned() else {
        return Err(format!("Act '{act_id}' does not exist."));
    };
    let scenes = request_scenes(payload);
    let scene_ids: Vec<String> = scenes.iter().enumerate().map(|(i, sc)| scene_id(sc, i)).collect();
    let number_by_id: HashMap<&String, i64> = scene_ids.iter().enumerate().map(|(i, id)| (id, i as i64 + 1)).collect();
    let plan: HashMap<String, Value> = array(layer.get("story_scene_plan")).into_iter().map(|e| (s(e.get("scene_id")), e)).collect();
    let names_by_id: Vec<(String, String)> = story_cast(payload).iter().filter(|c| c.get("id").is_some_and(truthy)).map(|c| (s(c.get("id")), s(c.get("name")))).collect();
    let mut act_scene_ids: Vec<String> = array(act.get("scene_ids")).iter().map(|x| text_or_empty(Some(x))).filter(|sid| plan.contains_key(sid)).collect();
    act_scene_ids.sort_by_key(|sid| number_by_id.get(sid).copied().unwrap_or(0));
    let mut lines = Vec::new();
    for sid in &act_scene_ids {
        let entry = &plan[sid];
        let who = array(entry.get("cast_ids")).iter().filter_map(|c| {
            let id = text_or_empty(Some(c));
            names_by_id.iter().rev().find(|(i, _)| *i == id).map(|(_, n)| n.clone())
        }).collect::<Vec<_>>().join(", ");
        let spot = entry.get("spot").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| "no spot".into());
        let number = number_by_id.get(sid).map_or("?".to_string(), |n| n.to_string());
        let mut line = format!("Scene {number} ({spot}");
        line.push_str(&if who.is_empty() { ")".to_string() } else { format!("; {who})") });
        let interaction = entry.get("interaction").filter(|v| truthy(v)).map(|v| format!(" {}", text_or_empty(Some(v)))).unwrap_or_default();
        lines.push(format!("{line}: {}{interaction}", s(entry.get("story_slice"))));
    }
    if lines.is_empty() {
        return Err("Plan the scenes of this act first.".into());
    }
    let chapters = array(normalize_story_book(layer.get("story_book")).get("chapters"));
    let chapter = chapters.iter().find(|c| s(c.get("id")) == s(act.get("chapter_id")));
    let setting = setting_name(&settings_by_id(&layer), &chapter.map(|c| s(c.get("setting_id"))).unwrap_or_default());
    let act_struct = normalize_story_acts(Some(&json!([act.clone()]))).remove(0);
    let instruction = story_act_summary_instruction(&s(act.get("title")), &setting, &lines.join("\n"), &act_user_text(&act_struct), &cast_lines(payload), &s(act.get("summary")));
    let (raw, info) = run(llm, payload, instruction, "Story Mode Act Description", 700, 0.5, schema("STORY_ACT_SUMMARY_SCHEMA")).await?;
    let summary = text(Some(&json!(extract_summary(&raw)?)), 2000);
    if summary.is_empty() {
        return Err("The LLM did not return a description. Try again.".into());
    }
    let updated: Vec<Value> = acts
        .iter()
        .map(|item| {
            if s(item.get("id")) == act_id {
                let mut map = item.as_object().cloned().unwrap_or_default();
                map.insert("summary".into(), json!(summary));
                Value::Object(map)
            } else {
                item.clone()
            }
        })
        .collect();
    let mut out = Obj::new();
    out.insert("story_layer".into(), json!({"story_acts": updated}));
    out.insert("summary".into(), json!(summary));
    out.extend(run_fields(&info));
    Ok(Value::Object(out))
}
