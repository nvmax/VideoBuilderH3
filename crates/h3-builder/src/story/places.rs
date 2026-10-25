//! Story Mode locations: a look for each chapter setting and one location card per spot, mapped to its scenes. Port of
//! `storyboard/story_locations.py` (`build_story_locations`).
//!
//! Built only from the story (book, settings, scene plan) and the style/theme notes. The lyrics are never sent.

use std::collections::HashMap;

use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_core::story::helpers::first_truthy;
use h3_core::story::locations::card_name;
use h3_core::story::mode::{normalize_story_book, normalize_story_settings, setting_id, story_book_to_text, sync_story_settings, text};
use h3_llm::locations::extract_json_object;
use h3_llm::prompts::constant;
use h3_llm::story_prompts::{story_place_list_instruction, story_setting_looks_instruction, story_spots_instruction};
use serde_json::{json, Value};

use super::book::run;
use super::context::{cast_lines, layer as request_layer, match_user_place, scenes as request_scenes, story_template, user_places, user_places_text};
use crate::prompt_writers::Llm;

type Reply = Result<Value, String>;

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

/// A place inside a setting that gets a location card.
#[derive(Debug, Clone, Default)]
pub struct Spot {
    pub name: String,
    pub card_name: String,
    pub scene_ids: Vec<String>,
    pub act_ids: Vec<String>,
    pub slices: Vec<String>,
}

/// A chapter setting with its look and its spots.
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub setting_id: String,
    pub setting_name: String,
    pub look: String,
    pub spots: Vec<Spot>,
    pub text: Vec<String>,
}

/// `spot_groups`: settings in story order, each with its spots in first-use order: names, scenes, acts and story slices.
pub fn spot_groups(layer: &Value, scene_ids: &[String]) -> Vec<Group> {
    let chapters = array(normalize_story_book(layer.get("story_book")).get("chapters"));
    let settings: HashMap<String, Value> = normalize_story_settings(layer.get("story_settings")).into_iter().map(|i| (s(i.get("id")), i)).collect();
    let chapter_setting: HashMap<String, String> = chapters.iter().map(|c| (s(c.get("id")), s(c.get("setting_id")))).collect();
    let act_chapter: HashMap<String, String> = array(layer.get("story_acts")).iter().map(|a| (s(a.get("id")), s(a.get("chapter_id")))).collect();
    let plan: HashMap<String, Value> = array(layer.get("story_scene_plan")).into_iter().map(|e| (s(e.get("scene_id")), e)).collect();
    let mut groups: Vec<Group> = Vec::new();
    for scene_id in scene_ids {
        let Some(entry) = plan.get(scene_id).filter(|e| truthy(e)) else { continue };
        let setting_key = act_chapter.get(&s(entry.get("act_id"))).and_then(|chapter| chapter_setting.get(chapter)).cloned().unwrap_or_default();
        let setting = settings.get(&setting_key).cloned().unwrap_or_else(|| json!({"id": setting_key, "name": "", "look": ""}));
        let position = match groups.iter().position(|g| g.setting_id == setting_key) {
            Some(position) => position,
            None => {
                groups.push(Group { setting_id: setting_key.clone(), setting_name: s(setting.get("name")), look: s(setting.get("look")), ..Group::default() });
                groups.len() - 1
            }
        };
        let name = card_name(&s(setting.get("name")), &s(entry.get("spot")));
        let group = &mut groups[position];
        let spot_position = match group.spots.iter().position(|spot| spot.card_name.to_lowercase() == name.to_lowercase()) {
            Some(position) => position,
            None => {
                let given = entry.get("spot").filter(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| s(setting.get("name")));
                group.spots.push(Spot { name: given.trim().to_string(), card_name: name, ..Spot::default() });
                group.spots.len() - 1
            }
        };
        let spot = &mut group.spots[spot_position];
        spot.scene_ids.push(scene_id.clone());
        let act_id = s(entry.get("act_id"));
        if !act_id.is_empty() && !spot.act_ids.contains(&act_id) {
            spot.act_ids.push(act_id);
        }
        if entry.get("story_slice").is_some_and(truthy) && spot.slices.len() < 6 {
            spot.slices.push(s(entry.get("story_slice")));
        }
    }
    groups
}

