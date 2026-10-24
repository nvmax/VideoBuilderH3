//! Story Mode data: the story book, chapter settings, acts and scene plan kept in `builder_story_layer`. Port of
//! `storyboard/story_mode.py`. Pure functions only (no LLM, no disk).
//!
//! The levels: the Story Builder shows Story > Acts > Chapters > Scenes. In the data, `story_book` chapters are the Acts and
//! `story_acts` are the Chapters inside them (the keys predate the names).

use std::collections::{HashMap, HashSet};

use fancy_regex::Regex;
use serde_json::{json, Value};

use crate::pyval::{text_or_empty, to_float, to_int, truthy, Obj};

pub const STORY_FLOW_VERSION: i64 = 2;
pub const STORY_CAST_ROLES: [&str; 4] = ["main", "love_interest", "supporting", "antagonist"];
pub const STORY_RELATIONSHIP_TYPES: [&str; 8] = ["love_interest", "partners", "exes", "family", "friends", "rivals", "strangers", "other"];
pub const STORY_VIDEO_STYLES: [&str; 3] = ["music_video", "spoken", "story_only"];
pub const STORY_CONTINUATION_MODES: [&str; 3] = ["off", "location", "llm"];
pub const CONTINUATION_SOURCES: [&str; 3] = ["auto", "llm", "manual"];
pub const STORY_STAMP_SECTIONS: [&str; 5] = ["acts", "plan", "places", "beats", "prompts"];
pub const DEFAULT_SPOT_MIN_SCENES: i64 = 3;
pub const STORY_ACT_PURPOSES: [&str; 4] = ["setup", "turn", "climax", "resolution"];
pub const STORY_PRESENCE_LEVELS: [&str; 3] = ["absent", "present", "featured"];

const MAX_CHAPTERS: usize = 40;
const MAX_PARAGRAPHS: usize = 200;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn is_match(pattern: &str, text: &str) -> bool {
    rx(pattern).is_match(text).unwrap_or(false)
}

fn squeeze(text: &str) -> String {
    static WHITESPACE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    WHITESPACE.get_or_init(|| rx(r"\s+")).replace_all(text, " ").into_owned()
}

/// Runs of white space and dashes as one underscore: `Story Only` and `story-only` become `story_only`.
fn underscored(text: &str) -> String {
    static RUNS: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RUNS.get_or_init(|| rx(r"[\s-]+")).replace_all(text, "_").into_owned()
}

/// `_text`: the value as trimmed text with `\n` line ends, cut at `limit` characters.
pub fn text(value: Option<&Value>, limit: usize) -> String {
    text_s(&text_or_empty(value), limit)
}

pub fn text_s(value: &str, limit: usize) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n").trim().chars().take(limit).collect()
}

/// `_line`: `text` on one line.
pub fn line(value: Option<&Value>, limit: usize) -> String {
    line_s(&text_or_empty(value), limit)
}

pub fn line_s(value: &str, limit: usize) -> String {
    squeeze(&text_s(value, limit * 2)).trim().chars().take(limit).collect()
}

fn lower_line(value: Option<&Value>, limit: usize) -> String {
    line(value, limit).to_lowercase()
}

/// `_id_list`: the non-empty lines of a list.
pub fn id_list(value: Option<&Value>, limit: usize) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(|i| line(Some(i), 160)).filter(|s| !s.is_empty()).take(limit).collect(),
        _ => Vec::new(),
    }
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|i| seen.insert(i.clone())).collect()
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| json!(s)).collect())
}

fn object(value: Option<&Value>) -> Obj {
    value.and_then(Value::as_object).cloned().unwrap_or_default()
}

fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// `int(float(value))`, `None` where Python raises.
pub fn int_of_float(value: Option<&Value>) -> Option<i64> {
    value.and_then(to_float).filter(|f| f.is_finite()).map(|f| f.trunc() as i64)
}

/// `str(value).strip().lower() == "true"` or `value is True`.
fn is_true(value: &Value) -> bool {
    matches!(value, Value::Bool(true)) || text_or_empty(Some(value)).trim().to_lowercase() == "true"
}

fn performer_ids(item: &Obj) -> Vec<String> {
    let mut ids = id_list(item.get("performer_ids"), 12);
    let legacy = line(item.get("performer_id"), 160);
    if !legacy.is_empty() && !ids.contains(&legacy) {
        ids.insert(0, legacy);
    }
    dedupe(ids)
}

/// `slug`: a lower-case id fragment from a name (`Home - Kitchen` becomes `home_kitchen`).
pub fn slug(value: &str) -> String {
    let lowered = line_s(value, 200).to_lowercase();
    let slug: String = rx(r"[^a-z0-9]+").replace_all(&lowered, "_").trim_matches('_').chars().take(60).collect();
    if slug.is_empty() { "item".into() } else { slug }
}

pub fn setting_id(name: &str) -> String {
    format!("setting_{}", slug(name))
}

// -- normalizers -----------------------------------------------------------------------------------------------------------

/// `{cast id: absent | present | featured}`: how much a character is in an act or chapter.
pub fn normalize_story_presence(value: Option<&Value>) -> Obj {
    let mut presence = Obj::new();
    if let Some(Value::Object(map)) = value {
        for (cast_id, level) in map.iter().take(40) {
            let (key, name) = (line_s(cast_id, 160), lower_line(Some(level), 20));
            if !key.is_empty() && STORY_PRESENCE_LEVELS.contains(&name.as_str()) {
                presence.insert(key, json!(name));
            }
        }
    }
    presence
}

/// A whole count between `minimum` and `maximum`. Anything else is `minimum` (0 means "decide for me").
pub fn normalize_story_count(value: Option<&Value>, minimum: i64, maximum: i64) -> i64 {
    match int_of_float(value) {
        Some(n) => n.clamp(minimum, maximum),
        None => minimum,
    }
}

/// The setting a chapter (a `story_acts` entry) happens in: its own, or its act's (a `story_book` chapter).
pub fn chapter_setting_id(act: &Value, chapter: &Value) -> String {
    let own = line(act.get("setting_id"), 120);
    if own.is_empty() { line(chapter.get("setting_id"), 120) } else { own }
}

