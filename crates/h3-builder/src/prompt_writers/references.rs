//! The Reference Builder's LLM buttons: describing a picture, extracting subjects and locations from the scenes, mapping scenes to
//! locations, and writing the prompt of a reference sheet. Ports of the `_generate_flux_reference_*`, `_generate_lm_scout_locations`,
//! `_generate_wizard_locations_from_lyrics` and `_generate_builder_reference_description` writers in `llm/image_prompt_generation.py`.

use std::collections::HashMap;
use std::path::Path;

use fancy_regex::Regex;
use h3_core::pyval::{round_half_even, text_or_empty, to_float, truthy, Obj};
use h3_llm::checks::clean_visual_text;
use h3_llm::describe;
use h3_llm::location_cards::{
    balance_location_map_by_usage, clean_location_card, fallback_location_map_by_overlap, location_usage_counts, normalize_location_name, parse_location_ideas_flexible, parse_location_lines, parse_scene_location_number_map,
    parse_subject_lines, valid_location_card, Scene,
};
use h3_llm::locations::{clean_location_context_text, extract_json_object, parse_scout_reply, preview, scout_instruction, scout_schema, Location, ScoutInput};
use h3_llm::prompt_text::validate_builder_prompt;
use h3_llm::runner::output_token_limit;
use h3_project::layout::scene_notes_path;
use serde_json::{json, Value};

use super::image_prompts::{is_external, vision_runner};
use super::images::image_from_payload;
use super::{number, text, whole, Llm, TextCall, VisionCall};

type Reply = Result<Value, String>;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn collapse(text: &str) -> String {
    rx(r"\s+").replace_all(text, " ").into_owned()
}

/// `str(payload.get(a) or payload.get(b) or "").strip()`.
fn or_text(payload: &Obj, keys: &[&str]) -> String {
    keys.iter().find_map(|k| payload.get(*k).filter(|v| truthy(v))).map(|v| text_or_empty(Some(v)).trim().to_string()).unwrap_or_default()
}

/// The pack's check that a Gemma file was chosen, which the other runners do not need.
fn require_model(payload: &Obj, message: &str) -> Result<(), String> {
    if or_text(payload, &["model_file", "gemma_model_file", "qwen_model_file"]).is_empty() && !is_external(payload) {
        return Err(message.to_string());
    }
    Ok(())
}

fn or_none(text: &str) -> &str {
    if text.is_empty() { "(none)" } else { text }
}

fn lines_or_none(lines: &[String]) -> String {
    if lines.is_empty() { "(none)".to_string() } else { lines.join("\n") }
}

/// What the reply carries about the run: the model, the runner and whether the model was unloaded.
fn finish(mut map: Obj, info: &Obj) -> Value {
    map.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    map.insert("runner".into(), info.get("runner").cloned().unwrap_or_else(|| json!("builtin")));
    map.insert("unloaded".into(), info.get("unloaded").cloned().unwrap_or_else(|| json!(true)));
    Value::Object(map)
}

fn with_raw_text(text: &str) -> Obj {
    let mut map = Obj::new();
    map.insert("raw_text".into(), json!(text));
    map
}

fn locations_json(locations: &[Location]) -> Value {
    Value::Array(locations.iter().map(|l| json!({"name": l.name, "description": l.description})).collect())
}

/// The scenes of a request: the list, or the message when it is not one or is empty.
fn scene_items(payload: &Obj, none_error: &str) -> Result<Vec<Value>, String> {
    match payload.get("scenes") {
        Some(Value::Array(items)) if !items.is_empty() => Ok(items.clone()),
        _ => Err(none_error.to_string()),
    }
}

fn scene_id(scene: &Obj, index: usize) -> String {
    let id = text(scene, "id");
    if id.is_empty() { format!("scene_{index}") } else { id }
}

fn scene_label(scene: &Obj, index: usize) -> String {
    let label = text(scene, "label");
    if label.is_empty() { format!("Scene {index}") } else { label }
}

/// The scenes that have a concept or notes.
fn scenes_with_text(items: &[Value]) -> Vec<Scene> {
    let mut scenes = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(scene) = item.as_object() else { continue };
        let (concept, notes) = (text(scene, "concept"), text(scene, "notes"));
        if !concept.is_empty() || !notes.is_empty() {
            scenes.push(Scene { id: scene_id(scene, index + 1), label: scene_label(scene, index + 1), concept, notes });
        }
    }
    scenes
}

/// Locations the project has: `name` and `description` of each entry that has a name.
fn existing_cards(payload: &Obj) -> Vec<(String, String)> {
    let mut cards = Vec::new();
    if let Some(Value::Array(items)) = payload.get("existing_locations") {
        for item in items {
            let Some(item) = item.as_object() else { continue };
            let (name, description) = (text(item, "name"), text(item, "description"));
            if !name.is_empty() {
                cards.push((name, description));
            }
        }
    }
    cards
}

