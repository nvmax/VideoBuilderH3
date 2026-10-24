//! What the Story Mode steps read from a request: the story layer, the cast, the places the user wants and the song's sections. Port of the
//! helper functions of `storyboard/story_book.py`.

use fancy_regex::Regex;
use h3_core::pyval::{round_half_even, text_or_empty, to_float, to_int, truthy, Obj};
use h3_core::story::helpers::{dialogue_reference_catalog, first_truthy, normalize_story_layer};
use h3_core::story::mode::{is_story_person_card, normalize_story_video_style, text, text_s};
use h3_llm::instructions::default_text;
use serde_json::Value;

use crate::instructions::effective_instruction;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn squeeze(value: &str) -> String {
    rx(r"\s+").replace_all(value, " ").into_owned()
}

/// `_layer`: the story layer of the request, cleaned.
pub fn layer(payload: &Obj) -> Value {
    let given = first_truthy(payload, &["story_layer", "storyLayer"]).cloned().unwrap_or_else(|| serde_json::json!({}));
    normalize_story_layer(Some(&given))
}

/// `_scenes`: the scenes of the request that are objects.
pub fn scenes(payload: &Obj) -> Vec<Value> {
    match payload.get("scenes") {
        Some(Value::Array(items)) => items.iter().filter(|s| s.is_object()).cloned().collect(),
        _ => Vec::new(),
    }
}

fn float_or_zero(value: Option<&Value>) -> f64 {
    value.filter(|v| truthy(v)).map_or(Some(0.0), to_float).unwrap_or(0.0)
}

/// `_song_seconds`: the length given, or the end of the last scene.
pub fn song_seconds(payload: &Obj, scenes: &[Value]) -> f64 {
    let given = float_or_zero(payload.get("song_seconds"));
    if given > 0.0 {
        return given;
    }
    let mut end = 0.0f64;
    for scene in scenes {
        for key in ["timeline_end", "end"] {
            if let Some(value) = scene.get(key).filter(|v| truthy(v)).map_or(Some(0.0), to_float) {
                end = end.max(value);
            }
        }
    }
    end
}

/// `_scene_count`: the count given, or the number of scenes.
pub fn scene_count(payload: &Obj, scenes: &[Value]) -> i64 {
    let given = payload.get("scene_count").filter(|v| truthy(v)).and_then(to_int).unwrap_or(0);
    if given != 0 { given } else { scenes.len() as i64 }
}

fn id_of(value: &Value) -> String {
    text_or_empty(value.get("id"))
}

/// `story_cast`: the people the story is about, the ones picked in `story_cast_ids` or every person card when none are. Clothing and prop
/// cards are never part of the cast.
pub fn story_cast(payload: &Obj) -> Vec<Value> {
    let (subjects, _) = dialogue_reference_catalog(payload);
    let subjects: Vec<Value> = subjects.into_iter().filter(is_story_person_card).collect();
    let chosen: Vec<String> = match layer(payload).get("story_cast_ids") {
        Some(Value::Array(ids)) => ids.iter().map(|i| text_or_empty(Some(i))).collect(),
        _ => Vec::new(),
    };
    let picked: Vec<Value> = subjects.iter().filter(|s| chosen.contains(&id_of(s))).cloned().collect();
    if !chosen.is_empty() && !picked.is_empty() { picked } else { subjects }
}

fn role_label(role: &str) -> &'static str {
    match role {
        "main" => "main character",
        "love_interest" => "love interest",
        "antagonist" => "antagonist",
        _ => "supporting",
    }
}

fn relationship_label(kind: &str) -> &'static str {
    match kind {
        "love_interest" => "love interests",
        "partners" => "partners",
        "exes" => "exes",
        "family" => "family",
        "friends" => "friends",
        "rivals" => "rivals",
        "strangers" => "strangers",
        _ => "connected",
    }
}