pub fn normalize_story_book(value: Option<&Value>) -> Value {
    let source = object(value);
    let mut chapters = Vec::new();
    for (index, chapter) in array(source.get("chapters")).iter().enumerate() {
        let Some(chapter) = chapter.as_object() else { continue };
        if chapters.len() >= MAX_CHAPTERS {
            continue;
        }
        let paragraphs: Vec<Value> = array(chapter.get("paragraphs")).iter().map(|p| text(Some(p), 6000)).filter(|p| !p.is_empty()).take(MAX_PARAGRAPHS).map(Value::String).collect();
        let purpose = lower_line(chapter.get("purpose"), 20);
        let id = line(chapter.get("id"), 80);
        chapters.push(json!({
            "id": if id.is_empty() { format!("ch_{}", index + 1) } else { id },
            "title": line(chapter.get("title"), 200),
            "setting_id": line(chapter.get("setting_id"), 120),
            "paragraphs": paragraphs,
            "purpose": if STORY_ACT_PURPOSES.contains(&purpose.as_str()) { purpose } else { String::new() },
            "presence": normalize_story_presence(chapter.get("presence")),
        }));
    }
    json!({"title": line(source.get("title"), 200), "premise": text(source.get("premise"), 2000), "chapters": chapters})
}

pub fn normalize_story_settings(value: Option<&Value>) -> Vec<Value> {
    let mut settings = Vec::new();
    let mut seen = HashSet::new();
    for item in array(value) {
        let Some(item) = item.as_object() else { continue };
        let name = line(item.get("name"), 160);
        let mut item_id = line(item.get("id"), 120);
        if item_id.is_empty() && !name.is_empty() {
            item_id = setting_id(&name);
        }
        if item_id.is_empty() || !seen.insert(item_id.clone()) {
            continue;
        }
        settings.push(json!({"id": item_id, "name": if name.is_empty() { item_id.clone() } else { name }, "look": text(item.get("look"), 2000)}));
    }
    settings
}

/// A chapter of the story (a `story_acts` entry).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Act {
    pub id: String,
    pub chapter_id: String,
    pub title: String,
    pub summary: String,
    pub cast_ids: Vec<String>,
    pub scene_ids: Vec<String>,
    pub setting_id: String,
    pub presence: Obj,
    pub location_note: Option<String>,
    pub direction: Option<String>,
}

impl Act {
    pub fn to_value(&self) -> Value {
        let mut map = Obj::new();
        map.insert("id".into(), json!(self.id));
        map.insert("chapter_id".into(), json!(self.chapter_id));
        map.insert("title".into(), json!(self.title));
        map.insert("summary".into(), json!(self.summary));
        map.insert("cast_ids".into(), strings(&self.cast_ids));
        map.insert("scene_ids".into(), strings(&self.scene_ids));
        map.insert("setting_id".into(), json!(self.setting_id));
        map.insert("presence".into(), Value::Object(self.presence.clone()));
        if let Some(note) = &self.location_note {
            map.insert("location_note".into(), json!(note));
        }
        if let Some(direction) = &self.direction {
            map.insert("direction".into(), json!(direction));
        }
        Value::Object(map)
    }
}

pub fn normalize_story_acts(value: Option<&Value>) -> Vec<Act> {
    let mut acts = Vec::new();
    for (index, item) in array(value).iter().enumerate() {
        let Some(item) = item.as_object() else { continue };
        let id = line(item.get("id"), 80);
        let note = text(item.get("location_note"), 2000);
        let direction = text(item.get("direction"), 2000);
        acts.push(Act {
            id: if id.is_empty() { format!("act_{}", index + 1) } else { id },
            chapter_id: line(item.get("chapter_id"), 80),
            title: line(item.get("title"), 200),
            summary: text(item.get("summary"), 2000),
            cast_ids: id_list(item.get("cast_ids"), 40),
            scene_ids: id_list(item.get("scene_ids"), 400),
            setting_id: line(item.get("setting_id"), 120),
            presence: normalize_story_presence(item.get("presence")),
            location_note: (!note.is_empty()).then_some(note),
            direction: (!direction.is_empty()).then_some(direction),
        });
    }
    acts
}

pub fn acts_to_value(acts: &[Act]) -> Value {
    Value::Array(acts.iter().map(Act::to_value).collect())
}

/// The user's location and direction for an act, as lines the plan and beat prompts must follow.
pub fn act_user_text(act: &Act) -> String {
    let mut lines = Vec::new();
    if let Some(note) = &act.location_note {
        lines.push(format!("Where this happens (user, follow exactly): {note}"));
    }
    if let Some(direction) = &act.direction {
        lines.push(format!("What must happen (user, follow exactly): {direction}"));
    }
    lines.join("\n")
}

pub fn normalize_plan_performance(value: Option<&Value>) -> String {
    let text = underscored(&lower_line(value, 200));
    match text.as_str() {
        "speaking" | "short_film" | "dialogue" | "dialog" | "spoken" => "speaking",
        "no_lip_sync" | "nolipsync" | "no_lipsync" | "story" | "story_only" | "visual_only" | "silent" => "no_lip_sync",
        "instrumental" | "music_only" | "no_vocals" => "instrumental",
        _ => "singing",
    }
    .to_string()
}

fn continuation_source(value: Option<&Value>) -> String {
    let text = lower_line(value, 20);
    if CONTINUATION_SOURCES.contains(&text.as_str()) { text } else { String::new() }
}

/// A scene's place in the story plan.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanEntry {
    pub scene_id: String,
    pub act_id: String,
    pub story_slice: String,
    pub spot: String,
    pub mood: String,
    pub performance_mode: String,
    pub note_sources: Vec<String>,
    pub continues_previous: bool,
    pub continuation_source: String,
    pub llm_continues: bool,
    pub continuation_direction: String,
    pub cast_ids: Vec<String>,
    pub performer_ids: Vec<String>,
    pub performer_id: String,
    pub performer_source: String,
    pub interaction: String,
    pub cast_source: String,
    pub user_note: String,
}