/// `_parse`: the objects under `key` in a JSON reply. Nothing when the reply is not usable.
fn parse_items(raw: &str, key: &str) -> Vec<Value> {
    match extract_json_object(raw) {
        Ok(Value::Object(map)) => array(map.get(key)).into_iter().filter(Value::is_object).collect(),
        _ => Vec::new(),
    }
}

/// A location card the story asks for.
#[derive(Debug, Clone, Default)]
struct Card {
    name: String,
    description: String,
    story_setting_id: String,
    story_act_ids: Vec<String>,
    scene_ids: Vec<String>,
}

impl Card {
    fn to_value(&self) -> Value {
        json!({"name": self.name, "description": self.description, "story_setting_id": self.story_setting_id, "story_act_ids": self.story_act_ids, "scene_ids": self.scene_ids})
    }
}

fn cards_value(cards: &[Card]) -> Value {
    Value::Array(cards.iter().map(Card::to_value).collect())
}

fn settings_layer(settings: Vec<Value>) -> Obj {
    let mut map = Obj::new();
    map.insert("story_settings".into(), Value::Array(settings));
    map
}

fn blank_reply(layer: Obj, locations: &[Card]) -> Value {
    json!({"story_layer": Value::Object(layer), "locations": cards_value(locations), "runner": "", "used_model": ""})
}

/// `_user_place_cards`: cards for the user's own places before any scene is planned. Their text word for word, no scene mapping. A chapter
/// setting that is one of these places also takes the user's details as its look.
fn user_place_cards(own_places: &[(String, String)], settings: &[Value]) -> Value {
    let looks: Vec<Value> = settings
        .iter()
        .map(|item| {
            let name = s(item.get("name"));
            let place = if name.is_empty() { None } else { match_user_place(own_places, &[&name]) };
            let mut map = item.as_object().cloned().unwrap_or_default();
            let take_user = place.as_ref().is_some_and(|(_, details)| !details.is_empty()) && !item.get("look").is_some_and(truthy);
            if take_user {
                map.insert("look".into(), json!(place.map(|(_, d)| d).unwrap_or_default()));
            }
            Value::Object(map)
        })
        .collect();
    let mut locations = Vec::new();
    for (name, details) in own_places {
        let setting = settings.iter().find(|item| match_user_place(&[(name.clone(), details.clone())], &[&s(item.get("name"))]).is_some());
        locations.push(Card { name: name.clone(), description: details.clone(), story_setting_id: setting.map(|i| s(i.get("id"))).unwrap_or_default(), ..Card::default() });
    }
    blank_reply(settings_layer(looks), &locations)
}

/// `_book_groups`: chapter settings in story order, each with the story text of its chapters, for the Places step before any scene is planned.
fn book_groups(layer: &Value, settings: &[Value]) -> Vec<Group> {
    let by_id: HashMap<String, &Value> = settings.iter().map(|i| (s(i.get("id")), i)).collect();
    let mut groups: Vec<Group> = Vec::new();
    for chapter in array(normalize_story_book(layer.get("story_book")).get("chapters")) {
        let Some(setting) = by_id.get(&s(chapter.get("setting_id"))) else { continue };
        if s(setting.get("name")).is_empty() {
            continue;
        }
        let id = s(setting.get("id"));
        let position = match groups.iter().position(|g| g.setting_id == id) {
            Some(p) => p,
            None => {
                groups.push(Group { setting_id: id, setting_name: s(setting.get("name")), look: s(setting.get("look")), ..Group::default() });
                groups.len() - 1
            }
        };
        groups[position].text.push(array(chapter.get("paragraphs")).iter().map(|p| s(Some(p))).collect::<Vec<_>>().join(" "));
    }
    groups
}

