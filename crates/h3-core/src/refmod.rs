//! Which RefMods a scene uses, in what order, and the labels the text encoder gives them. Port of
//! `minimax/refmod_scene.py`.
//!
//! A RefMod reference is a Reference Builder card with `source == "refmod"` whose saved RefMod is named in
//! `card["refmod"]["name"]`. Text Encode with RefMods numbers visual mods per kind in loader order: `<Picture n>` for
//! single frame mods, `<Video n>` for mods with several frames and `<Audio n>` for audio. Mods at strength 0 take no number.

use serde_json::{json, Value};

use crate::pyval::{text_or_empty, to_float, to_int, truthy, Obj};
use crate::scene::{builder, forced_extra_keys, scene_map_value, split_ids, subject_items_for_segment};

pub const TOKEN_WARN_LIMIT: i64 = 8192;
const IMBALANCE_RATIO: f64 = 2.0;

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

fn rank(category: &str) -> i64 {
    match category {
        "character" => 0,
        "extra" => 1,
        "clothing" => 2,
        "object" => 3,
        "background" => 4,
        "style" => 5,
        _ => 3,
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "image" => "Picture",
        "audio" => "Audio",
        _ => "Video",
    }
}

pub fn is_refmod_card(card: &Value) -> bool {
    card.is_object() && text(card.get("source")) == "refmod" && card.get("refmod").is_some_and(Value::is_object) && !text(card["refmod"].get("name")).is_empty()
}

/// The ordering category of a card: character, extra, clothing, object, background or style.
pub fn card_category(card: &Value, kind: &str) -> &'static str {
    match kind {
        "extra" => return "extra",
        "location" => return "background",
        _ => {}
    }
    let reference_type = Some(text(card.get("reference_type"))).filter(|t| !t.is_empty()).unwrap_or_else(|| "character".into());
    match reference_type.as_str() {
        "character" | "creature" => "character",
        "outfit" => "clothing",
        "environment" => "background",
        "style" => "style",
        _ => "object",
    }
}

fn strength(card: &Value) -> f64 {
    match card["refmod"].get("strength") {
        None => 1.0,
        Some(v) => to_float(v).map_or(1.0, |s| s.clamp(0.0, 1.0)),
    }
}

fn item(card: &Value, kind: &str) -> Obj {
    let refmod = &card["refmod"];
    let mut out = Obj::new();
    out.insert("key".into(), json!(format!("{kind}:{}", text(card.get("id")))));
    out.insert("card_id".into(), json!(text(card.get("id"))));
    out.insert("name".into(), json!(Some(text(card.get("name"))).filter(|n| !n.is_empty()).unwrap_or_else(|| text(refmod.get("name")))));
    out.insert("category".into(), json!(card_category(card, kind)));
    out.insert("reference_type".into(), json!(Some(text(card.get("reference_type"))).filter(|t| !t.is_empty()).unwrap_or_else(|| if kind == "location" { "environment".into() } else { "character".into() })));
    out.insert("mod_name".into(), json!(text(refmod.get("name"))));
    out.insert("kind".into(), json!(Some(text(refmod.get("kind"))).filter(|k| !k.is_empty()).unwrap_or_else(|| "video".into())));
    out.insert("strength".into(), json!(strength(card)));
    out.insert("tokens".into(), json!(refmod.get("tokens").filter(|v| truthy(v)).and_then(to_int).unwrap_or(0)));
    out.insert("description".into(), json!(text(card.get("description"))));
    out.insert("wears".into(), json!(text(card.get("wears"))));
    out
}

fn str_of<'a>(item: &'a Obj, key: &str) -> &'a str {
    item.get(key).and_then(Value::as_str).unwrap_or("")
}

fn strength_of(item: &Obj) -> f64 {
    item.get("strength").and_then(Value::as_f64).unwrap_or(0.0)
}

fn tokens_of(item: &Obj) -> i64 {
    item.get("tokens").and_then(Value::as_i64).unwrap_or(0)
}

/// The subject cards a scene sends: none for a "no character present" scene.
pub fn scene_subject_cards(segment: &Obj, subject_cards: Vec<Value>) -> Vec<Value> {
    if segment.get("no_character_present").is_some_and(truthy) { vec![] } else { subject_cards }
}