impl PlanEntry {
    pub fn to_value(&self) -> Value {
        json!({
            "scene_id": self.scene_id,
            "act_id": self.act_id,
            "story_slice": self.story_slice,
            "spot": self.spot,
            "mood": self.mood,
            "performance_mode": self.performance_mode,
            "note_sources": self.note_sources,
            "continues_previous": self.continues_previous,
            "continuation_source": self.continuation_source,
            "llm_continues": self.llm_continues,
            "continuation_direction": self.continuation_direction,
            "cast_ids": self.cast_ids,
            "performer_ids": self.performer_ids,
            "performer_id": self.performer_id,
            "performer_source": self.performer_source,
            "interaction": self.interaction,
            "cast_source": self.cast_source,
            "user_note": self.user_note,
        })
    }
}

pub fn normalize_story_scene_plan(value: Option<&Value>) -> Vec<PlanEntry> {
    let mut plan = Vec::new();
    let mut seen = HashSet::new();
    for item in array(value) {
        let Some(item) = item.as_object() else { continue };
        let scene_id = line(item.get("scene_id"), 160);
        if scene_id.is_empty() || !seen.insert(scene_id.clone()) {
            continue;
        }
        let performers = performer_ids(item);
        let cast_source = lower_line(item.get("cast_source"), 20);
        plan.push(PlanEntry {
            scene_id,
            act_id: line(item.get("act_id"), 80),
            story_slice: text(item.get("story_slice"), 1200),
            spot: line(item.get("spot"), 160),
            mood: line(item.get("mood"), 200),
            performance_mode: normalize_plan_performance(item.get("performance_mode")),
            note_sources: id_list(item.get("note_sources"), 12),
            continues_previous: matches!(item.get("continues_previous"), Some(Value::Bool(true))),
            continuation_source: continuation_source(item.get("continuation_source")),
            llm_continues: matches!(item.get("llm_continues"), Some(Value::Bool(true))),
            continuation_direction: text(item.get("continuation_direction"), 600),
            cast_ids: id_list(item.get("cast_ids"), 12),
            performer_id: performers.first().cloned().unwrap_or_default(),
            performer_ids: performers,
            performer_source: if item.get("performer_source").and_then(Value::as_str) == Some("manual") { "manual".into() } else { String::new() },
            interaction: text(item.get("interaction"), 600),
            cast_source: if matches!(cast_source.as_str(), "story" | "manual") { cast_source } else { String::new() },
            user_note: text(item.get("user_note"), 1200),
        });
    }
    plan
}

pub fn plan_to_value(plan: &[PlanEntry]) -> Value {
    Value::Array(plan.iter().map(PlanEntry::to_value).collect())
}

/// The Story Mode keys of a story layer, normalized. Keys the source does not have are left out.
pub fn normalize_story_mode_fields(source: &Obj) -> Obj {
    let mut result = Obj::new();
    let has = |key: &str| source.contains_key(key);
    if has("story_flow_version") {
        let version = int_of_float(Some(&json!(truthy_or_zero(source.get("story_flow_version"))))).unwrap_or(0);
        result.insert("story_flow_version".into(), json!(if version >= STORY_FLOW_VERSION { STORY_FLOW_VERSION } else { 0 }));
    }
    if has("story_book") {
        result.insert("story_book".into(), normalize_story_book(source.get("story_book")));
    }
    if has("story_book_text") {
        result.insert("story_book_text".into(), json!(text(source.get("story_book_text"), 120_000)));
    }
    if has("story_settings") {
        result.insert("story_settings".into(), Value::Array(normalize_story_settings(source.get("story_settings"))));
    }
    if has("story_acts") {
        result.insert("story_acts".into(), acts_to_value(&normalize_story_acts(source.get("story_acts"))));
    }
    if has("story_scene_plan") {
        result.insert("story_scene_plan".into(), plan_to_value(&normalize_story_scene_plan(source.get("story_scene_plan"))));
    }
    if has("story_spot_min_scenes") {
        result.insert("story_spot_min_scenes".into(), json!(normalize_spot_min_scenes(source.get("story_spot_min_scenes"))));
    }
    if has("story_cast_ids") {
        result.insert("story_cast_ids".into(), strings(&id_list(source.get("story_cast_ids"), 40)));
    }
    if has("story_allow_new_cast") {
        result.insert("story_allow_new_cast".into(), json!(source.get("story_allow_new_cast").is_some_and(is_true)));
    }
    if has("story_video_style") {
        result.insert("story_video_style".into(), json!(normalize_story_video_style(source.get("story_video_style"))));
    }
    if has("story_continuation_mode") {
        result.insert("story_continuation_mode".into(), json!(normalize_story_continuation_mode(source.get("story_continuation_mode"))));
    }
    if has("story_stamps") {
        result.insert("story_stamps".into(), normalize_story_stamps(source.get("story_stamps")));
    }
    if has("story_main_cast_id") {
        result.insert("story_main_cast_id".into(), json!(line(source.get("story_main_cast_id"), 160)));
    }
    if has("story_cast_roles") {
        result.insert("story_cast_roles".into(), Value::Object(normalize_story_cast_roles(source.get("story_cast_roles"))));
    }
    if has("story_relationships") {
        result.insert("story_relationships".into(), Value::Array(normalize_story_relationships(source.get("story_relationships"))));
    }
    if has("story_user_locations") {
        result.insert("story_user_locations".into(), json!(text(source.get("story_user_locations"), 3000)));
    }
    if has("story_tone_notes") {
        result.insert("story_tone_notes".into(), json!(text(source.get("story_tone_notes"), 2000)));
    }
    if has("story_character_notes") {
        result.insert("story_character_notes".into(), Value::Object(normalize_story_character_notes(source.get("story_character_notes"))));
    }
    if has("story_act_count") {
        result.insert("story_act_count".into(), json!(normalize_story_count(source.get("story_act_count"), 0, 12)));
    }
    if has("story_chapter_counts") {
        // {act (story_book chapter) id: how many chapters the user wants in it}.
        let mut counts = Obj::new();
        if let Some(Value::Object(map)) = source.get("story_chapter_counts") {
            for (key, value) in map.iter().take(40) {
                let (key, count) = (line_s(key, 80), normalize_story_count(Some(value), 0, 20));
                if !key.is_empty() && count > 0 {
                    counts.insert(key, json!(count));
                }
            }
        }
        result.insert("story_chapter_counts".into(), Value::Object(counts));
    }
    result
}

