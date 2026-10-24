//! Story Mode location cards: their names, and merging them into the Reference Builder's data. Port of `card_name` and
//! `apply_story_locations` in `storyboard/story_locations.py`.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::pyval::{text_or_empty, truthy, Obj};

/// `Home - Kitchen`. Just the setting when the spot is the setting itself.
pub fn card_name(setting_name: &str, spot: &str) -> String {
    let (setting, spot) = (setting_name.trim(), spot.trim());
    if spot.is_empty() || spot.to_lowercase() == setting.to_lowercase() {
        return if setting.is_empty() { spot.to_string() } else { setting.to_string() };
    }
    if setting.is_empty() { spot.to_string() } else { format!("{setting} - {spot}") }
}

/// How many cards a merge added and how many descriptions it replaced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MergeCounts {
    pub added: i64,
    pub updated: i64,
}

fn key_of(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_lowercase()
}

/// Reference Builder data with the story's location cards merged in and every planned scene mapped. A card with the same name is reused. Its
/// description is only replaced when it is empty (or with `replace_descriptions`) and its image is never touched. `new_id(taken)` makes an id
/// for a new card.
pub fn apply_story_locations(refs: &Value, locations: &[Value], new_id: &mut dyn FnMut(&mut HashSet<String>) -> String, replace_descriptions: bool) -> (Value, MergeCounts) {
    let mut result: Obj = refs.as_object().cloned().unwrap_or_default();
    let mut cards: Vec<Value> = match result.get("locations") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    // A dict by lowercased name: a repeated name takes the later card.
    let mut by_name: Vec<(String, usize)> = Vec::new();
    for (index, card) in cards.iter().enumerate() {
        if card.is_object() {
            let key = key_of(card.get("name"));
            match by_name.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 = index,
                None => by_name.push((key, index)),
            }
        }
    }
    let mut taken: HashSet<String> = cards.iter().filter(|c| c.is_object()).map(|c| text_or_empty(c.get("id"))).collect();
    let mut scene_map: Obj = result.get("scene_map").and_then(Value::as_object).cloned().unwrap_or_default();
    let (mut added, mut updated) = (0, 0);
    for item in locations {
        let name = text_or_empty(item.get("name").filter(|v| truthy(v))).trim().to_string();
        if name.is_empty() {
            continue;
        }
        let key = name.to_lowercase();
        let found = by_name.iter().find(|(k, _)| *k == key).map(|(_, i)| *i);
        let index = match found {
            None => {
                let id = new_id(&mut taken);
                let description = item.get("description").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!(""));
                cards.push(json!({"id": id, "name": name, "description": description, "image": {"path": "", "data": "", "name": ""}}));
                by_name.push((key, cards.len() - 1));
                added += 1;
                cards.len() - 1
            }
            Some(index) => {
                let replace = item.get("description").is_some_and(truthy) && (replace_descriptions || text_or_empty(cards[index].get("description")).trim().is_empty());
                if replace {
                    if let Some(card) = cards[index].as_object_mut() {
                        card.insert("description".into(), item.get("description").cloned().unwrap_or(Value::Null));
                    }
                    updated += 1;
                }
                index
            }
        };
        let Some(card) = cards[index].as_object_mut() else { continue };
        card.insert("story_setting_id".into(), item.get("story_setting_id").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("")));
        card.insert("story_act_ids".into(), match item.get("story_act_ids") {
            Some(Value::Array(ids)) => Value::Array(ids.clone()),
            _ => json!([]),
        });
        let card_id = card.get("id").cloned().unwrap_or(Value::Null);
        if let Some(Value::Array(ids)) = item.get("scene_ids") {
            for scene_id in ids {
                scene_map.insert(text_or_empty(Some(scene_id)), card_id.clone());
            }
        }
    }
    result.insert("use_location_references".into(), json!(!cards.is_empty()));
    result.insert("locations".into(), Value::Array(cards));
    result.insert("scene_map".into(), Value::Object(scene_map));
    result.insert("locations_cleared".into(), json!(false));
    (Value::Object(result), MergeCounts { added, updated })
}