fn card_line(name: &str, description: &str) -> String {
    format!("- {name}") + &if description.is_empty() { String::new() } else { format!(": {description}") }
}

// -- describing a picture --------------------------------------------------------------------------------------------------

/// `POST describe_reference_image`: one description of a reference picture, with a calmer second try when the first is unusable.
pub async fn describe_reference_image(llm: &dyn Llm, payload: &Obj) -> Reply {
    let reference_type = describe::normalize_reference_type(&or_text(payload, &["reference_type"]));
    let name = or_text(payload, &["name"]);
    require_model(payload, "Choose a Gemma vision model first.")?;
    let image = image_from_payload(&text(payload, "image_path"), &text(payload, "image_data"), "Reference image")?;
    let count = if reference_type == "extra" {
        let raw = payload.get("count").filter(|v| truthy(v)).map_or(Some(1.0), to_float);
        raw.filter(|f| f.is_finite()).map_or(1, |f| round_half_even(f).clamp(1, 100) as u32)
    } else {
        1
    };
    let instruction = describe::instruction(&reference_type, &name, count, &or_text(payload, &["style"]));
    let temperature = number(payload, "temperature", 0.2)?;
    let top_p = number(payload, "top_p", 0.9)?;
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 180)?);
    let mut last_error = None;
    let mut description = String::new();
    let mut info = Obj::new();
    for attempt in 0..2 {
        let mut asked = instruction.clone();
        if attempt > 0 {
            asked.push_str("\n\nPrevious output was rejected because it repeated words or was not a usable description. Write a normal visual description with varied concrete nouns. Do not repeat any word more than twice.");
        }
        let (temperature, top_p) = if attempt > 0 { (0.05, 0.75) } else { (temperature, top_p) };
        let (raw, run) = llm.vision(VisionCall { instruction: asked, images: vec![image.clone()], temperature, top_p, max_new_tokens }).await?;
        info = run;
        match describe::clean_reference_description(&raw, &reference_type, &name) {
            Ok(cleaned) => {
                description = cleaned;
                last_error = None;
                break;
            }
            Err(why) => last_error = Some(why),
        }
    }
    if let Some(why) = last_error {
        return Err(why);
    }
    let external = is_external(payload);
    let used_model = if external { or_text(payload, &["lmstudio_model"]) } else { info.get("used_model").and_then(Value::as_str).unwrap_or("").to_string() };
    Ok(json!({
        "description": description,
        "used_model": used_model,
        "used_mmproj": "",
        "runner": vision_runner(payload),
        "unloaded": if external { false } else { payload.get("unload_after").is_none_or(truthy) },
    }))
}

// -- mapping scenes to locations -------------------------------------------------------------------------------------------

/// `POST flux_reference_location_map`: the location each scene is set in, balanced so every location is used.
pub async fn flux_reference_location_map(llm: &dyn Llm, payload: &Obj) -> Reply {
    require_model(payload, "Choose a non-vision Gemma model first.")?;
    let items = scene_items(payload, "No scenes were provided for location mapping.")?;
    let scenes = scenes_with_text(&items);
    if scenes.is_empty() {
        return Err("Scenes need lyrics, scene notes, concept prompts, or timeline notes before Gemma can map locations.".into());
    }
    let subject_scene = clean_location_context_text(&text(payload, "subject_scene_text"));
    let mut locations: Vec<Location> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, description) in existing_cards(payload) {
        if seen.insert(name.to_lowercase()) {
            locations.push(Location { name: collapse(&name), description: collapse(&description) });
        }
    }
    if locations.is_empty() {
        return Err("Auto Map needs locations first. Click Extract Locations or add locations manually, then run Auto Map.".into());
    }
    let scene_lines: Vec<String> = scenes.iter().enumerate().map(|(i, s)| format!("Scene {}\nid: {}\nlabel: {}\nconcept: {}\nnotes: {}", i + 1, s.id, s.label, s.concept, s.notes)).collect();
    let numbered = locations
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{}={}", i + 1, l.name) + &if l.description.is_empty() { String::new() } else { format!(" | {}", l.description) })
        .collect::<Vec<_>>()
        .join("\n");
    let previous = location_usage_counts(payload.get("used_location_counts"), payload.get("previous_assignments"), &locations);
    let mut ordered: Vec<&(String, i64)> = previous.iter().collect();
    ordered.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    let usage = ordered.iter().map(|(name, count)| format!("- {name}: already used {count} time(s)")).collect::<Vec<_>>().join("\n");
    let instruction = format!(
        "You are mapping music-video scenes to an existing location list.\n\n\
         Choose the best existing location number for each scene using the scene lyric line, concept text, notes, visual mood, objects, and environment. \
         First use a location clearly named or implied by the scene text. \
         If the scene has no clear location, choose the closest fit from the location list based on emotional tone and visual atmosphere. \
         Use locations that have not been used yet before repeating locations that were already used. \
         Avoid repeating the same location across too many neighboring scenes when another listed location fits equally well.\n\n\
         Output only simple lines in this exact format:\n\
         Scene1=1\n\
         Scene2=3\n\
         Scene3=3\n\n\
         Rules:\n\
         - Every scene must get one line.\n\
         - Use only location numbers from the list.\n\
         - Prefer the least-used matching location when multiple locations fit.\n\
         - If there are enough scenes, use every listed location at least once before heavy repeats.\n\
         - Do not output JSON, markdown, bullets, explanations, names, or descriptions.\n\
         - Do not invent new locations.\n\n\
         Optional extra context, if any:\n{}\n\n\
         Already used locations before these scenes:\n{}\n\n\
         Locations:\n{numbered}\n\n\
         Scenes:\n\n{}",
        or_none(&subject_scene),
        or_none(&usage),
        scene_lines.join("\n"),
    );
    let temperature = number(payload, "temperature", 0.25)?;
    let top_p = number(payload, "top_p", 0.8)?;
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 5000)?);
    let (reply, info) = llm.text(TextCall::new(instruction, temperature, top_p, max_new_tokens, "Gemma")).await?;
    let mut scene_map = parse_scene_location_number_map(&reply, &scenes, &locations);
    let fallback = fallback_location_map_by_overlap(&scenes, &locations);
    if scene_map.is_empty() {
        scene_map = fallback;
    } else {
        for scene in &scenes {
            scene_map.set_default(&scene.id, fallback.get(&scene.id).unwrap_or(""));
        }
    }
    let scene_map = balance_location_map_by_usage(scene_map, &scenes, &locations, &previous);
    let mut map = Obj::new();
    map.insert("locations".into(), locations_json(&locations));
    map.insert("scene_map".into(), Value::Object(scene_map.iter().map(|(k, v)| (k.to_string(), json!(v))).collect()));
    map.insert("raw_text".into(), json!(reply));
    Ok(finish(map, &info))
}