/// Turn the cards a scene uses into ordered RefMod items. `override_` is `{character id: clothing card id(s)}`.
pub fn compose_items(subject_cards: &[Value], extra_cards: &[Value], location_card: Option<&Value>, all_subjects: &[Value], override_: Option<&Obj>) -> Vec<Obj> {
    let empty = Obj::new();
    let override_ = override_.unwrap_or(&empty);
    let mut items: Vec<Obj> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut add = |items: &mut Vec<Obj>, card: &Value, kind: &str| {
        if !is_refmod_card(card) {
            return;
        }
        let key = format!("{kind}:{}", text(card.get("id")));
        if seen.contains(&key) {
            return;
        }
        seen.push(key);
        items.push(item(card, kind));
    };
    for card in subject_cards {
        add(&mut items, card, "subject");
    }
    for card in extra_cards {
        add(&mut items, card, "extra");
    }
    let characters: Vec<String> = items.iter().filter(|i| str_of(i, "category") == "character").map(|i| str_of(i, "card_id").to_string()).collect();
    for character_id in characters {
        if let Some(chosen) = override_.get(&character_id) {
            let chosen: Vec<String> = split_ids(Some(chosen));
            items.retain(|i| !(str_of(i, "category") == "clothing" && str_of(i, "wears") == character_id && !chosen.iter().any(|c| c == str_of(i, "card_id"))));
            for card in all_subjects.iter().filter(|c| c.is_object()) {
                if chosen.contains(&text(card.get("id"))) {
                    add(&mut items, card, "subject");
                }
            }
        }
    }
    if let Some(location) = location_card {
        add(&mut items, location, "location");
    }
    order_items(items)
}

/// Stable order: category rank, then for clothing the position of the character who wears it.
pub fn order_items(items: Vec<Obj>) -> Vec<Obj> {
    let characters: Vec<String> = items.iter().filter(|i| str_of(i, "category") == "character").map(|i| str_of(i, "card_id").to_string()).collect();
    let key = |position: usize, i: &Obj| -> (i64, usize, usize) {
        let clothing = str_of(i, "category") == "clothing";
        let wearer = if clothing { characters.iter().position(|c| c == str_of(i, "wears")).unwrap_or(characters.len()) } else { 0 };
        (rank(str_of(i, "category")), wearer, position)
    };
    let mut indexed: Vec<(usize, Obj)> = items.into_iter().enumerate().collect();
    indexed.sort_by_key(|(position, i)| key(*position, i));
    indexed.into_iter().map(|(_, i)| i).collect()
}

/// Who wears or uses each piece of clothing or prop in one scene. `users` is `segment["scene_item_users"]`.
pub fn apply_scene_item_users(items: Vec<Obj>, users: Option<&Value>) -> Vec<Obj> {
    let empty = Obj::new();
    let chosen = users.and_then(Value::as_object).unwrap_or(&empty);
    let characters: Vec<String> = items.iter().filter(|i| str_of(i, "category") == "character").map(|i| str_of(i, "card_id").to_string()).collect();
    let changed = items
        .into_iter()
        .map(|mut i| {
            let user = text(chosen.get(str_of(&i, "card_id")));
            if matches!(str_of(&i, "category"), "clothing" | "object") && characters.contains(&user) {
                i.insert("wears".into(), json!(user));
            }
            i
        })
        .collect();
    order_items(changed)
}

/// The scene's RefMod references in render order, with no labels yet.
pub fn refmod_items_for_scene(session: &Value, segment: &Obj, index: usize) -> Vec<Obj> {
    let refs = builder(session);
    let objects = |key: &str| -> Vec<Value> { refs.get(key).and_then(Value::as_array).map(|a| a.iter().filter(|v| v.is_object()).cloned().collect()).unwrap_or_default() };
    let subjects = objects("subjects");
    let extras = objects("extra_subjects");
    let extra_cards: Vec<Value> = forced_extra_keys(&refs, segment, index)
        .iter()
        .filter_map(|key| key.split_once(':').map(|(_, id)| id.to_string()))
        .filter_map(|id| extras.iter().find(|e| text(e.get("id")) == id).cloned())
        .collect();
    let location_id = scene_map_value(refs.get("scene_map"), segment, index).map(|v| text(Some(&v))).unwrap_or_default();
    let location = if location_id.is_empty() { None } else { objects("locations").into_iter().find(|l| text(l.get("id")) == location_id) };
    let subject_cards: Vec<Value> = subject_items_for_segment(&refs, segment, index).into_iter().map(Value::Object).collect();
    let items = compose_items(&scene_subject_cards(segment, subject_cards), &extra_cards, location.as_ref(), &subjects, None);
    apply_scene_item_users(items, segment.get("scene_item_users"))
}