/// `main_cast`: the main character card, the one picked in `story_main_cast_id` or the first story cast card.
pub fn main_cast(payload: &Obj) -> Option<Value> {
    let cast = story_cast(payload);
    let chosen = text_or_empty(layer(payload).get("story_main_cast_id").filter(|v| truthy(v)));
    cast.iter().find(|s| id_of(s) == chosen).cloned().or_else(|| cast.first().cloned())
}

/// `cast_role`: the subject's role in the story. The main character is always `main`. Others default to supporting.
pub fn cast_role(payload: &Obj, subject: &Value) -> String {
    if let Some(main) = main_cast(payload) {
        if id_of(&main) == id_of(subject) {
            return "main".into();
        }
    }
    let roles = layer(payload).get("story_cast_roles").cloned();
    let role = roles.as_ref().and_then(|r| r.get(id_of(subject))).map(|r| text_or_empty(Some(r))).unwrap_or_else(|| "supporting".into());
    if role == "main" { "supporting".into() } else { role }
}

/// `relationship_lines`: `- Mara and Joe: love interests (estranged)` for each relationship between story cast members.
pub fn relationship_lines(payload: &Obj) -> Vec<String> {
    let names: Vec<(String, String)> = story_cast(payload).iter().map(|s| (id_of(s), text_or_empty(s.get("name")))).collect();
    let name_of = |id: &str| names.iter().rev().find(|(i, _)| i == id).map(|(_, n)| n.clone());
    let mut lines = Vec::new();
    if let Some(Value::Array(items)) = layer(payload).get("story_relationships") {
        for item in items {
            let (a, b) = (text_or_empty(item.get("a")), text_or_empty(item.get("b")));
            if let (Some(first), Some(second)) = (name_of(&a), name_of(&b)) {
                let note = text_or_empty(item.get("note"));
                let note = if note.is_empty() { String::new() } else { format!(" ({note})") };
                lines.push(format!("- {first} and {second}: {}{note}", relationship_label(&text_or_empty(item.get("type")))));
            }
        }
    }
    lines
}

/// `cast_lines`: `Name (type, role): description` for every character the story uses, then their relationships.
pub fn cast_lines(payload: &Obj) -> String {
    let mut lines: Vec<String> = Vec::new();
    let story = layer(payload);
    for subject in story_cast(payload) {
        let kind = Some(text_or_empty(subject.get("reference_type"))).filter(|k| !k.is_empty()).unwrap_or_else(|| "character".into());
        let role = role_label(&cast_role(payload, &subject));
        let description = text_or_empty(subject.get("description"));
        lines.push(format!("- {} ({kind}, {role})", text_or_empty(subject.get("name"))) + &if description.is_empty() { String::new() } else { format!(": {description}") });
        let personality = story.get("story_character_notes").and_then(|n| n.get(id_of(&subject))).map(|n| text_or_empty(Some(n))).filter(|n| !n.is_empty());
        if let Some(personality) = personality {
            lines.push(format!("  Personality and emotions: {personality}"));
        }
    }
    let relationships = relationship_lines(payload);
    if !relationships.is_empty() {
        lines.push(String::new());
        lines.push("Relationships:".into());
        lines.extend(relationships);
    }
    lines.join("\n")
}

fn reference_locations(payload: &Obj) -> Vec<Value> {
    let refs = first_truthy(payload, &["reference_builder", "referenceBuilder"]);
    match refs.and_then(|r| r.get("locations")) {
        Some(Value::Array(cards)) => cards.clone(),
        _ => Vec::new(),
    }
}

/// `_loaded_places_text`: one `Name: description` line per location card the user loaded in the Reference Builder (cards the story made for
/// itself carry a `story_setting_id` and are left out).
fn loaded_places_text(payload: &Obj) -> String {
    let mut lines = Vec::new();
    for card in reference_locations(payload) {
        let Some(map) = card.as_object() else { continue };
        if map.get("story_setting_id").is_some_and(truthy) {
            continue;
        }
        let name = squeeze(&text_or_empty(map.get("name")).replace(':', " ")).trim().to_string();
        if name.is_empty() {
            continue;
        }
        let details = squeeze(&text(map.get("description"), 300));
        lines.push(if details.is_empty() { name } else { format!("{name}: {details}") });
    }
    lines.join("\n")
}