// -- locations -------------------------------------------------------------------------------------------------------------

/// Locations a model wrote, the text it wrote them in and the run that made them.
struct Found {
    locations: Vec<Location>,
    text: String,
    info: Obj,
}

/// The cards without repeats or things that are not places.
fn dedupe_cards(found: &[Location]) -> Vec<Location> {
    let mut seen = std::collections::HashSet::new();
    let mut cards = Vec::new();
    for item in found {
        let (name, description) = clean_location_card(&item.name, &item.description, Some(&item.name));
        if !valid_location_card(&name, &description) || !seen.insert(name.to_lowercase()) {
            continue;
        }
        cards.push(Location { name, description });
    }
    cards
}

/// The locations in a `{"locations": [...]}` reply.
fn locations_from_json(reply: &str) -> Vec<Location> {
    let mut found = Vec::new();
    let Ok(data) = extract_json_object(&clean_visual_text(reply)) else { return found };
    let raw = match &data {
        Value::Object(map) => map.get("locations").cloned(),
        other => Some(other.clone()),
    };
    let Some(Value::Array(items)) = raw else { return found };
    for item in items.iter().filter_map(Value::as_object) {
        let raw_name = text_or_empty(item.get("name"));
        let name = normalize_location_name(&raw_name);
        let description = collapse(text_or_empty(item.get("description")).trim());
        if !name.is_empty() {
            let (name, description) = clean_location_card(&name, &description, Some(&raw_name));
            if valid_location_card(&name, &description) {
                found.push(Location { name, description });
            }
        }
    }
    found
}