/// Add `label` (for example `<Video 2>`) to each item the way Text Encode with RefMods numbers them. With
/// `include_audio` an `<Audio 1>` entry follows.
pub fn assign_labels(items: &[Obj], include_audio: bool) -> Vec<Obj> {
    let (mut image, mut video, mut audio) = (0, 0, 0);
    let mut labelled: Vec<Obj> = Vec::new();
    for source in items {
        let mut entry = source.clone();
        let kind = if matches!(str_of(source, "kind"), "image" | "video") { str_of(source, "kind").to_string() } else { "video".into() };
        let label = if strength_of(source) <= 0.0 {
            String::new()
        } else {
            let n = if kind == "image" { &mut image } else { &mut video };
            *n += 1;
            format!("<{} {}>", kind_label(&kind), n)
        };
        entry.insert("label".into(), json!(label));
        labelled.push(entry);
    }
    if include_audio {
        audio += 1;
        let mut entry = Obj::new();
        for (k, v) in [("key", json!("audio:scene")), ("card_id", json!("")), ("name", json!("Scene audio")), ("category", json!("audio")), ("reference_type", json!("audio")), ("mod_name", json!("")), ("kind", json!("audio")), ("strength", json!(1.0)), ("tokens", json!(0)), ("description", json!("")), ("wears", json!("")), ("label", json!(format!("<Audio {audio}>")))] {
            entry.insert(k.into(), v);
        }
        labelled.push(entry);
    }
    labelled
}

pub fn total_tokens(items: &[Obj]) -> i64 {
    items.iter().filter(|i| strength_of(i) > 0.0).map(tokens_of).sum()
}

/// The scene's token total and the character balance. A person whose weight (tokens times strength) is under half of the
/// strongest person's tends to be duplicated or replaced, so the weakest and strongest are named past the ratio.
pub fn token_report(items: &[Obj]) -> Value {
    let active: Vec<&Obj> = items.iter().filter(|i| strength_of(i) > 0.0).collect();
    let total: i64 = active.iter().map(|i| tokens_of(i)).sum();
    let people: Vec<(f64, &Obj)> = active.iter().filter(|i| matches!(str_of(i, "category"), "character" | "extra") && tokens_of(i) > 0).map(|i| (tokens_of(i) as f64 * strength_of(i), *i)).collect();
    let mut imbalance = Value::Null;
    if people.len() >= 2 {
        let weak = people.iter().fold(people[0], |a, b| if b.0 < a.0 { *b } else { a });
        let strong = people.iter().fold(people[0], |a, b| if b.0 > a.0 { *b } else { a });
        if weak.0 > 0.0 && strong.0 / weak.0 > IMBALANCE_RATIO {
            imbalance = json!({"weak": str_of(weak.1, "name"), "strong": str_of(strong.1, "name"), "ratio": crate::pyval::round_to(strong.0 / weak.0, 1)});
        }
    }
    json!({"total": total, "limit": TOKEN_WARN_LIMIT, "over_limit": total > TOKEN_WARN_LIMIT, "imbalance": imbalance})
}

/// The `refmod_references` list a render carries: visual items only, already ordered.
pub fn reference_payload(items: &[Obj]) -> Vec<Value> {
    items
        .iter()
        .filter(|i| !str_of(i, "mod_name").is_empty())
        .map(|i| json!({"name": str_of(i, "mod_name"), "strength": strength_of(i), "kind": str_of(i, "kind"), "label": str_of(i, "label"), "card_id": str_of(i, "card_id"), "display_name": str_of(i, "name"), "category": str_of(i, "category")}))
        .collect()
}
