//! Reading location and subject cards out of a model's reply, and mapping scenes to locations. Port of the location and subject
//! helpers in `llm/output_checks.py`.

use std::collections::{HashMap, HashSet};

use fancy_regex::Regex;
use h3_core::pyval::{text_or_empty, to_int};
use h3_core::timeline::py_splitlines;
use serde_json::Value;

use crate::checks::clean_visual_text;
use crate::locations::{extract_json_object, looks_like_location_meta_text, Location};

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn is_match(pattern: &str, text: &str) -> bool {
    rx(pattern).is_match(text).unwrap_or(false)
}

fn collapse(text: &str) -> String {
    rx(r"\s+").replace_all(text, " ").into_owned()
}

fn lower_tokens(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit()).filter(|t| !t.is_empty()).map(str::to_string).collect()
}

fn has_any(tokens: &HashSet<String>, words: &[&str]) -> bool {
    words.iter().any(|w| tokens.contains(*w))
}

const PLACE_WORDS: &[&str] = &[
    "alley", "apartment", "arena", "attic", "ballroom", "bar", "barn", "bathroom", "beach", "bedroom", "bridge", "building", "cabin", "cafe", "casino", "cathedral", "cave",
    "chapel", "church", "city", "club", "corridor", "courtyard", "desert", "diner", "dock", "factory", "field", "forest", "foyer", "garage", "garden", "greenhouse", "hall",
    "hallway", "harbor", "hotel", "house", "kitchen", "lab", "lake", "lounge", "mansion", "market", "motel", "museum", "office", "palace", "parking", "pier", "pool", "quarry",
    "railway", "road", "rooftop", "room", "school", "set", "shore", "stage", "station", "street", "studio", "subway", "temple", "theater", "tower", "train", "tunnel", "vault",
    "warehouse", "workshop",
];

const OBJECT_WORDS: &[&str] = &[
    "bottle", "bowl", "bracelet", "camera", "candle", "chair", "collar", "comb", "crown", "dress", "frame", "glass", "kit", "knife", "locket", "mirror", "necklace", "perfume",
    "phone", "pocket", "razor", "rice", "ring", "shaving", "shoe", "sink", "sugar", "table", "tooth", "vanity", "veil", "window",
];

const SURFACE_WORDS: &[&str] = &["ceiling", "corner", "floor", "partition", "partitions", "wall", "walls"];

const CHARACTER_WORDS: &[&str] = &["bride", "character", "face", "ghost", "girl", "hand", "human", "man", "person", "silhouette", "subject", "woman"];

fn alternation(words: &[&str]) -> String {
    let mut sorted: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    sorted.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    sorted.join("|")
}

fn place_pattern() -> String {
    format!("(?:{})", alternation(PLACE_WORDS))
}

fn surface_or_object_pattern() -> String {
    let all: Vec<&str> = SURFACE_WORDS.iter().chain(OBJECT_WORDS.iter()).copied().collect();
    alternation(&all)
}

fn has_non_place_word(tokens: &HashSet<String>) -> bool {
    has_any(tokens, OBJECT_WORDS) || has_any(tokens, SURFACE_WORDS) || has_any(tokens, CHARACTER_WORDS)
}

const OPEN_ENDED: &str = r"(?:a|an|the|with|without|under|beneath|behind|beside|inside|outside|near|through|over|heavy|long|thick|deep|wide|large|small|empty|covered)";