/// `_generate_wizard_locations_from_lyrics`.
async fn locations_from_lyrics(llm: &dyn Llm, payload: &Obj) -> Result<Found, String> {
    require_model(payload, "Choose a non-vision Gemma model first.")?;
    let lyrics = or_text(payload, &["lyrics_text", "lyrics"]);
    let user_input = or_text(payload, &["user_input", "notes"]);
    let style_theme = clean_location_context_text(&text(payload, "style_theme"));
    let subject_context = clean_location_context_text(&text(payload, "subject_context"));
    let max_locations = whole(payload, "max_locations", 8)?.clamp(1, 50);
    if lyrics.is_empty() {
        return Err("Paste or create lyrics before creating locations from lyrics.".into());
    }
    let existing: Vec<String> = existing_cards(payload).iter().map(|(n, d)| card_line(&collapse(n), &collapse(d))).collect();
    let instruction = format!(
        "You are a music video location scout.\n\n\
         The user will provide song lyrics.\n\n\
         Your task is to analyze the mood, imagery, setting clues, themes, and emotional tone of the lyrics, \
         then generate a list of reusable filming locations where the main subject or character could be placed.\n\n\
         Return only actual locations, sets, rooms, buildings, outdoor areas, roads, stages, landscapes, or environments.\n\n\
         Use character/reference descriptions and style/theme only to understand the visual world, era, mood, genre, and design language. \
         Do not turn characters, clothing, props, accessories, or body details into location names.\n\n\
         Rules:\n\n\
         Do not summarize the lyrics.\n\
         Do not explain the song meaning.\n\
         Do not quote long lyric sections.\n\
         Focus on visual places that could realistically appear in a music video and hold the subject.\n\
         Include literal locations from the lyrics and cinematic locations inspired by the mood, but they must still be real places.\n\
         Do not output props, objects, clothing, accessories, body parts, people, characters, creatures, or symbolic items as locations.\n\
         Bad location names: Vintage shaving kit, Ornate sugar bowl, Lace veil, Silver frame, Human tooth, Ghostly face, Tattered dress.\n\
         Good location names: Grand ballroom, Dark wood study, Steamy bathroom, Overgrown garden, Dimly lit hallway, Empty stage.\n\
         Descriptions must describe only the place: architecture, layout, surfaces, lighting, weather, atmosphere, era, and color.\n\
         Do not include characters or actions in descriptions. No bride, woman, man, face, hand, tooth, dress, razor, locket, veil, or similar subject/object details.\n\
         Test every idea: could the selected subject stand, walk, sit, perform, or be filmed inside this place? If no, reject it.\n\
         Make the locations specific and cinematic.\n\
         Output no more than {max_locations} locations. Prefer reusable locations rather than one location per scene.\n\
         Use short bullet points.\n\
         Each bullet should be a place only, with a brief visual detail if helpful.\n\n\
         Output format:\n\n\
         Music Video Locations:\n\n\
         - [location idea]\n\
         - [location idea]\n\
         - [location idea]\n\n\
         Existing user locations to avoid duplicating exactly:\n{}\n\n\
         Optional style/theme guidance:\n{}\n\n\
         Character/reference descriptions for style guidance only:\n{}\n\n\
         Optional user input:\n{}\n\n\
         User lyrics:\n{lyrics}",
        lines_or_none(&existing),
        or_none(&style_theme),
        or_none(&subject_context),
        or_none(&user_input),
    );
    let tokens = whole(payload, "max_new_tokens", 2200)?;
    let (mut reply, mut info) = llm.text(TextCall::new(instruction, number(payload, "temperature", 0.45)?, number(payload, "top_p", 0.9)?, tokens, "Gemma")).await?;
    let mut locations = parse_location_ideas_flexible(&reply);
    if locations.is_empty() {
        let retry = format!(
            "Return no more than {max_locations} reusable music video filming locations for the lyrics below.\n\
             Do not write a heading.\n\
             Do not explain anything.\n\
             Do not summarize the lyrics.\n\
             Write one location per line as a short bullet.\n\
             Each bullet must be a specific cinematic place where the subject could stand, walk, sit, perform, or be filmed.\n\
             Reject props, objects, clothing, accessories, body parts, people, characters, creatures, symbolic items, and actions.\n\
             Descriptions must describe only the place/background, not characters or objects from the lyrics.\n\
             Example format:\n\
             - Abandoned motel pool, turquoise water under buzzing neon signs\n\
             - Foggy pine road, wet asphalt and fading headlights\n\n\
             Optional style/theme guidance:\n{}\n\n\
             Character/reference descriptions for style guidance only:\n{}\n\n\
             Optional user input:\n{}\n\n\
             Lyrics:\n{lyrics}",
            or_none(&style_theme),
            or_none(&subject_context),
            or_none(&user_input),
        );
        let (retry_reply, retry_info) = llm.text(TextCall::new(retry, number(payload, "temperature", 0.55)?, number(payload, "top_p", 0.9)?, tokens, "Gemma")).await?;
        let retry_locations = parse_location_ideas_flexible(&retry_reply);
        if !retry_locations.is_empty() {
            reply = retry_reply;
            info = retry_info;
            locations = retry_locations;
        }
    }
    if locations.is_empty() {
        locations = parse_location_lines(&reply);
    }
    if locations.is_empty() {
        let shown = preview(&reply);
        return Err(format!("Gemma did not return any usable location ideas. Raw response preview: {}", if shown.is_empty() { "(empty)" } else { &shown }));
    }
    let cards = dedupe_cards(&locations);
    if cards.is_empty() {
        let shown = preview(&reply);
        return Err(format!("Gemma returned only non-location items. Raw response preview: {}", if shown.is_empty() { "(empty)" } else { &shown }));
    }
    Ok(Found { locations: cards.into_iter().take(max_locations as usize).collect(), text: reply, info })
}

/// `POST wizard_locations_from_lyrics`: reusable filming locations for a song's lyrics.
pub async fn wizard_locations_from_lyrics(llm: &dyn Llm, payload: &Obj) -> Reply {
    let found = locations_from_lyrics(llm, payload).await?;
    Ok(finish(
        {
            let mut map = Obj::new();
            map.insert("locations".into(), locations_json(&found.locations));
            map.extend(with_raw_text(&found.text));
            map
        },
        &found.info,
    ))
}