/// `user_places_text`: the places the user wants in the video (one per line), or nothing. The Story tab's Locations box comes first. Without
/// it, the location cards the user already loaded stand in, so the story is written around them.
pub fn user_places_text(payload: &Obj) -> String {
    let typed = text(layer(payload).get("story_user_locations"), 3000);
    if !typed.is_empty() { typed } else { text_s(&loaded_places_text(payload), 3000) }
}

/// `user_places`: `(name, details)` from the user's places, one per line with the details after the first colon.
pub fn user_places(payload: &Obj) -> Vec<(String, String)> {
    let mut places = Vec::new();
    for line in h3_core::timeline::py_splitlines(&user_places_text(payload)) {
        let (name, details) = line.split_once(':').map_or((line.as_str(), ""), |(a, b)| (a, b));
        let name = squeeze(name).trim_matches([' ', '-', '*']).to_string();
        if !name.is_empty() {
            places.push((name, details.trim().to_string()));
        }
    }
    places
}

/// `made_place_cards`: `(spot name, description)` for the story location cards already made in the Places step. With a `setting_id` only that
/// setting's cards and the user's own places count. The spot name is the card name without its `Setting - ` prefix.
pub fn made_place_cards(payload: &Obj, setting_id: &str) -> Vec<(String, String)> {
    let settings: Vec<(String, String)> = match layer(payload).get("story_settings") {
        Some(Value::Array(items)) => items.iter().map(|i| (text_or_empty(i.get("id")), text_or_empty(i.get("name")))).collect(),
        _ => Vec::new(),
    };
    let own: Vec<String> = user_places(payload).into_iter().map(|(n, _)| n.to_lowercase()).collect();
    let mut result = Vec::new();
    for card in reference_locations(payload) {
        let Some(map) = card.as_object() else { continue };
        if text_or_empty(map.get("name")).trim().is_empty() {
            continue;
        }
        let name = text(map.get("name"), 160);
        let card_setting = text_or_empty(map.get("story_setting_id"));
        let is_own = own.contains(&name.to_lowercase());
        if card_setting.is_empty() && !is_own {
            continue;
        }
        if !setting_id.is_empty() && !card_setting.is_empty() && card_setting != setting_id {
            continue;
        }
        let setting_name = settings.iter().rev().find(|(i, _)| *i == card_setting).map(|(_, n)| n.clone()).unwrap_or_default();
        let prefix = format!("{setting_name} - ");
        let spot = if !card_setting.is_empty() && name.to_lowercase().starts_with(&prefix.to_lowercase()) { name.chars().skip(prefix.chars().count()).collect() } else { name };
        result.push((spot, text(map.get("description"), 300)));
    }
    result
}

/// `made_places_text`: one `- Spot: description` line per made place card, or nothing when there are none.
pub fn made_places_text(payload: &Obj, setting_id: &str) -> String {
    made_place_cards(payload, setting_id).iter().map(|(spot, description)| if description.is_empty() { format!("- {spot}") } else { format!("- {spot}: {description}") }).collect::<Vec<_>>().join("\n")
}

fn words(value: &str) -> std::collections::HashSet<String> {
    let cleaned = rx(r"[^a-z0-9' ]+").replace_all(&value.to_lowercase(), " ").into_owned();
    cleaned.split_whitespace().filter(|w| *w != "the").map(str::to_string).collect()
}