/// `int(float(value or 0))`'s argument: 0 where the value is falsy.
fn truthy_or_zero(value: Option<&Value>) -> Value {
    match value {
        Some(v) if truthy(v) => v.clone(),
        _ => json!(0),
    }
}

/// `{cast id: role}` for the story's characters. Unknown roles are dropped.
pub fn normalize_story_cast_roles(value: Option<&Value>) -> Obj {
    let mut roles = Obj::new();
    if let Some(Value::Object(map)) = value {
        for (cast_id, role) in map.iter().take(40) {
            let (key, name) = (line_s(cast_id, 160), lower_line(Some(role), 40));
            if !key.is_empty() && STORY_CAST_ROLES.contains(&name.as_str()) {
                roles.insert(key, json!(name));
            }
        }
    }
    roles
}

/// `{cast id: text}`: each character's personality and emotions. Empty notes are dropped.
pub fn normalize_story_character_notes(value: Option<&Value>) -> Obj {
    let mut notes = Obj::new();
    if let Some(Value::Object(map)) = value {
        for (cast_id, note) in map.iter().take(40) {
            let (key, text) = (line_s(cast_id, 160), line(Some(note), 600));
            if !key.is_empty() && !text.is_empty() {
                notes.insert(key, json!(text));
            }
        }
    }
    notes
}

/// `[{a, b, type, note}]`: how two characters are connected. A pair with one person or an unknown type is dropped.
pub fn normalize_story_relationships(value: Option<&Value>) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for item in array(value).iter().take(40) {
        let item = object(Some(item));
        let (a, b) = (line(item.get("a"), 160), line(item.get("b"), 160));
        let kind = lower_line(item.get("type"), 40);
        let mut pair = [a.clone(), b.clone()];
        pair.sort();
        let pair = pair.join("|");
        if a.is_empty() || b.is_empty() || a == b || !STORY_RELATIONSHIP_TYPES.contains(&kind.as_str()) || seen.contains(&pair) {
            continue;
        }
        seen.insert(pair);
        result.push(json!({"a": a, "b": b, "type": kind, "note": line(item.get("note"), 300)}));
    }
    result
}

fn stamp_parts(value: Option<&Value>) -> Obj {
    let mut parts = Obj::new();
    if let Some(Value::Object(map)) = value {
        for (key, print) in map.iter().take(12) {
            let (name, text) = (line_s(key, 40), line(Some(print), 64));
            if !name.is_empty() && !text.is_empty() {
                parts.insert(name, json!(text));
            }
        }
    }
    parts
}

/// `{book: {part: print}, acts|plan|places|beats|prompts: {id: {part: print}}}`: what each step was made from.
pub fn normalize_story_stamps(value: Option<&Value>) -> Value {
    let source = object(value);
    let mut result = Obj::new();
    result.insert("book".into(), Value::Object(stamp_parts(source.get("book"))));
    for section in STORY_STAMP_SECTIONS {
        let mut items = Obj::new();
        if let Some(Value::Object(group)) = source.get(section) {
            for (item_id, item) in group.iter().take(2000) {
                let (key, parts) = (line_s(item_id, 160), stamp_parts(Some(item)));
                if !key.is_empty() && !parts.is_empty() {
                    items.insert(key, Value::Object(parts));
                }
            }
        }
        result.insert(section.into(), Value::Object(items));
    }
    Value::Object(result)
}

/// `off`, `location` or `llm`. Anything else is `off`.
pub fn normalize_story_continuation_mode(value: Option<&Value>) -> String {
    let text = underscored(&lower_line(value, 200));
    if STORY_CONTINUATION_MODES.contains(&text.as_str()) { text } else { "off".into() }
}

/// `music_video`, `spoken` or `story_only`. Anything else is `music_video`.
pub fn normalize_story_video_style(value: Option<&Value>) -> String {
    let text = underscored(&lower_line(value, 200));
    if STORY_VIDEO_STYLES.contains(&text.as_str()) { text } else { "music_video".into() }
}

pub fn normalize_spot_min_scenes(value: Option<&Value>) -> i64 {
    match int_of_float(value) {
        Some(n) => n.clamp(1, 20),
        None => DEFAULT_SPOT_MIN_SCENES,
    }
}

/// Timeline segment fields for a scene plan performance. Story-only and instrumental scenes are no lip sync.
pub fn plan_segment_fields(performance_mode: Option<&Value>) -> Obj {
    let mode = normalize_plan_performance(performance_mode);
    let mut fields = Obj::new();
    if mode == "no_lip_sync" || mode == "instrumental" {
        fields.insert("performance_mode".into(), json!("no_lip_sync"));
        fields.insert("lyric_no_lip_sync".into(), json!(true));
    } else {
        fields.insert("performance_mode".into(), json!(mode));
        fields.insert("lyric_no_lip_sync".into(), json!(false));
    }
    fields
}

pub fn is_story_mode_layer(layer: &Value) -> bool {
    int_of_float(Some(&truthy_or_zero(layer.get("story_flow_version")))).unwrap_or(0) >= STORY_FLOW_VERSION
}

fn seconds(value: Option<&Value>) -> f64 {
    value.and_then(to_float).unwrap_or(0.0)
}