/// `POST flux_reference_extract_locations`: reusable locations from the scenes, in up to four steps (the line format, a plainer
/// request, the lyrics scout, and then giving up).
pub async fn flux_reference_extract_locations(llm: &dyn Llm, payload: &Obj) -> Reply {
    require_model(payload, "Choose a non-vision Gemma model first.")?;
    let items = scene_items(payload, "No scenes were provided for location extraction.")?;
    let scenes = scenes_with_text(&items);
    if scenes.is_empty() {
        return Err("Scenes need lyrics, scene notes, concept prompts, or timeline notes before Gemma can extract locations.".into());
    }
    let subject_scene = clean_location_context_text(&text(payload, "subject_scene_text"));
    let style_theme = clean_location_context_text(&text(payload, "style_theme"));
    let subject_context = clean_location_context_text(&text(payload, "subject_context"));
    let max_locations = whole(payload, "max_locations", 8)?.clamp(1, 50);
    let existing: Vec<String> = existing_cards(payload).iter().map(|(n, d)| card_line(n, d)).collect();
    let scene_lines: Vec<String> = scenes.iter().enumerate().map(|(i, s)| format!("Scene {}: {}\nconcept: {}\nnotes: {}", i + 1, s.label, s.concept, s.notes)).collect();
    let context = format!(
        "Optional style/theme guidance:\n{}\n\nCharacter/reference descriptions for style guidance only:\n{}\n\nOptional extra context:\n{}\n\nExisting user locations:\n{}\n\nScenes:\n\n{}",
        or_none(&style_theme),
        or_none(&subject_context),
        or_none(&subject_scene),
        lines_or_none(&existing),
        scene_lines.join("\n"),
    );
    let instruction = format!(
        "Extract a short reusable location list for Flux/Klein or Nano B reference images.\n\n\
         Use the scene concept prompts and scene notes as the source of truth. \
         Optional extra context may be empty; if it is empty or missing, ignore it completely. \
         Use character/reference descriptions and style/theme only to understand the visual world, era, mood, genre, and design language. \
         Do not turn characters, clothing, props, accessories, or body details into location names. \
         Find concrete physical places, sets, rooms, buildings, landscapes, or backgrounds that repeat or are useful as references. \
         If the extra context includes locations not directly named in a concept prompt, include them when they fit the project.\n\n\
         Output only simple lines in this exact format:\n\
         1|location name|short visual description for a reference image\n\
         2|location name|short visual description for a reference image\n\n\
         Rules:\n\
         - Do not output JSON, markdown, bullets, headings, or explanations.\n\
         - Keep names short, like bedroom, foggy white forest, salt-flat desert, ruined opera house.\n\
         - Every name must be an actual place where the subject could stand, walk, sit, perform, or be filmed.\n\
         - Do not output props, objects, clothing, accessories, body parts, people, characters, creatures, or symbolic items as locations.\n\
         - Descriptions must describe only the place/background: architecture, layout, surfaces, lighting, weather, atmosphere, era, and color.\n\
         - Do not include characters or actions in descriptions. No bride, woman, man, face, hand, tooth, dress, razor, locket, veil, or similar subject/object details.\n\
         - Reuse broad locations instead of creating one unique location for every scene.\n\
         - Return no more than {max_locations} locations. Reuse broad locations instead of exceeding this maximum.\n\n\
         {context}",
    );
    let tokens = whole(payload, "max_new_tokens", 2200)?;
    let (mut reply, mut info) = llm.text(TextCall::new(instruction, number(payload, "temperature", 0.2)?, number(payload, "top_p", 0.8)?, tokens, "Gemma")).await?;
    let mut locations = parse_location_lines(&reply);
    if locations.is_empty() {
        locations = parse_location_ideas_flexible(&reply);
    }
    if locations.is_empty() {
        locations = locations_from_json(&reply);
    }
    if locations.is_empty() {
        let retry = format!(
            "Return only reusable music video filming locations from the scene text below.\n\
             Do not explain anything.\n\
             Do not summarize the scenes.\n\
             Write no more than {max_locations} locations.\n\
             Write one location per line as a short bullet.\n\
             Each bullet must be a concrete visual place/background where the subject could stand or move.\n\
             Reject props, objects, clothing, accessories, people, body parts, symbolic items, and actions.\n\
             Describe only the environment, not characters or objects from the lyrics.\n\
             If any optional context is missing, invisible, empty, or unavailable, ignore that and use only the scene text.\n\
             Never mention missing files, missing context, subjectsandscenes.txt, prompts, or instructions.\n\
             Example format:\n\
             - Abandoned motel pool, turquoise water under buzzing neon signs\n\
             - Foggy pine road, wet asphalt and fading headlights\n\n\
             {context}",
        );
        let (retry_reply, retry_info) = llm.text(TextCall::new(retry, number(payload, "temperature", 0.35)?, number(payload, "top_p", 0.9)?, tokens, "Gemma")).await?;
        let mut retry_locations = parse_location_lines(&retry_reply);
        if retry_locations.is_empty() {
            retry_locations = parse_location_ideas_flexible(&retry_reply);
        }
        if !retry_locations.is_empty() {
            reply = retry_reply;
            info = retry_info;
            locations = retry_locations;
        }
    }
    if locations.is_empty() {
        let mut scout = payload.clone();
        scout.insert("subject_scene_text".into(), json!(""));
        scout.insert("style_theme".into(), json!(style_theme));
        scout.insert("subject_context".into(), json!(subject_context));
        scout.insert("lyrics_text".into(), json!(scene_lines.join("\n\n")));
        scout.insert("user_input".into(), json!("Create reusable location ideas from these music-video scene lines."));
        let scout_ask = |key: &str, default: f64| number(payload, key, default).unwrap_or(default);
        scout.insert("temperature".into(), json!(scout_ask("temperature", 0.45)));
        scout.insert("top_p".into(), json!(scout_ask("top_p", 0.9)));
        scout.insert("max_new_tokens".into(), json!(tokens));
        // Any failure of the scout leaves the earlier reply to report on.
        if let Ok(found) = locations_from_lyrics(llm, &scout).await {
            if !found.locations.is_empty() {
                reply = found.text;
                for key in ["used_model", "runner", "unloaded"] {
                    if let Some(value) = found.info.get(key) {
                        info.insert(key.into(), value.clone());
                    }
                }
                locations = found.locations;
            }
        }
    }
    if locations.is_empty() {
        let shown = preview(&reply);
        return Err(format!("Gemma did not return any usable locations. Raw response preview: {}", if shown.is_empty() { "(empty)" } else { &shown }));
    }
    let cards = dedupe_cards(&locations);
    if cards.is_empty() {
        let shown = preview(&reply);
        return Err(format!("Gemma returned only non-location items. Raw response preview: {}", if shown.is_empty() { "(empty)" } else { &shown }));
    }
    let kept: Vec<Location> = cards.into_iter().take(max_locations as usize).collect();
    let mut map = Obj::new();
    map.insert("locations".into(), locations_json(&kept));
    map.extend(with_raw_text(&reply));
    Ok(finish(map, &info))
}