fn nice_location_name(value: &str) -> String {
    let text = collapse(value.trim_matches([' ', ',', '.', '-']));
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn text_has_non_place_focus(value: &str) -> bool {
    let tokens: HashSet<String> = lower_tokens(value).into_iter().collect();
    has_non_place_word(&tokens)
}

/// `_normalize_location_name`: a name that trails off or names an object or a wall is cut back to the place.
pub fn normalize_location_name(name: &str) -> String {
    let text = collapse(name.trim_matches([' ', ',', '.', '-']));
    if text.is_empty() {
        return String::new();
    }
    let lowered = text.to_lowercase();
    let place = place_pattern();
    let tokens: HashSet<String> = lower_tokens(&lowered).into_iter().collect();

    if is_match(&format!(r"\b{OPEN_ENDED}\s*$"), &lowered) {
        if let Some(found) = rx(&format!(r"(?i)^(.{{3,80}}?\b{place}\b)")).captures(&text).ok().flatten().and_then(|c| c.get(1)) {
            return nice_location_name(found.as_str());
        }
    }

    if has_non_place_word(&tokens) {
        let preposition = rx(&format!(r"(?i)\b(?:in|inside|within|at|on|near|beside|behind|beneath|under)\s+(?:a|an|the)?\s*([^,.;]*?\b{place}\b(?:\s+\w+)?)"));
        if let Some(found) = preposition.captures(&text).ok().flatten().and_then(|c| c.get(1)) {
            return nice_location_name(found.as_str());
        }
        let extras = surface_or_object_pattern();
        let surface = rx(&format!(r"(?i)^(.{{3,80}}?\b{place}\b)\s+(?:{extras})\b"));
        if let Some(found) = surface.captures(&text).ok().flatten().and_then(|c| c.get(1)) {
            return nice_location_name(found.as_str());
        }
        let trailing = rx(&format!(r"(?i)\b({place})\s+(?:{extras})\b"));
        if let Some(found) = trailing.captures(&text).ok().flatten().and_then(|c| c.get(1)) {
            return nice_location_name(text[..found.end()].trim());
        }
    }

    nice_location_name(&text)
}

/// `_clean_location_description`: the details after the name without people or props in them.
fn clean_location_description(name: &str, description: &str) -> String {
    let name_text = collapse(name.trim());
    let desc = collapse(description.trim());
    if desc.is_empty() {
        return name_text;
    }
    let fragments: Vec<String> = rx(r"\s*,\s*").split(&desc).flatten().map(|p| p.trim_matches([' ', '.']).to_string()).filter(|p| !p.is_empty()).collect();
    let mut kept = Vec::new();
    for (index, fragment) in fragments.iter().enumerate() {
        let lower = fragment.to_lowercase();
        if index > 0 && is_match(r"\b(?:bride|woman|man|girl|boy|face|hand|human|silhouette|wearing|watching|pressed|resting|trailing|caught|hidden|dissolving|reflecting|smelling)\b", &lower) {
            continue;
        }
        if index > 0 && is_match(r"\b(?:razor|tooth|dress|veil|bottle|locket|collar|frame|kit|rice|sugar bowl)\b", &lower) {
            continue;
        }
        kept.push(fragment.clone());
    }
    let mut cleaned = kept.join(", ").trim().to_string();
    if cleaned.is_empty() {
        cleaned = name_text.clone();
    }
    if !name_text.is_empty() && !cleaned.to_lowercase().starts_with(&name_text.to_lowercase()) {
        cleaned = format!("{name_text}, {cleaned}");
    }
    cleaned
}

/// `_clean_location_card`: the name and description of a card, tidied. `raw_name` is what the model wrote.
pub fn clean_location_card(name: &str, description: &str, raw_name: Option<&str>) -> (String, String) {
    let normalized = normalize_location_name(name);
    let mut cleaned = clean_location_description(&normalized, description);
    let raw_text = raw_name.unwrap_or(name);
    if normalized.to_lowercase() != collapse(raw_text.trim_matches([' ', ',', '.', '-'])).to_lowercase() && text_has_non_place_focus(raw_text) {
        cleaned = normalized.clone();
    }
    (normalized, cleaned)
}

fn name_fragment_is_complete(fragment: &str) -> bool {
    let text = collapse(fragment.trim()).to_lowercase();
    if text.is_empty() || is_match(&format!(r"\b{OPEN_ENDED}\s*$"), &text) {
        return false;
    }
    is_match(&format!(r"(?i)\b{}\b", place_pattern()), &text)
}

fn name_is_place(name: &str) -> bool {
    let text = collapse(name.trim()).to_lowercase();
    if text.is_empty() {
        return false;
    }
    let tokens: HashSet<String> = lower_tokens(&text).into_iter().collect();
    if has_any(&tokens, PLACE_WORDS) {
        if has_non_place_word(&tokens) {
            let normalized = normalize_location_name(name).to_lowercase();
            let normalized_tokens: HashSet<String> = lower_tokens(&normalized).into_iter().collect();
            if has_non_place_word(&normalized_tokens) {
                return false;
            }
        }
        return true;
    }
    if has_non_place_word(&tokens) {
        return false;
    }
    is_match(r"\b(?:inside|outside|interior|exterior|room|space|area|zone)\b", &text)
}

/// `_valid_location_card`: a place, not an object or a person.
pub fn valid_location_card(name: &str, description: &str) -> bool {
    if !name_is_place(name) {
        return false;
    }
    let combined = format!("{name} {description}").to_lowercase();
    if is_match(r"\b(?:character|subject|woman|man|girl|boy|bride|ghostly face|human tooth|hand pressed)\b", &combined) {
        let without_name = description.to_lowercase().replace(&name.to_lowercase(), "");
        if !is_match(r"\b(?:room|hall|hallway|corridor|street|road|forest|garden|stage|kitchen|bathroom|bedroom|ballroom|pool|motel|warehouse|studio|city)\b", &without_name) {
            return false;
        }
    }
    true
}

fn usable_card(name: &str, description: &str) -> bool {
    !(name.is_empty() || looks_like_location_meta_text(name) || looks_like_location_meta_text(description) || !valid_location_card(name, description))
}

/// `_parse_location_lines`: lines like `1|bedroom|small room with a lamp`.
pub fn parse_location_lines(text: &str) -> Vec<Location> {
    let numbered = rx(r"(?i)^\s*(?:Location\s*)?(\d+)\s*(?:[|:=\).-])\s*(.+?)\s*$");
    let name_label = rx(r"(?i)^\s*name\s*[:=]\s*");
    let description_label = rx(r"(?i)^\s*description\s*[:=]\s*");
    let mut locations: Vec<Location> = Vec::new();
    let mut seen = HashSet::new();
    for raw in py_splitlines(text) {
        let line = raw.trim().trim_matches('-').trim().to_string();
        if line.is_empty() || matches!(line.as_str(), "{" | "}" | "[" | "]") {
            continue;
        }
        let Some(rest) = numbered.captures(&line).ok().flatten().and_then(|c| c.get(2)).map(|m| m.as_str().trim().trim_matches('"').trim_end_matches(',').to_string()) else { continue };
        let parts: Vec<String> = rest.split('|').map(|p| p.trim().trim_matches('"').to_string()).collect();
        let (name, description) = if parts.len() >= 2 { (parts[0].clone(), parts[1..].join(" | ")) } else { (rest.clone(), rest.clone()) };
        let name = name_label.replace(&name, "").into_owned();
        let description = description_label.replace(&description, "").into_owned();
        let (name, description) = clean_location_card(&name, &description, Some(&name.clone()));
        if !usable_card(&name, &description) || seen.contains(&name.to_lowercase()) {
            continue;
        }
        seen.insert(name.to_lowercase());
        locations.push(Location { name, description });
    }
    locations
}

fn parse_location_idea_lines(text: &str) -> Vec<Location> {
    let heading = rx(r"(?i)^\s*music\s+video\s+locations\s*:?\s*$");
    let bullet = rx(r"^\s*(?:[-*\x{2022}]+|\d+[\).:-])\s*");
    let label = rx(r"(?i)^(?:location ideas?|output|user input)\s*:?\s*$");
    let split = rx(r"^(.{4,80}?)(?:\s+-\s+|\s+--\s+|:\s+)(.{4,})$");
    let mut locations: Vec<Location> = Vec::new();
    let mut seen = HashSet::new();
    for raw in py_splitlines(text) {
        let line = raw.trim();
        if line.is_empty() || heading.is_match(line).unwrap_or(false) {
            continue;
        }
        let line = bullet.replace(line, "").trim().to_string();
        let line = line.trim_matches('"').trim().to_string();
        if line.is_empty() || matches!(line.as_str(), "{" | "}" | "[" | "]") || line.starts_with('{') || line.starts_with('[') || label.is_match(&line).unwrap_or(false) {
            continue;
        }
        let (mut name, mut description) = (line.clone(), line.clone());
        if let Some(caps) = split.captures(&line).ok().flatten() {
            name = caps[1].trim().to_string();
            description = caps[2].trim().to_string();
        } else if let Some((head, _)) = line.split_once(',') {
            let length = head.trim().chars().count();
            if (4..=60).contains(&length) && name_fragment_is_complete(head) {
                name = head.trim().to_string();
            }
        }
        let (name, description) = clean_location_card(&name.clone(), &description, Some(&name));
        if !usable_card(&name, &description) || seen.contains(&name.to_lowercase()) {
            continue;
        }
        seen.insert(name.to_lowercase());
        locations.push(Location { name, description });
    }
    locations
}

fn first_truthy<'a>(object: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().filter_map(|k| object.get(*k)).find(|v| h3_core::pyval::truthy(v))
}