/// The scene plan with `continues_previous` set for every scene. `scenes` is the timeline in order (`id`, `start`, `end`). A manual
/// choice is kept. Otherwise `location` continues a scene that touches the scene before at the same spot in the same chapter, `llm`
/// uses the LLM's answer and `off` continues nothing. The first scene, and a scene with a gap before it, never continue.
pub fn resolve_story_continuations(plan: Option<&Value>, scenes: &[Value], acts: Option<&Value>, mode: Option<&Value>) -> Vec<PlanEntry> {
    let mode = normalize_story_continuation_mode(mode);
    let normalized = normalize_story_scene_plan(plan);
    let mut entries: HashMap<String, PlanEntry> = normalized.iter().map(|e| (e.scene_id.clone(), e.clone())).collect();
    let mut chapter_of: HashMap<String, String> = HashMap::new();
    for act in normalize_story_acts(acts) {
        for scene_id in &act.scene_ids {
            chapter_of.insert(scene_id.clone(), act.chapter_id.clone());
        }
    }
    let mut previous: Option<&Value> = None;
    for scene in scenes {
        let sid = text_or_empty(scene.get("id"));
        if entries.contains_key(&sid) {
            let before_id = previous.map(|p| text_or_empty(p.get("id")));
            let before = before_id.as_ref().and_then(|id| entries.get(id)).cloned();
            let touches = previous.is_some_and(|p| (seconds(scene.get("start")) - seconds(p.get("end"))).abs() <= 0.05);
            let same_spot = before.as_ref().is_some_and(|b| {
                let entry = &entries[&sid];
                let (mine, theirs) = (chapter_of.get(&sid), chapter_of.get(&b.scene_id));
                !entry.spot.is_empty() && entry.spot.to_lowercase() == b.spot.to_lowercase() && mine.is_some_and(|m| !m.is_empty()) && mine == theirs
            });
            let entry = entries.get_mut(&sid).expect("checked above");
            if !touches {
                entry.continues_previous = false;
                entry.continuation_source = String::new();
            } else if entry.continuation_source != "manual" {
                if mode == "location" && same_spot {
                    entry.continues_previous = true;
                    entry.continuation_source = "auto".into();
                } else if mode == "llm" && entry.llm_continues {
                    entry.continues_previous = true;
                    entry.continuation_source = "llm".into();
                } else {
                    entry.continues_previous = false;
                    entry.continuation_source = String::new();
                }
            }
        }
        previous = Some(scene);
    }
    normalized.iter().map(|e| entries[&e.scene_id].clone()).collect()
}

// -- the book text ---------------------------------------------------------------------------------------------------------

/// True when the line is a book section heading (`Act 1: title`, `Chapter 2 - title`).
pub fn is_chapter_heading(line: &str) -> bool {
    is_match(r"(?i)^\s*(?:#+\s*)?(?:chapter|act)\s+([0-9]+|[a-z]+(?:[\s-][a-z]+)?)\s*(?:[:.\-\x{2013}\x{2014}]\s*(.*?))?\s*$", line)
}

/// Paragraphs of a story. Blank lines separate paragraphs. Text without blank lines is one paragraph per line.
pub fn split_paragraphs(value: &str) -> Vec<String> {
    let body = text_s(value, 200_000);
    if body.is_empty() {
        return Vec::new();
    }
    let blocks: Vec<String> = if is_match(r"\n\s*\n", &body) { rx(r"\n\s*\n").split(&body).flatten().map(str::to_string).collect() } else { body.split('\n').map(str::to_string).collect() };
    blocks.into_iter().map(|b| b.trim().to_string()).filter(|b| !b.is_empty()).collect()
}

/// A parsed book and the names of the settings written in it, by setting id.
pub struct ParsedBook {
    pub book: Value,
    pub setting_names: Obj,
}

pub fn parse_story_book_text(source: &str) -> ParsedBook {
    let heading_pattern = rx(r"(?i)^\s*(?:#+\s*)?(?:chapter|act)\s+([0-9]+|[a-z]+(?:[\s-][a-z]+)?)\s*(?:[:.\-\x{2013}\x{2014}]\s*(.*?))?\s*$");
    let meta_pattern = rx(r"(?i)^\s*(title|premise|setting)\s*:\s*(.*?)\s*$");
    let lines: Vec<String> = text_s(source, 200_000).split('\n').map(str::to_string).collect();
    let (mut title, mut premise) = (String::new(), String::new());
    let mut chapters: Vec<Value> = Vec::new();
    let mut setting_names = Obj::new();
    let mut current: Option<Obj> = None;
    let mut buffer: Vec<String> = Vec::new();
    let mut preamble: Vec<String> = Vec::new();

    fn flush(current: &mut Option<Obj>, buffer: &[String], chapters: &mut Vec<Value>) {
        if let Some(mut chapter) = current.take() {
            chapter.insert("paragraphs".into(), strings(&split_paragraphs(&buffer.join("\n"))));
            chapters.push(Value::Object(chapter));
        }
    }

    for raw in &lines {
        if let Some(caps) = heading_pattern.captures(raw).ok().flatten() {
            flush(&mut current, &buffer, &mut chapters);
            buffer = Vec::new();
            let number = caps.get(1).map_or("", |m| m.as_str());
            let heading_title = line_s(caps.get(2).map_or("", |m| m.as_str()), 200);
            let mut chapter = Obj::new();
            chapter.insert("id".into(), json!(format!("ch_{}", chapters.len() + 1)));
            chapter.insert("title".into(), json!(if heading_title.is_empty() { format!("Act {number}") } else { heading_title }));
            chapter.insert("setting_id".into(), json!(""));
            chapter.insert("paragraphs".into(), json!([]));
            current = Some(chapter);
            continue;
        }
        if let Some(caps) = meta_pattern.captures(raw).ok().flatten() {
            let key = caps[1].to_lowercase();
            let value = caps.get(2).map_or("", |m| m.as_str());
            if key == "setting" {
                if let Some(chapter) = current.as_mut() {
                    let unset = chapter.get("setting_id").and_then(Value::as_str).is_some_and(str::is_empty);
                    if unset && buffer.join("").trim().is_empty() {
                        if !line_s(value, 200).is_empty() {
                            let id = setting_id(value);
                            chapter.insert("setting_id".into(), json!(id));
                            if !setting_names.contains_key(&id) {
                                setting_names.insert(id, json!(line_s(value, 160)));
                            }
                        }
                        continue;
                    }
                }
            }
            if current.is_none() && (key == "title" || key == "premise") {
                if key == "title" && title.is_empty() {
                    title = line_s(value, 200);
                    continue;
                }
                if key == "premise" && premise.is_empty() {
                    premise = text_s(value, 2000);
                    continue;
                }
            }
        }
        if current.is_none() {
            preamble.push(raw.clone());
        } else {
            buffer.push(raw.clone());
        }
    }
    flush(&mut current, &buffer, &mut chapters);
    if chapters.is_empty() {
        let paragraphs = split_paragraphs(&preamble.join("\n"));
        if !paragraphs.is_empty() {
            chapters.push(json!({"id": "ch_1", "title": "Act 1", "setting_id": "", "paragraphs": paragraphs}));
        }
    }
    let book = normalize_story_book(Some(&json!({"title": title, "premise": premise, "chapters": chapters})));
    ParsedBook { book, setting_names }
}