/// `POST lm_scout_locations` ("LM Extract"): a location scout's JSON list from the lyrics, the scene text and the style.
pub async fn lm_scout_locations(llm: &dyn Llm, payload: &Obj) -> Reply {
    require_model(payload, "Choose a non-vision Gemma model first, or use LM Studio, LLM API, or your own server.")?;
    // The scene numbers count every entry of the list, kept or not.
    let scenes: Vec<(String, String)> = match payload.get("scenes") {
        Some(Value::Array(items)) => items.iter().map(|i| i.as_object().map(|s| (text(s, "concept"), text(s, "notes"))).unwrap_or_default()).collect(),
        _ => Vec::new(),
    };
    let input = ScoutInput {
        lyrics_text: text(payload, "lyrics_text"),
        scenes,
        style_theme: text(payload, "style_theme"),
        subject_context: text(payload, "subject_context"),
        existing_locations: existing_names(payload),
        ..ScoutInput::default()
    };
    let instruction = scout_instruction(&input)?;
    let mut call = TextCall::new(instruction, number(payload, "temperature", 0.6)?, number(payload, "top_p", 0.9)?, whole(payload, "max_new_tokens", 4000)?, "Gemma");
    call.json_schema = Some(scout_schema());
    let (reply, info) = llm.text(call).await?;
    let locations = parse_scout_reply(&reply)?;
    let mut map = Obj::new();
    map.insert("locations".into(), json!(locations.iter().map(|l| json!({"name": l.name, "description": l.description})).collect::<Vec<_>>()));
    map.extend(with_raw_text(&reply));
    Ok(finish(map, &info))
}

/// The names of the locations the project has, from entries that are objects.
fn existing_names(payload: &Obj) -> Vec<String> {
    match payload.get("existing_locations") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_object).map(|i| collapse(text(i, "name").trim())).collect(),
        _ => Vec::new(),
    }
}

// -- subjects --------------------------------------------------------------------------------------------------------------