/// `_parse_location_ideas_flexible`: bullets, then JSON, then sentences.
pub fn parse_location_ideas_flexible(text: &str) -> Vec<Location> {
    let found = parse_location_idea_lines(text);
    if !found.is_empty() {
        return found;
    }
    let cleaned = clean_visual_text(text);
    if let Ok(data) = extract_json_object(&cleaned) {
        let raw = match &data {
            Value::Object(map) => first_truthy(map, &["locations", "location_ideas", "music_video_locations"]).cloned(),
            other => Some(other.clone()),
        };
        if let Some(Value::Array(items)) = raw {
            let mut parsed = Vec::new();
            for item in &items {
                let (raw_name, name, description) = match item {
                    Value::Object(map) => {
                        let raw_name = text_or_empty(first_truthy(map, &["name", "location", "idea"]));
                        let name = normalize_location_name(&raw_name);
                        let detail = text_or_empty(first_truthy(map, &["description", "detail", "visual_detail"]));
                        let description = collapse(if detail.is_empty() { name.trim() } else { detail.trim() });
                        (raw_name, name, description)
                    }
                    other => {
                        let raw_name = h3_core::pyval::py_str(Some(other));
                        let name = normalize_location_name(&raw_name);
                        (raw_name, name.clone(), name)
                    }
                };
                let basis = if description.is_empty() { name.clone() } else { description };
                let (name, description) = clean_location_card(&name, &basis, Some(&raw_name));
                if !name.is_empty() && !looks_like_location_meta_text(&name) && !looks_like_location_meta_text(&description) && valid_location_card(&name, &description) {
                    let description = if description.is_empty() { name.clone() } else { description };
                    parsed.push(Location { name, description });
                }
            }
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    let bulletish: Vec<String> = rx(r"(?:\n+|(?<=\.)\s+(?=[A-Z][A-Za-z ]{4,70}(?:\s+-|:)))").split(&cleaned).flatten().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    let parsed = parse_location_idea_lines(&bulletish.join("\n"));
    if !parsed.is_empty() {
        return parsed;
    }
    let lead = rx(r"(?i)^\s*(?:Music Video Locations|Locations|Location ideas)\s*:?\s*");
    let mut candidates = Vec::new();
    for part in rx(r";|\n").split(&cleaned).flatten() {
        let part = lead.replace(part.trim(), "").into_owned();
        let length = part.chars().count();
        if (4..=180).contains(&length) && !looks_like_location_meta_text(&part) && !is_match(r"(?i)\b(?:sorry|cannot|unable|lyrics|song meaning|summary)\b", &part) {
            candidates.push(part);
        }
    }
    parse_location_idea_lines(&candidates.iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n"))
}

/// A subject or location a scene can use: `name` and `description`.
#[derive(Debug, Clone, PartialEq)]
pub struct Subject {
    pub name: String,
    pub description: String,
}

/// `_parse_subject_lines`: `1|young singer|description` or `young singer: description`.
pub fn parse_subject_lines(text: &str) -> Vec<Subject> {
    let piped = rx(r"^\s*(?:\d+[\).:\-|]\s*)?([^|:]+?)\s*\|\s*(.+)$");
    let colon = rx(r"^\s*(?:\d+[\).:\-|]\s*)?([^:]+?)\s*:\s*(.+)$");
    let mut subjects = Vec::new();
    for raw in py_splitlines(text) {
        let line = raw.trim().trim_matches(['-', '*', ' ']).to_string();
        if line.is_empty() {
            continue;
        }
        let caps = match piped.captures(&line).ok().flatten() {
            Some(caps) => caps,
            None => match colon.captures(&line).ok().flatten() {
                Some(caps) => caps,
                None => continue,
            },
        };
        let (name, description) = (collapse(caps[1].trim()), collapse(caps[2].trim()));
        if !name.is_empty() && !description.is_empty() {
            subjects.push(Subject { name, description });
        }
    }
    subjects
}

// -- mapping scenes to locations -------------------------------------------------------------------------------------------

/// A scene as the location mapper sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub id: String,
    pub label: String,
    pub concept: String,
    pub notes: String,
}

/// A dict of scene id to location name that keeps the order things were added in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneMap(Vec<(String, String)>);

impl SceneMap {
    pub fn get(&self, id: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == id).map(|(_, v)| v.as_str())
    }

    pub fn insert(&mut self, id: &str, name: &str) {
        match self.0.iter_mut().find(|(k, _)| k == id) {
            Some(entry) => entry.1 = name.to_string(),
            None => self.0.push((id.to_string(), name.to_string())),
        }
    }

    /// Python `dict.setdefault`.
    pub fn set_default(&mut self, id: &str, name: &str) {
        if self.get(id).is_none() {
            self.0.push((id.to_string(), name.to_string()));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// `_parse_scene_location_number_map`: lines like `Scene1=2`.
pub fn parse_scene_location_number_map(text: &str, scenes: &[Scene], locations: &[Location]) -> SceneMap {
    let mut scene_by_label: HashMap<String, String> = HashMap::new();
    let mut scene_by_number: HashMap<String, String> = HashMap::new();
    for (index, scene) in scenes.iter().enumerate() {
        let number = index + 1;
        scene_by_number.insert(number.to_string(), scene.id.clone());
        scene_by_label.insert(scene.id.to_lowercase(), scene.id.clone());
        scene_by_label.insert(scene.label.to_lowercase(), scene.id.clone());
        scene_by_label.insert(format!("scene {number}"), scene.id.clone());
        scene_by_label.insert(format!("scene{number}"), scene.id.clone());
    }
    let mut location_by_number: HashMap<String, String> = HashMap::new();
    let mut location_by_name: HashMap<String, String> = HashMap::new();
    for (index, location) in locations.iter().enumerate() {
        location_by_number.insert((index + 1).to_string(), location.name.clone());
        location_by_name.insert(location.name.to_lowercase(), location.name.clone());
    }
    let line_pattern = rx(r#"(?i)^\s*(?:Scene\s*)?("?[^"=:\|]+"?)\s*(?:=|:|\|)\s*(?:Location\s*)?("?[^",\s]+"?)"#);
    let mut scene_map = SceneMap::default();
    for raw in py_splitlines(text) {
        let line = raw.trim().trim_matches(',');
        if line.is_empty() {
            continue;
        }
        let Some(caps) = line_pattern.captures(line).ok().flatten() else { continue };
        let raw_scene = caps[1].trim().trim_matches('"').to_lowercase();
        let raw_location = caps[2].trim().trim_matches('"').to_lowercase();
        let non_empty = |v: Option<&String>| v.filter(|s| !s.is_empty()).cloned();
        let scene_id = non_empty(scene_by_number.get(&raw_scene)).or_else(|| non_empty(scene_by_label.get(&raw_scene))).or_else(|| non_empty(scene_by_label.get(&raw_scene.replace(' ', ""))));
        let location_name = non_empty(location_by_number.get(&raw_location)).or_else(|| non_empty(location_by_name.get(&raw_location)));
        if let (Some(id), Some(name)) = (scene_id, location_name) {
            scene_map.insert(&id, &name);
        }
    }
    scene_map
}

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "by", "for", "from", "in", "into", "is", "it", "of", "on", "or", "the", "to", "with", "scene", "shot", "cinematic", "woman", "man", "girl",
    "boy", "subject", "character", "wearing", "light", "lighting",
];

/// `_location_text_overlap_score`: 3 for each word of the location the scene also uses, 1 for a shared word stem.
pub fn location_text_overlap_score(scene_text: &str, location_text: &str) -> i64 {
    let keep = |text: &str| -> Vec<String> { lower_tokens(text).into_iter().filter(|t| t.len() > 2 && !STOP_WORDS.contains(&t.as_str())).collect() };
    let scene_tokens: HashSet<String> = keep(scene_text).into_iter().collect();
    let location_tokens = keep(location_text);
    if scene_tokens.is_empty() || location_tokens.is_empty() {
        return 0;
    }
    let mut score = 0;
    for token in &location_tokens {
        if scene_tokens.contains(token) {
            score += 3;
        } else if scene_tokens.iter().any(|s| s.len() > 4 && (s.starts_with(token.as_str()) || token.starts_with(s.as_str()))) {
            score += 1;
        }
    }
    score
}

fn scene_text(scene: &Scene) -> String {
    format!("{} {}", scene.concept, scene.notes)
}

fn card_text(location: &Location) -> String {
    format!("{} {}", location.name, location.description)
}

fn best_location_for_scene<'a>(scene: &Scene, locations: &'a [Location]) -> Option<&'a Location> {
    let mut best = locations.first()?;
    let mut best_score = -1;
    for location in locations {
        let score = location_text_overlap_score(&scene_text(scene), &card_text(location));
        if score > best_score {
            best = location;
            best_score = score;
        }
    }
    Some(best)
}

/// `_fallback_location_map_by_overlap`: each scene gets the location that shares the most words with its text.
pub fn fallback_location_map_by_overlap(scenes: &[Scene], locations: &[Location]) -> SceneMap {
    let mut mapped = SceneMap::default();
    for scene in scenes {
        let name = best_location_for_scene(scene, locations).map(|l| l.name.clone()).unwrap_or_else(|| "Location 1".into());
        mapped.insert(&scene.id, &name);
    }
    mapped
}

fn canonical_location_name(name: &str, locations: &[Location]) -> String {
    let raw = collapse(name.trim()).to_lowercase();
    for location in locations {
        let candidate = collapse(location.name.trim());
        if candidate.to_lowercase() == raw {
            return candidate;
        }
    }
    String::new()
}

/// `_location_usage_counts_from_payload`: how often each location has been used before, from `used_location_counts` and
/// `previous_assignments`.
pub fn location_usage_counts(used_counts: Option<&Value>, assignments: Option<&Value>, locations: &[Location]) -> Vec<(String, i64)> {
    let mut counts: Vec<(String, i64)> = Vec::new();
    for location in locations {
        let name = collapse(location.name.trim());
        if !name.is_empty() && !counts.iter().any(|(n, _)| *n == name) {
            counts.push((name, 0));
        }
    }
    let set = |counts: &mut Vec<(String, i64)>, name: &str, value: i64| {
        match counts.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = value,
            None => counts.push((name.to_string(), value)),
        }
    };
    let get = |counts: &Vec<(String, i64)>, name: &str| counts.iter().find(|(n, _)| n == name).map_or(0, |(_, c)| *c);
    if let Some(Value::Object(map)) = used_counts {
        for (raw_name, raw_count) in map {
            let name = canonical_location_name(raw_name, locations);
            if !name.is_empty() {
                let current = get(&counts, &name);
                let value = if h3_core::pyval::truthy(raw_count) { to_int(raw_count).map(|n| n.max(0)).unwrap_or(current) } else { 0 };
                set(&mut counts, &name, value);
            }
        }
    }
    if let Some(Value::Array(items)) = assignments {
        for item in items {
            let raw = match item {
                Value::Object(map) => text_or_empty(first_truthy(map, &["location", "location_name"])),
                other => text_or_empty(Some(other)),
            };
            let name = canonical_location_name(&raw, locations);
            if !name.is_empty() {
                let current = get(&counts, &name);
                set(&mut counts, &name, current + 1);
            }
        }
    }
    counts
}

/// `_balance_location_map_by_usage`: every location gets used before any repeats, preferring the least used ones.
pub fn balance_location_map_by_usage(scene_map: SceneMap, scenes: &[Scene], locations: &[Location], previous: &[(String, i64)]) -> SceneMap {
    if scene_map.is_empty() || scenes.is_empty() || locations.is_empty() {
        return scene_map;
    }
    let mut location_by_name: HashMap<String, &Location> = HashMap::new();
    let mut location_names: Vec<String> = Vec::new();
    for location in locations {
        let name = collapse(location.name.trim());
        if !name.is_empty() {
            if !location_by_name.contains_key(&name) {
                location_names.push(name.clone());
            }
            location_by_name.insert(name, location);
        }
    }
    if location_names.len() <= 1 {
        return scene_map;
    }
    let fallback = fallback_location_map_by_overlap(scenes, locations);
    let mut balanced = SceneMap::default();
    for scene in scenes {
        let named = canonical_location_name(scene_map.get(&scene.id).unwrap_or(""), locations);
        let name = if named.is_empty() { fallback.get(&scene.id).unwrap_or("").to_string() } else { named };
        balanced.insert(&scene.id, &name);
    }
    let previous_count = |name: &str| previous.iter().find(|(n, _)| n == name).map_or(0, |(_, c)| *c);
    let mut current: HashMap<String, i64> = location_names.iter().map(|n| (n.clone(), 0)).collect();
    for (_, name) in balanced.iter() {
        if let Some(count) = current.get_mut(name) {
            *count += 1;
        }
    }
    let target = scenes.len().min(location_names.len());
    let mut desired: Vec<String> = location_names.clone();
    desired.sort_by_key(|name| (previous_count(name), current.get(name).copied().unwrap_or(0), location_names.iter().position(|n| n == name).unwrap_or(0)));
    desired.truncate(target);

    for desired_name in &desired {
        if current.get(desired_name).copied().unwrap_or(0) > 0 {
            continue;
        }
        let blank = Location { name: desired_name.clone(), description: String::new() };
        let desired_location: &Location = location_by_name.get(desired_name).copied().unwrap_or(&blank);
        let mut best: Option<(&Scene, i64)> = None;
        for scene in scenes {
            let current_name = balanced.get(&scene.id).unwrap_or("").to_string();
            if current_name == *desired_name {
                continue;
            }
            if current.get(&current_name).copied().unwrap_or(0) <= 1 && desired.iter().any(|n| n != desired_name && current.get(n).copied().unwrap_or(0) == 0) {
                continue;
            }
            let text = scene_text(scene);
            let desired_score = location_text_overlap_score(&text, &card_text(desired_location));
            let current_blank = Location { name: current_name.clone(), description: String::new() };
            let current_location: &Location = location_by_name.get(&current_name).copied().unwrap_or(&current_blank);
            let current_score = location_text_overlap_score(&text, &card_text(current_location));
            let repeat_penalty = current.get(&current_name).copied().unwrap_or(0) + previous_count(&current_name);
            let score = (desired_score - current_score) + repeat_penalty;
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((scene, score));
            }
        }
        if let Some((scene, _)) = best {
            let old = balanced.get(&scene.id).unwrap_or("").to_string();
            if let Some(count) = current.get_mut(&old) {
                *count = (*count - 1).max(0);
            }
            balanced.insert(&scene.id, desired_name);
            *current.entry(desired_name.clone()).or_insert(0) += 1;
        }
    }
    balanced
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn place(name: &str, description: &str) -> Location {
        Location { name: name.into(), description: description.into() }
    }

    fn scene(id: &str, concept: &str) -> Scene {
        Scene { id: id.into(), label: format!("Scene {id}"), concept: concept.into(), notes: String::new() }
    }

    #[test]
    fn lines_with_numbers_become_cards() {
        let found = parse_location_lines("1|bedroom|small bedroom with a lamp and a bed\n2|razor|a shaving razor\n3|Foggy forest|pine trees in fog\n");
        let names: Vec<&str> = found.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["Bedroom", "Foggy forest"]);
        assert_eq!(found[0].description, "Bedroom, small bedroom with a lamp and a bed");
    }

    #[test]
    fn bullets_json_and_sentences_are_read() {
        let bullets = parse_location_ideas_flexible("Music Video Locations:\n\n- Abandoned motel pool, turquoise water under buzzing neon signs\n- Foggy pine road, wet asphalt and fading headlights\n");
        assert_eq!(bullets.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["Abandoned motel pool", "Foggy pine road"]);
        let json_reply = parse_location_ideas_flexible(r#"{"locations": [{"name": "dark wood hallway", "description": "dark wood hallway with a lamp"}, {"name": "Lace veil", "description": "a veil"}]}"#);
        assert_eq!(json_reply.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["Dark wood hallway"]);
        assert!(parse_location_ideas_flexible("I am sorry, I cannot do that.").is_empty());
    }

    #[test]
    fn subjects_come_from_pipes_or_colons() {
        let found = parse_subject_lines("1|young singer|a girl in a red coat\n- masked man: tall, black mask\nno colon here\n");
        assert_eq!(found, vec![Subject { name: "young singer".into(), description: "a girl in a red coat".into() }, Subject { name: "masked man".into(), description: "tall, black mask".into() }]);
    }

    #[test]
    fn scenes_are_mapped_by_number_and_every_location_is_used() {
        let locations = vec![place("Bedroom", "small bedroom"), place("Foggy forest", "pine trees in fog"), place("Empty stage", "stage with lights")];
        let scenes = vec![scene("a", "she wakes in the bedroom"), scene("b", "she runs through the forest"), scene("c", "she walks home")];
        let parsed = parse_scene_location_number_map("Scene1=1\nScene2=2\nScene3=2\n", &scenes, &locations);
        assert_eq!(parsed.iter().collect::<Vec<_>>(), vec![("a", "Bedroom"), ("b", "Foggy forest"), ("c", "Foggy forest")]);
        let balanced = balance_location_map_by_usage(parsed, &scenes, &locations, &location_usage_counts(None, None, &locations));
        assert_eq!(balanced.iter().map(|(_, n)| n).collect::<HashSet<_>>().len(), 3);
        let counts = location_usage_counts(Some(&json!({"bedroom": 2, "nowhere": 4})), Some(&json!(["Foggy forest", {"location": "Bedroom"}])), &locations);
        assert_eq!(counts, vec![("Bedroom".to_string(), 3), ("Foggy forest".to_string(), 1), ("Empty stage".to_string(), 0)]);
    }
}