/// The editable plain-text form of a book. `parse_story_book_text` reads it back unchanged.
pub fn story_book_to_text(book: &Value, settings: Option<&Value>) -> String {
    let book = normalize_story_book(Some(book));
    let names: HashMap<String, String> = normalize_story_settings(settings).iter().map(|s| (text_or_empty(s.get("id")), text_or_empty(s.get("name")))).collect();
    let mut head = Vec::new();
    let title = text_or_empty(book.get("title"));
    let premise = text_or_empty(book.get("premise"));
    if !title.is_empty() {
        head.push(format!("Title: {title}"));
    }
    if !premise.is_empty() {
        head.push(format!("Premise: {premise}"));
    }
    let mut blocks = Vec::new();
    if !head.is_empty() {
        blocks.push(head.join("\n"));
    }
    for (index, chapter) in array(book.get("chapters")).iter().enumerate() {
        let number = index + 1;
        let chapter_title = text_or_empty(chapter.get("title"));
        let mut heading = format!("Act {number}: {}", if chapter_title.is_empty() { format!("Act {number}") } else { chapter_title });
        if let Some(name) = names.get(&text_or_empty(chapter.get("setting_id"))).filter(|n| !n.is_empty()) {
            heading.push_str(&format!("\nSetting: {name}"));
        }
        let mut parts = vec![heading];
        parts.extend(array(chapter.get("paragraphs")).iter().map(|p| text_or_empty(Some(p))));
        blocks.push(parts.join("\n\n"));
    }
    blocks.join("\n\n")
}

/// Settings for every chapter: keeps existing looks, adds settings named in the text, drops unused ones.
pub fn sync_story_settings(book: &Value, setting_names: &Obj, existing: Option<&Value>) -> Vec<Value> {
    let current: HashMap<String, Value> = normalize_story_settings(existing).into_iter().map(|s| (text_or_empty(s.get("id")), s)).collect();
    let mut used: Vec<String> = Vec::new();
    for chapter in array(normalize_story_book(Some(book)).get("chapters")) {
        let sid = text_or_empty(chapter.get("setting_id"));
        if !sid.is_empty() && !used.contains(&sid) {
            used.push(sid);
        }
    }
    let mut settings = Vec::new();
    for sid in used {
        let mut item = current.get(&sid).and_then(Value::as_object).cloned().unwrap_or_else(|| {
            let mut fresh = Obj::new();
            fresh.insert("id".into(), json!(sid));
            fresh.insert("name".into(), json!(""));
            fresh.insert("look".into(), json!(""));
            fresh
        });
        let written = text_or_empty(setting_names.get(&sid));
        let name = if !written.is_empty() { written } else { Some(text_or_empty(item.get("name"))).filter(|n| !n.is_empty()).unwrap_or_else(|| sid.clone()) };
        item.insert("name".into(), json!(name));
        settings.push(Value::Object(item));
    }
    settings
}

/// Story layer fields after the user edits the book text: book, text and settings updated together.
pub fn apply_story_book_text(layer: &Value, source: &str) -> Obj {
    let parsed = parse_story_book_text(source);
    let settings = sync_story_settings(&parsed.book, &parsed.setting_names, layer.get("story_settings"));
    let settings_value = Value::Array(settings);
    let mut fields = Obj::new();
    fields.insert("story_flow_version".into(), json!(STORY_FLOW_VERSION));
    fields.insert("story_book_text".into(), json!(story_book_to_text(&parsed.book, Some(&settings_value))));
    fields.insert("story_book".into(), parsed.book);
    fields.insert("story_settings".into(), settings_value);
    // The pack builds the dict as book, text, settings. Keep that order.
    let mut ordered = Obj::new();
    for key in ["story_flow_version", "story_book", "story_book_text", "story_settings"] {
        if let Some(v) = fields.get(key) {
            ordered.insert(key.into(), v.clone());
        }
    }
    ordered
}

// -- pasted stories --------------------------------------------------------------------------------------------------------

/// Build chapters from a paragraph split, keeping every paragraph exactly as written. `split` is a list of
/// `{start_paragraph, title, setting}` with 1-based paragraph numbers. Starts are sorted, the first chapter always starts at
/// paragraph 1, and out-of-range or repeated starts are dropped.
pub fn chapters_from_split(paragraphs: &[String], split: Option<&Value>) -> (Vec<Value>, Obj) {
    let count = paragraphs.len() as i64;
    if count == 0 {
        return (Vec::new(), Obj::new());
    }
    let mut entries: Vec<(i64, String, String)> = Vec::new();
    for item in array(split) {
        let Some(item) = item.as_object() else { continue };
        let Some(start) = item.get("start_paragraph").and_then(to_int) else { continue };
        if (1..=count).contains(&start) {
            entries.push((start, line(item.get("title"), 200), line(item.get("setting"), 160)));
        }
    }
    entries.sort_by_key(|e| e.0);
    let mut starts = HashSet::new();
    let mut kept: Vec<(i64, String, String)> = Vec::new();
    for entry in entries {
        if starts.insert(entry.0) {
            kept.push(entry);
        }
    }
    if kept.first().is_none_or(|k| k.0 != 1) {
        kept.insert(0, (1, String::new(), String::new()));
    }
    let mut chapters = Vec::new();
    let mut names = Obj::new();
    for (index, (start, title, setting)) in kept.iter().enumerate() {
        let end = if index + 1 < kept.len() { kept[index + 1].0 - 1 } else { count };
        let sid = if setting.is_empty() { String::new() } else { setting_id(setting) };
        if !sid.is_empty() && !names.contains_key(&sid) {
            names.insert(sid.clone(), json!(setting));
        }
        chapters.push(json!({
            "id": format!("ch_{}", index + 1),
            "title": if title.is_empty() { format!("Act {}", index + 1) } else { title.clone() },
            "setting_id": sid,
            "paragraphs": paragraphs[(*start as usize - 1)..(end as usize)].to_vec(),
        }));
    }
    let normalized = normalize_story_book(Some(&json!({"chapters": chapters})));
    (array(normalized.get("chapters")), names)
}