/// `_load_scene_notes_json`: the timeline notes saved with the project, by scene number.
fn scene_notes(project_folder: &str) -> Result<HashMap<usize, String>, String> {
    let folder = project_folder.trim().trim_matches('"');
    let mut notes = HashMap::new();
    if folder.is_empty() {
        return Ok(notes);
    }
    let path = scene_notes_path(Path::new(folder));
    if !path.is_file() {
        return Ok(notes);
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let body = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string();
    let data: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let Value::Object(map) = data else { return Ok(notes) };
    let digits = rx(r"(\d+)");
    for (key, value) in map {
        if let Some(number) = digits.captures(&key).ok().flatten().and_then(|c| c.get(1)).and_then(|m| m.as_str().parse::<usize>().ok()) {
            notes.insert(number, text_or_empty(Some(&value)).trim().to_string());
        }
    }
    Ok(notes)
}

/// `POST flux_reference_extract_subjects`: the recurring characters of the scenes, a line each.
pub async fn flux_reference_extract_subjects(llm: &dyn Llm, payload: &Obj) -> Reply {
    require_model(payload, "Choose a non-vision Gemma model first.")?;
    let items = scene_items(payload, "No scenes were provided for subject extraction.")?;
    let fallbacks = scene_notes(&text(payload, "project_folder"))?;
    struct SubjectScene {
        label: String,
        concept: String,
        notes: String,
        director_note: String,
    }
    let mut scenes: Vec<SubjectScene> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(scene) = item.as_object() else { continue };
        let (concept, notes) = (text(scene, "concept"), text(scene, "notes"));
        let mut director_note = text(scene, "director_note");
        if director_note.is_empty() {
            director_note = fallbacks.get(&(index + 1)).cloned().unwrap_or_default();
        }
        if !concept.is_empty() || !notes.is_empty() || !director_note.is_empty() {
            scenes.push(SubjectScene { label: scene_label(scene, index + 1), concept, notes, director_note });
        }
    }
    if scenes.is_empty() {
        return Err("Scenes need concept prompt text, notes, or Director Notes before Gemma can extract subjects.".into());
    }
    let requested = whole(payload, "requested_count", 2)?.clamp(2, 12);
    let subject_scene = text(payload, "subject_scene_text");
    let mut existing = Vec::new();
    if let Some(Value::Array(subjects)) = payload.get("existing_subjects") {
        for item in subjects.iter().filter_map(Value::as_object) {
            let (name, description) = (text(item, "name"), text(item, "description"));
            if !name.is_empty() && !description.is_empty() {
                existing.push(format!("- {name}: {description}"));
            }
        }
    }
    let scene_lines: Vec<String> = scenes.iter().enumerate().map(|(i, s)| format!("Scene {}: {}\nconcept: {}\nnotes: {}\ndirector_note: {}", i + 1, s.label, s.concept, s.notes, s.director_note)).collect();
    let instruction = format!(
        "Extract reusable character/subject identities for reference images.\n\n\
         Look at the scene concept prompts, notes, Director Notes, and subject/scene context. \
         Find distinct recurring people, creatures, mascots, or main visual subjects that may need separate reference images. \
         The user expects about {requested} character references if the project supports that count.\n\n\
         Output only simple lines in this exact format:\n\
         1|character name|short visual description for a character reference image\n\
         2|character name|short visual description for a character reference image\n\n\
         Rules:\n\
         - Do not output JSON, markdown, bullets, headings, or explanations.\n\
         - Keep names short and stable, like blonde woman, masked man, young singer, red android.\n\
         - Descriptions must describe identity, face/body, hair, outfit, colors, and visual consistency details.\n\
         - Do not describe locations as characters.\n\
         - If two characters appear together in a scene, list them as separate characters, not one combined subject.\n\n\
         Subject/scene context:\n{}\n\n\
         Existing user subjects:\n{}\n\n\
         Scenes:\n\n{}",
        or_none(&subject_scene),
        lines_or_none(&existing),
        scene_lines.join("\n"),
    );
    let call = TextCall::new(instruction, number(payload, "temperature", 0.2)?, number(payload, "top_p", 0.8)?, whole(payload, "max_new_tokens", 2200)?, "Gemma");
    let (reply, info) = llm.text(call).await?;
    let subjects = parse_subject_lines(&reply);
    if subjects.is_empty() {
        return Err("Gemma did not return any usable subjects.".into());
    }
    let mut seen = std::collections::HashSet::new();
    let deduped: Vec<Value> = subjects.iter().filter(|s| seen.insert(s.name.to_lowercase())).map(|s| json!({"name": s.name, "description": s.description})).collect();
    let mut map = Obj::new();
    map.insert("subjects".into(), Value::Array(deduped));
    map.extend(with_raw_text(&reply));
    Ok(finish(map, &info))
}

// -- reference sheet prompts -----------------------------------------------------------------------------------------------