fn truncated(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// `_find_book_spots`: fill each group's spots with the places the story uses inside its setting. Falls back to the setting itself when the
/// LLM fails. Returns the runner info.
async fn find_book_spots(llm: &dyn Llm, payload: &Obj, groups: &mut [Group], style_theme: &str) -> Obj {
    let settings_text = groups.iter().map(|g| format!("- {}: {}", g.setting_name, truncated(&g.text.join(" "), 1500))).collect::<Vec<_>>().join("\n");
    let mut info = Obj::new();
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    let instruction = story_place_list_instruction(&truncated(&settings_text, 12000), &user_places_text(payload), style_theme);
    match run(llm, payload, instruction, "Story Mode Place List", 1200, 0.4, schema("STORY_PLACE_LIST_SCHEMA")).await {
        Ok((raw, run_info)) => {
            info = run_info;
            for item in parse_items(&raw, "settings") {
                let names: Vec<String> = array(item.get("spots")).iter().map(|n| text(Some(n), 80)).filter(|n| !n.is_empty()).take(6).collect();
                let key = s(item.get("name").filter(|v| truthy(v))).trim().to_lowercase();
                match found.iter_mut().find(|(k, _)| *k == key) {
                    Some(entry) => entry.1 = names,
                    None => found.push((key, names)),
                }
            }
        }
        Err(error) => eprintln!("[VRGDG-RS Story Mode] The place list failed, each setting is one place: {error}"),
    }
    for group in groups.iter_mut() {
        let story = truncated(&group.text.join(" "), 900);
        let listed = found.iter().find(|(k, _)| *k == group.setting_name.to_lowercase()).map(|(_, names)| names.clone()).filter(|names| !names.is_empty());
        let names = listed.unwrap_or_else(|| vec![group.setting_name.clone()]);
        let mut seen: Vec<String> = Vec::new();
        for name in names {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let card = card_name(&group.setting_name, &name);
            if !group.spots.iter().any(|spot| spot.card_name.to_lowercase() == card.to_lowercase()) {
                group.spots.push(Spot { name, card_name: card, slices: vec![story.clone()], ..Spot::default() });
            }
        }
    }
    info
}

/// `_name_unassigned_chapters`: story layer fields that give every chapter without a setting one named after its title. The LLM sometimes
/// writes the story without `Setting:` lines. The places are still found inside each chapter, so the chapter title stands in as its setting.
fn name_unassigned_chapters(layer: &Value) -> Obj {
    let mut book = normalize_story_book(layer.get("story_book"));
    let mut names = Obj::new();
    let mut changed = false;
    if let Some(chapters) = book.get_mut("chapters").and_then(Value::as_array_mut) {
        for chapter in chapters.iter_mut() {
            let (has_setting, title) = (chapter.get("setting_id").is_some_and(truthy), s(chapter.get("title")));
            if has_setting || title.is_empty() {
                continue;
            }
            let id = setting_id(&title);
            chapter["setting_id"] = json!(id);
            if !names.contains_key(&id) {
                names.insert(id, json!(title));
            }
            changed = true;
        }
    }
    if !changed {
        return Obj::new();
    }
    let settings = Value::Array(sync_story_settings(&book, &names, layer.get("story_settings")));
    let mut fields = Obj::new();
    fields.insert("story_book_text".into(), json!(story_book_to_text(&book, Some(&settings))));
    fields.insert("story_book".into(), book);
    fields.insert("story_settings".into(), settings);
    // The pack lists the book first.
    let mut ordered = Obj::new();
    for key in ["story_book", "story_book_text", "story_settings"] {
        if let Some(v) = fields.get(key) {
            ordered.insert(key.into(), v.clone());
        }
    }
    ordered
}

/// `_map_scenes_to_made_cards`: maps every planned scene to a location card that already exists. No card, look or description is made. A spot
/// with no card of its own takes the first card of its setting. A card keeps the story link it already has (the apply step writes
/// `story_setting_id` back), and a setting whose cards lost that link still maps to the unlinked cards, so a planned scene is not left unmapped.
/// A chapter with no setting at all maps to any card.
fn map_scenes_to_made_cards(payload: &Obj, groups: &[Group], settings: Vec<Value>) -> Value {
    let refs = first_truthy(payload, &["reference_builder", "referenceBuilder"]).cloned().unwrap_or_else(|| json!({}));
    let cards: Vec<Value> = array(refs.get("locations")).into_iter().filter(Value::is_object).collect();
    let linked_to = |card: &Value| s(card.get("story_setting_id").filter(|v| truthy(v)));
    let named: Vec<(String, &Value)> = cards
        .iter()
        .filter_map(|card| {
            let name = s(card.get("name")).trim().to_string();
            (!name.is_empty()).then_some((name, card))
        })
        .collect();
    // A dict by lowercased name: a repeated name takes the later card.
    let card_by_name: HashMap<String, &Value> = named.iter().map(|(name, card)| (name.to_lowercase(), *card)).collect();
    let mut locations: Vec<Card> = Vec::new();
    for group in groups {
        let linked: Vec<(String, &Value)> = named.iter().filter(|(_, card)| linked_to(card) == group.setting_id).cloned().collect();
        let unlinked: Vec<(String, &Value)> = named.iter().filter(|(_, card)| linked_to(card).is_empty()).cloned().collect();
        let made = if group.setting_id.is_empty() {
            named.clone()
        } else if linked.is_empty() {
            unlinked
        } else {
            linked
        };
        for spot in &group.spots {
            let mut name = made.iter().find(|(n, _)| n.to_lowercase() == spot.card_name.to_lowercase()).map(|(n, _)| n.clone()).unwrap_or_default();
            if name.is_empty() {
                let places: Vec<(String, String)> = made.iter().map(|(n, _)| (n.clone(), n.clone())).collect();
                name = match match_user_place(&places, &[&spot.name, &spot.card_name]) {
                    Some((_, details)) => details,
                    None => made.first().map(|(n, _)| n.clone()).unwrap_or_default(),
                };
            }
            if name.is_empty() {
                continue;
            }
            let position = match locations.iter().position(|c| c.name.to_lowercase() == name.to_lowercase()) {
                Some(p) => p,
                None => {
                    let own_setting = card_by_name.get(&name.to_lowercase()).map(|card| linked_to(card)).unwrap_or_default();
                    let story_setting_id = if group.setting_id.is_empty() { own_setting } else { group.setting_id.clone() };
                    locations.push(Card { name: name.clone(), story_setting_id, ..Card::default() });
                    locations.len() - 1
                }
            };
            let existing = &mut locations[position];
            for act in &spot.act_ids {
                if !existing.story_act_ids.contains(act) {
                    existing.story_act_ids.push(act.clone());
                }
            }
            existing.scene_ids.extend(spot.scene_ids.iter().cloned());
        }
    }
    blank_reply(settings_layer(settings), &locations)
}

fn scene_ids_of(payload: &Obj) -> Vec<String> {
    request_scenes(payload).iter().enumerate().map(|(i, scene)| match scene.get("id").filter(|v| truthy(v)) {
        Some(v) => text_or_empty(Some(v)),
        None => format!("scene_{}", i + 1),
    }).collect()
}

/// `build_story_locations`: looks for settings without one (all with `regenerate_looks`) and a card for every spot in the scene plan. With
/// `map_only` and a scene plan, nothing is made: each scene is only mapped to a card that already exists.
pub async fn build_story_locations(llm: &dyn Llm, payload: &Obj) -> Reply {
    let mut layer = request_layer(payload);
    let scene_ids = scene_ids_of(payload);
    let mut groups = spot_groups(&layer, &scene_ids);
    let mut settings = normalize_story_settings(layer.get("story_settings"));
    let own_places = user_places(payload);
    if payload.get("map_only").is_some_and(truthy) && !groups.is_empty() {
        return Ok(map_scenes_to_made_cards(payload, &groups, settings));
    }
    let style_theme = text(payload.get("style_theme"), 3000);
    // Before any scene is planned the places come from the story itself: the places inside each chapter setting, found by the LLM, plus the
    // user's own list. The acts and the scene plan then work inside these cards.
    let from_book = groups.is_empty();
    let mut info = Obj::new();
    let mut named = Obj::new();
    if from_book {
        groups = book_groups(&layer, &settings);
        if groups.is_empty() {
            named = name_unassigned_chapters(&layer);
            if !named.is_empty() {
                if let Some(map) = layer.as_object_mut() {
                    for (key, value) in &named {
                        map.insert(key.clone(), value.clone());
                    }
                }
                settings = array(named.get("story_settings"));
                groups = book_groups(&layer, &settings);
            }
        }
        if groups.is_empty() {
            if !own_places.is_empty() {
                return Ok(user_place_cards(&own_places, &settings));
            }
            return Err("Write the story first, or list your locations in the Story tab; locations are built from them.".into());
        }
        info = find_book_spots(llm, payload, &mut groups, &style_theme).await;
    }
    // A setting that is one of the user's places takes the user's own details as its look, word for word.
    let regenerate = payload.get("regenerate_looks").is_some_and(truthy);
    let mut from_user: Vec<String> = Vec::new();
    for group in groups.iter_mut() {
        let place = if group.setting_name.is_empty() { None } else { match_user_place(&own_places, &[&group.setting_name]) };
        if let Some((_, details)) = place {
            if !details.is_empty() && (regenerate || group.look.is_empty()) {
                group.look = details;
                from_user.push(group.setting_id.clone());
            }
        }
    }
    let need_looks: Vec<usize> = groups.iter().enumerate().filter(|(_, g)| !g.setting_name.is_empty() && !from_user.contains(&g.setting_id) && (regenerate || g.look.is_empty())).map(|(i, _)| i).collect();
    if !need_looks.is_empty() {
        let book = normalize_story_book(layer.get("story_book"));
        let summary = array(book.get("chapters"))
            .iter()
            .enumerate()
            .map(|(i, chapter)| format!("Chapter {} ({}): {}", i + 1, s(chapter.get("title")), truncated(&array(chapter.get("paragraphs")).iter().map(|p| s(Some(p))).collect::<Vec<_>>().join(" "), 600)))
            .collect::<Vec<_>>()
            .join("\n");
        let instruction = story_setting_looks_instruction(
            &need_looks.iter().map(|i| format!("- {}", groups[*i].setting_name)).collect::<Vec<_>>().join("\n"),
            &truncated(&summary, 12000),
            &style_theme,
            &cast_lines(payload),
            Some(&story_template(payload, "story_mode_setting_looks")),
            &user_places_text(payload),
        );
        let mut looks: Vec<(String, String)> = Vec::new();
        match run(llm, payload, instruction, "Story Mode Setting Looks", 1600, 0.5, schema("STORY_SETTING_LOOKS_SCHEMA")).await {
            Ok((raw, run_info)) => {
                info = run_info;
                for item in parse_items(&raw, "settings") {
                    let key = s(item.get("name").filter(|v| truthy(v))).trim().to_lowercase();
                    let look = text(item.get("look"), 2000);
                    match looks.iter_mut().find(|(k, _)| *k == key) {
                        Some(entry) => entry.1 = look,
                        None => looks.push((key, look)),
                    }
                }
            }
            Err(error) => eprintln!("[VRGDG-RS Story Mode] Setting looks failed, spots are written without them: {error}"),
        }
        for index in &need_looks {
            let group = &mut groups[*index];
            if let Some((_, look)) = looks.iter().find(|(k, _)| *k == group.setting_name.to_lowercase()) {
                if !look.is_empty() {
                    group.look = look.clone();
                }
            }
        }
    }
    if !need_looks.is_empty() || !from_user.is_empty() {
        settings = settings
            .iter()
            .map(|item| {
                let id = s(item.get("id"));
                let found = groups.iter().find(|g| g.setting_id == id && !g.look.is_empty()).map(|g| g.look.clone()).unwrap_or_else(|| s(item.get("look")));
                let mut map = item.as_object().cloned().unwrap_or_default();
                map.insert("look".into(), json!(found));
                Value::Object(map)
            })
            .collect();
    }
    let mut locations: Vec<Card> = Vec::new();
    for group in &groups {
        // A spot that is one of the user's places takes the user's name and details word for word. The LLM writes the rest. One card per
        // place, so the card made from the list before the plan is the one scenes use.
        let mut matched: HashMap<String, (String, String)> = HashMap::new();
        for spot in &group.spots {
            if let Some(place) = match_user_place(&own_places, &[&spot.name, &spot.card_name]) {
                matched.insert(spot.card_name.clone(), place);
            }
        }
        let open_spots: Vec<&Spot> = group.spots.iter().filter(|spot| matched.get(&spot.card_name).is_none_or(|(_, details)| details.is_empty())).collect();
        let spots_text = open_spots
            .iter()
            .map(|spot| format!("- {}: {}", spot.name, if spot.slices.is_empty() { "[no story yet]".to_string() } else { spot.slices.join(" ") }))
            .collect::<Vec<_>>()
            .join("\n");
        let mut descriptions: Vec<(String, String)> = Vec::new();
        if !open_spots.is_empty() {
            let instruction = story_spots_instruction(&group.setting_name, &group.look, &truncated(&spots_text, 8000), &style_theme, Some(&story_template(payload, "story_mode_spots")));
            match run(llm, payload, instruction, "Story Mode Spots", 2000, 0.5, schema("STORY_SPOTS_SCHEMA")).await {
                Ok((raw, run_info)) => {
                    info = run_info;
                    for item in parse_items(&raw, "spots") {
                        let key = s(item.get("name").filter(|v| truthy(v))).trim().to_lowercase();
                        let description = text(item.get("description"), 800);
                        match descriptions.iter_mut().find(|(k, _)| *k == key) {
                            Some(entry) => entry.1 = description,
                            None => descriptions.push((key, description)),
                        }
                    }
                }
                Err(error) => eprintln!("[VRGDG-RS Story Mode] Spot descriptions for {} failed: {error}", group.setting_name),
            }
        }
        let described = |key: &str| descriptions.iter().find(|(k, _)| k == key).map(|(_, d)| d.clone()).filter(|d| !d.is_empty());
        for spot in &group.spots {
            let place = matched.get(&spot.card_name);
            let description = place.map(|(_, d)| d.clone()).filter(|d| !d.is_empty()).or_else(|| described(&spot.name.to_lowercase())).or_else(|| described(&spot.card_name.to_lowercase())).unwrap_or_default();
            let name = place.map(|(n, _)| n.clone()).unwrap_or_else(|| spot.card_name.clone());
            if let Some(existing) = locations.iter_mut().find(|item| item.name.to_lowercase() == name.to_lowercase()) {
                for act in &spot.act_ids {
                    if !existing.story_act_ids.contains(act) {
                        existing.story_act_ids.push(act.clone());
                    }
                }
                existing.scene_ids.extend(spot.scene_ids.iter().cloned());
                continue;
            }
            locations.push(Card {
                name,
                description: if description.is_empty() { group.look.clone() } else { description },
                story_setting_id: group.setting_id.clone(),
                story_act_ids: spot.act_ids.clone(),
                scene_ids: spot.scene_ids.clone(),
            });
        }
    }
    if from_book {
        // Places the user listed that no setting's place list took still get their card.
        for (name, details) in &own_places {
            if !locations.iter().any(|item| item.name.to_lowercase() == name.to_lowercase()) {
                locations.push(Card { name: name.clone(), description: details.clone(), ..Card::default() });
            }
        }
    }
    let mut story_layer = named;
    story_layer.insert("story_settings".into(), Value::Array(settings));
    Ok(json!({
        "story_layer": Value::Object(story_layer),
        "locations": cards_value(&locations),
        "runner": info.get("runner").cloned().unwrap_or_else(|| json!("")),
        "used_model": info.get("used_model").cloned().unwrap_or_else(|| json!("")),
    }))
}