/// True when the chapters hold exactly the pasted paragraphs, in order.
pub fn chapters_keep_text(paragraphs: &[String], chapters: &[Value]) -> bool {
    let joined: Vec<String> = chapters.iter().flat_map(|c| array(c.get("paragraphs"))).map(|p| text_or_empty(Some(&p))).collect();
    joined == paragraphs
}

/// Story Mode text for one scene's beat, or `None` when the scene has no plan entry. Returns `(arc_text, brief_text)`. The arc text
/// holds the act and the previous, current and next scene slices as `Scene N (Spot) - slice` lines. The brief text holds the book
/// premise and the setting's look.
pub fn story_beat_context(layer: &Value, scene_id: &str, scene_number: Option<&Value>) -> Option<(String, String)> {
    let plan = normalize_story_scene_plan(layer.get("story_scene_plan"));
    let index = plan.iter().position(|e| e.scene_id == scene_id)?;
    let entry = &plan[index];
    let acts: HashMap<String, Act> = normalize_story_acts(layer.get("story_acts")).into_iter().map(|a| (a.id.clone(), a)).collect();
    let act = acts.get(&entry.act_id);
    let book = normalize_story_book(layer.get("story_book"));
    let chapter = array(book.get("chapters")).into_iter().find(|c| act.is_some_and(|a| text_or_empty(c.get("id")) == a.chapter_id)).unwrap_or_else(|| json!({}));
    let settings: HashMap<String, Value> = normalize_story_settings(layer.get("story_settings")).into_iter().map(|s| (text_or_empty(s.get("id")), s)).collect();
    let setting = settings.get(&text_or_empty(chapter.get("setting_id"))).cloned().unwrap_or_else(|| json!({}));
    let number = scene_number.and_then(to_int).unwrap_or(index as i64 + 1);
    let mut lines: Vec<String> = Vec::new();
    if let Some(act) = act {
        lines.push(format!("Act: {} - {}", act.title, act.summary).trim_matches([' ', '-']).to_string());
    }
    for offset in [-1i64, 0, 1] {
        let position = index as i64 + offset;
        if position >= 0 && (position as usize) < plan.len() {
            let other = &plan[position as usize];
            let spot = if other.spot.is_empty() { text_or_empty(setting.get("name")) } else { other.spot.clone() };
            lines.push(format!("Scene {} ({}) - {}", number + offset, spot, other.story_slice));
        }
    }
    let mut brief: Vec<String> = Vec::new();
    let premise = text_or_empty(book.get("premise"));
    if !premise.is_empty() {
        brief.push(format!("Story premise: {premise}"));
    }
    if chapter.as_object().is_some_and(|c| !c.is_empty()) {
        let setting_name = text_or_empty(setting.get("name"));
        brief.push(format!("Chapter: {}", text_or_empty(chapter.get("title"))) + &if setting_name.is_empty() { String::new() } else { format!(", set in {setting_name}") });
    }
    let look = text_or_empty(setting.get("look"));
    if !look.is_empty() {
        brief.push(format!("Setting look: {look}"));
    }
    if !entry.mood.is_empty() {
        brief.push(format!("Scene mood: {}", entry.mood));
    }
    if entry.continues_previous {
        brief.push(
            format!(
                "Continuation: this scene is the very next moment of Scene {} in one unbroken camera take. It opens exactly where that scene ends (same place, people, framing and camera move), with no cut or new setup.",
                number - 1
            ) + &if entry.continuation_direction.is_empty() { String::new() } else { format!(" Then: {}", entry.continuation_direction) },
        );
    }
    if !entry.interaction.is_empty() {
        brief.push(format!("What the characters do with each other: {}", entry.interaction));
    }
    if !entry.user_note.is_empty() {
        brief.push(format!("User direction for this scene (follow it exactly): {}", entry.user_note));
    }
    if let Some(act) = act {
        let user_text = act_user_text(act);
        if !user_text.is_empty() {
            brief.push(user_text);
        }
    }
    let song_brief = text_or_empty(layer.get("song_story_brief"));
    if !song_brief.is_empty() {
        brief.push(song_brief);
    }
    Some((lines.join("\n"), brief.join("\n")))
}

const PERSON_REFERENCE_TYPES: [&str; 7] = ["", "character", "person", "human", "subject", "singer", "creature"];

/// True for a subject card that is a person. Clothing, props, vehicles and other things are not cast members.
pub fn is_story_person_card(card: &Value) -> bool {
    let Some(map) = card.as_object() else { return false };
    if map.get("extra_reference_for").is_some_and(truthy) {
        return false;
    }
    !is_scene_item_card(card)
}

/// True for a clothing, prop, vehicle or other thing card: its type is not a person type.
pub fn is_scene_item_card(card: &Value) -> bool {
    card.as_object().is_some_and(|c| !PERSON_REFERENCE_TYPES.contains(&text_or_empty(c.get("reference_type")).trim().to_lowercase().as_str()))
}