const SUBJECT_SHEET: &str = "Create one text-to-image prompt for a character reference sheet.\n\n\
The user input may be a simple subject description or a structured character-creation brief with sections such as generation mode, subject label, reference type, existing description, lyrics, song style, gender/role, reference image notes, and extra user direction. \
When lyrics or song style are provided, use them to invent or refine the character's identity, wardrobe, era, mood, expression, and visual design. Do not quote lyrics or make the final prompt a lyric scene. \
When an existing description or reference image notes are provided, preserve those explicit identity details and use lyrics/style only as supporting visual influence.\n\n\
The image must contain the same character shown three times in one image, with clearly different camera distances:\n\
1. Left panel: extreme close-up face portrait only, from top of hair to just below the chin. The face fills 80-90% of the panel. No shoulders, chest, torso, hands, or outfit details visible except maybe a tiny neckline.\n\
2. Center panel: upper-body waist-up portrait, from head to waist. Face, shoulders, chest, arms, and main outfit details visible.\n\
3. Right panel: full-body standing view, from head to shoes. Entire outfit, body proportions, legs, and feet visible.\n\n\
All three views must show the same person with consistent face, hair, outfit, colors, body type, and identity. \
Use a clean neutral studio background. The character should face forward or mostly forward. Keep lighting clear and even. \
Do not make the left and center panels the same crop. Do not create a cinematic scene, action pose, environment, story moment, props, text labels, captions, logos, watermarks, or multiple different characters.\n\n\
Use this exact output structure:\n\n\
A clean three-panel character reference sheet on a neutral studio background, showing the same [subject] in all panels with consistent [face/hair/body/outfit/identity details]. \
Left panel: extreme close-up face-only portrait, top of hair to just below chin, face fills 80-90% of the panel, no shoulders, no chest, no torso. \
Center panel: upper-body waist-up portrait from head to waist, showing shoulders, chest, arms, and outfit details. \
Right panel: full-body standing view from head to shoes, entire outfit and feet visible. \
Each panel uses a clearly different camera distance: face-only close-up, waist-up portrait, full-body standing. \
Clear even lighting, front-facing pose, consistent outfit colors and body proportions, detailed character design, no text, no labels, no props, no environment.\n\n\
Rules:\n\
- Output only one polished text-to-image prompt.\n\
- Do not include markdown, labels, quotes, explanations, or multiple options.\n\
- Keep it as one single image containing three views of the same character.\n\
- The left panel must be face-only, not another upper-body portrait.\n\
- Preserve the subject identity from the user input.\n\n";

const LOCATION_SHEET: &str = "Create one text-to-image prompt for a reusable location reference image.\n\n\
The image must show only the physical environment/location. Do not include the main character, people, animals, readable text, captions, logos, watermarks, or story action.\n\n\
Important: if the input, style/theme, or surrounding context mentions a character reference sheet, white/neutral studio background, plain backdrop, panel layout, close-up portrait, upper-body portrait, or full-body character view, treat those as character-reference-sheet artifacts only. \
Do not turn those artifacts into the location. Do not create a white studio, neutral studio, seamless photo backdrop, blank room, or character-sheet background unless the user explicitly names that as the intended location.\n\n\
Use this exact output structure:\n\n\
A clear cinematic environment reference image of [location/environment], showing [layout/architecture], [important furniture/props/objects], [lighting details], [colors/materials/textures], and [atmosphere]. \
Wide enough framing to understand the space layout, [time of day/weather if relevant], no people, no animals, no readable text, no logos, no captions.\n\n\
Rules:\n\
- Output only one polished text-to-image prompt.\n\
- Do not include markdown, labels, quotes, explanations, or multiple options.\n\
- Do not include the main character.\n\
- Do not describe a music video action.\n\
- Keep it as a reusable setting/reference image.\n\
- Preserve the location identity from the user input.\n\
- Ignore character-sheet backgrounds, panel layouts, and portrait crops when inventing the setting.\n\
- Use the optional style/theme only for visual mood, lighting, color, or texture.\n\n";

/// `POST flux_reference_zimage_prompt`: the text-to-image prompt of a character sheet or a location picture.
pub async fn flux_reference_zimage_prompt(llm: &dyn Llm, payload: &Obj) -> Reply {
    let reference_type = text(payload, "reference_type").to_lowercase();
    let source_text = text(payload, "source_text");
    let style_theme = text(payload, "style_theme");
    if reference_type != "subject" && reference_type != "location" {
        return Err("Reference type must be subject or location.".into());
    }
    require_model(payload, "Choose a non-vision Gemma model first.")?;
    if source_text.is_empty() {
        return Err("Enter a subject or location description first.".into());
    }
    let style = or_none(&style_theme);
    let instruction = if reference_type == "subject" {
        format!("{SUBJECT_SHEET}Subject creation brief:\n{source_text}\n\nOptional global style/theme:\n{style}")
    } else {
        format!("{LOCATION_SHEET}Location description:\n{source_text}\n\nOptional global style/theme:\n{style}")
    };
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 900)?);
    let call = TextCall::new(instruction, number(payload, "temperature", 0.25)?, number(payload, "top_p", 0.8)?, max_new_tokens, "Gemma");
    let (reply, info) = llm.text(call).await?;
    let prompt = clean_visual_text(&reply);
    validate_builder_prompt(&prompt, &format!("Flux/Klein {reference_type} reference"), None)?;
    let mut map = Obj::new();
    map.insert("prompt".into(), json!(prompt));
    Ok(finish(map, &info))
}