/// `match_user_place`: the user's place a setting or spot name refers to. Names match without case or a leading "the", and all the words of
/// the shorter name must be words of the longer one ("Kitchen" is "Grandma's kitchen").
pub fn match_user_place(places: &[(String, String)], names: &[&str]) -> Option<(String, String)> {
    for (name, details) in places {
        let own = words(name);
        for candidate in names {
            let item = words(candidate);
            if !own.is_empty() && !item.is_empty() && (own.is_subset(&item) || item.is_subset(&own)) {
                return Some((name.clone(), details.clone()));
            }
        }
    }
    None
}

fn clock(seconds: f64) -> String {
    let seconds = round_half_even(seconds).max(0);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn number(value: Option<&Value>) -> f64 {
    value.and_then(to_float).unwrap_or(0.0)
}

/// `song_map`: the song in order, one line per section: `0:12-0:40 Verse 1: lyric / lyric`. Neighbouring scenes with the same section label
/// are one section. Scenes without a label are grouped into runs of vocal or instrumental scenes.
pub fn song_map(scenes: &[Value]) -> String {
    struct Group {
        key: String,
        label: String,
        start: f64,
        end: f64,
        lyrics: Vec<String>,
    }
    let mut groups: Vec<Group> = Vec::new();
    for scene in scenes {
        let lyric = text(first_truthy(scene.as_object().unwrap_or(&Obj::new()), &["lyrics", "lyric_text"]), 300).replace('\n', " / ");
        let instrumental = scene.get("lyric_instrumental").is_some_and(truthy) || lyric.is_empty() || lyric.trim_matches(['[', ']', ' ']).to_lowercase() == "instrumental";
        let section = text(scene.get("lyric_section"), 60);
        let key = if section.is_empty() { if instrumental { "instrumental".to_string() } else { "vocal".to_string() } } else { section.to_lowercase() };
        let start = number(scene.get("timeline_start").or_else(|| scene.get("start")));
        let end = number(scene.get("timeline_end").or_else(|| scene.get("end")));
        if let Some(last) = groups.last_mut().filter(|g| g.key == key) {
            last.end = last.end.max(end);
            if !instrumental {
                last.lyrics.push(lyric);
            }
            continue;
        }
        groups.push(Group {
            key,
            label: if section.is_empty() { if instrumental { "Instrumental".into() } else { "Vocals".into() } } else { section },
            start,
            end,
            lyrics: if instrumental { Vec::new() } else { vec![lyric] },
        });
    }
    let mut lines = Vec::new();
    for group in groups {
        let words: String = group.lyrics.join(" / ").chars().take(600).collect();
        lines.push(format!("{}-{} {}", clock(group.start), clock(group.end), group.label) + &if words.is_empty() { " (no vocals)".to_string() } else { format!(": {words}") });
    }
    lines.join("\n")
}

/// `video_style`: how the story is told on screen (`music_video`, `spoken` or `story_only`).
pub fn video_style(payload: &Obj) -> String {
    normalize_story_video_style(layer(payload).get("story_video_style"))
}

/// `lead_name`: the main character's name, used as the performer in the prompts.
pub fn lead_name(payload: &Obj) -> String {
    main_cast(payload).map(|m| text_or_empty(m.get("name"))).unwrap_or_default()
}

/// `_lyrics`: the lyrics of the request, or the lyric of each scene.
pub fn lyrics_of(payload: &Obj, scenes: &[Value]) -> String {
    let lyrics = text(first_truthy(payload, &["lyrics", "lyrics_text"]), 12000);
    if !lyrics.is_empty() {
        return lyrics;
    }
    let joined = scenes.iter().map(|s| text(first_truthy(s.as_object().unwrap_or(&Obj::new()), &["lyrics", "lyric_text"]), 400)).collect::<Vec<_>>().join("\n");
    joined.chars().take(12000).collect()
}

/// `story_template`: the project's own text for a Story Mode instruction (`story_mode_*`), or its built-in default.
pub fn story_template(payload: &Obj, key: &str) -> String {
    effective_instruction(payload, key, default_text(key).unwrap_or(""))
}