/// One line for a scene's prompt or beat that says who wears or uses each clothing or prop card mapped to it. `items` are the scene's
/// clothing and prop cards. `people` are its people (`id`, `name`). An item's `wears` names who wears or uses it in this scene. When
/// that person is not in the scene, the first person does. Empty when the scene has no items.
pub fn scene_items_text(items: &[Value], people: &[Value]) -> String {
    // A dict by id: a repeated id keeps its first place and takes the later name.
    let mut names: Vec<(String, String)> = Vec::new();
    for person in people.iter().filter_map(Value::as_object) {
        let (id, name) = (text_or_empty(person.get("id")), text_or_empty(person.get("name")).trim().to_string());
        match names.iter_mut().find(|(i, _)| *i == id) {
            Some(entry) => entry.1 = name,
            None => names.push((id, name)),
        }
    }
    let first = names.iter().map(|(_, n)| n.clone()).find(|n| !n.is_empty()).unwrap_or_default();
    let mut lines = Vec::new();
    for item in items {
        let name = text_or_empty(item.get("name")).trim().to_string();
        let kind = text_or_empty(item.get("reference_type")).trim().to_lowercase();
        // A style card is not a prop, and the Story Builder does not use style cards yet.
        if name.is_empty() || kind == "style" {
            continue;
        }
        let description: String = text_or_empty(item.get("description")).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect();
        let detail = if description.is_empty() { String::new() } else { format!(" ({description})") };
        let wears = text_or_empty(item.get("wears"));
        let user = names.iter().find(|(id, _)| *id == wears).map(|(_, n)| n.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| first.clone());
        if user.is_empty() && kind != "prop" {
            lines.push(format!("- {name}{detail} is in the scene."));
        } else if kind == "outfit" {
            lines.push(format!("- {user} wears {name}{detail}. Show {user} wearing it whenever {user} is seen."));
        } else if kind == "vehicle" {
            lines.push(format!("- {name}{detail} is {user}'s vehicle: {user} drives it or stands beside it."));
        } else if kind == "prop" {
            lines.push(format!("- {name}{detail} is part of the set: it stands where it naturally belongs and the action is staged around it."));
        } else {
            lines.push(format!("- {user} holds and uses {name}{detail} the way it is normally used."));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut all = vec!["WARDROBE AND PROPS IN THIS SCENE (things, not people; the beat must show them as described):".to_string()];
    all.extend(lines);
    all.join("\n")
}

/// The ids a scene mapping holds: a list, or a comma-separated string from older projects.
fn scene_map_ids(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(|i| crate::pyval::py_str(Some(i))).collect(),
        other => text_or_empty(other).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
    }
}

fn ids_value(ids: &[String]) -> Value {
    strings(ids)
}

/// Characters for scenes that have none: each act's cast goes onto its scenes. User mappings are kept, and clothing or props mapped to a
/// scene (ids not in `subject_ids`) stay beside the characters added. Returns `(new_map, changed_scene_ids)`.
pub fn fill_scene_cast(subject_scene_map: &Obj, acts: Option<&Value>, subject_ids: &[String]) -> (Obj, Vec<String>) {
    let mut result = subject_scene_map.clone();
    let valid: HashSet<&String> = subject_ids.iter().collect();
    let mut changed = Vec::new();
    for act in normalize_story_acts(acts) {
        let cast: Vec<String> = act.cast_ids.iter().filter(|c| valid.contains(c)).cloned().collect();
        if cast.is_empty() {
            continue;
        }
        for scene_id in &act.scene_ids {
            let current = scene_map_ids(result.get(scene_id));
            if current.iter().any(|i| valid.contains(i)) {
                continue;
            }
            let mut merged = cast.clone();
            merged.extend(current);
            result.insert(scene_id.clone(), ids_value(&merged));
            changed.push(scene_id.clone());
        }
    }
    (result, changed)
}

/// Each planned scene's characters (`cast_ids`) become the characters of its mapping. Clothing and props mapped to the scene (ids not in
/// `subject_ids`) are kept after them. Scenes the plan gives no cast keep theirs, or get their act's cast when they have none.
pub fn apply_scene_cast(subject_scene_map: &Obj, plan: Option<&Value>, acts: Option<&Value>, subject_ids: &[String]) -> (Obj, Vec<String>) {
    let mut result = subject_scene_map.clone();
    let valid: HashSet<&String> = subject_ids.iter().collect();
    let mut changed: Vec<String> = Vec::new();
    for entry in normalize_story_scene_plan(plan) {
        let cast: Vec<String> = entry.cast_ids.iter().filter(|c| valid.contains(c)).cloned().collect();
        if cast.is_empty() {
            continue;
        }
        let mut following = cast;
        following.extend(scene_map_ids(result.get(&entry.scene_id)).into_iter().filter(|i| !valid.contains(i)));
        let following_value = ids_value(&following);
        if result.get(&entry.scene_id) != Some(&following_value) {
            result.insert(entry.scene_id.clone(), following_value);
            changed.push(entry.scene_id.clone());
        }
    }
    let (result, filled) = fill_scene_cast(&result, acts, subject_ids);
    for scene_id in filled {
        if !changed.contains(&scene_id) {
            changed.push(scene_id);
        }
    }
    (result, changed)
}

/// Plain paragraphs from an old song-section story arc, for converting a project to Story Mode. Section headings (`Verse 1:`, `[Chorus 2]`)
/// and `Scene N (Location) -` prefixes are removed. Each section becomes one paragraph. The words themselves are kept.
pub fn story_arc_to_story_text(arc_text: &str) -> String {
    let heading = rx(
        r"(?i)^\s*(?:\[[^\]\n]{1,40}\]|(?:intro|outro|verse|chorus|pre[\s-]?chorus|post[\s-]?chorus|bridge|hook|refrain|interlude|break|instrumental|breakdown|drop|solo|tag|coda|part|section|act)\b[^:\n]{0,30}:)\s*",
    );
    let scene_prefix = rx(r"(?i)^\s*scene\s+\d+\s*(?:\([^)]*\))?\s*[\x{2014}\x{2013}:-]\s*");
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for raw in text_s(arc_text, 200_000).split('\n') {
        let mut line = raw.trim().to_string();
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current = Vec::new();
            }
            continue;
        }
        if let Some(found) = heading.find(&line).ok().flatten() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current = Vec::new();
            }
            line = line[found.end()..].trim().to_string();
            if line.is_empty() {
                continue;
            }
        }
        let line = scene_prefix.replace(&line, "").trim().to_string();
        if !line.is_empty() {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs.join("\n\n")
}
