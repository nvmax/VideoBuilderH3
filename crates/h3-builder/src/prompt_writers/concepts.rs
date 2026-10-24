//! Concept prompts: one short scene concept for every scene, written from the notes, the story idea and the style. Port of
//! `_generate_builder_concept_prompts` in `llm/image_prompt_generation.py`.

use fancy_regex::Regex;
use h3_core::pyval::{py_str, text_or_empty, to_int, truthy, Obj};
use h3_llm::clean::clean_plain_text;
use h3_llm::locations::extract_json_object;
use serde_json::{json, Value};

use super::{number, text, whole, Llm, TextCall};

type Reply = Result<Value, String>;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

/// `str(value or "").strip()[:limit]`.
fn clip(value: Option<&Value>, limit: usize) -> String {
    text_or_empty(value).trim().chars().take(limit).collect()
}

fn scene_flag(scene: &Obj, keys: &[&str]) -> bool {
    keys.iter().any(|k| scene.get(*k).is_some_and(truthy))
}

fn cleaned_scenes(payload: &Obj) -> Result<Vec<Value>, String> {
    let scenes = payload.get("scenes").and_then(Value::as_array).filter(|s| !s.is_empty()).ok_or("No scenes were provided for concept prompt creation.")?;
    let mut cleaned: Vec<Value> = Vec::new();
    for scene in scenes.iter().take(20) {
        let Some(scene) = scene.as_object() else { continue };
        let fallback = cleaned.len() as i64 + 1;
        let number = ["scene_number", "number"].iter().filter_map(|k| scene.get(*k)).find(|v| truthy(v)).map_or(fallback, |v| to_int(v).unwrap_or(fallback));
        let label = Some(clip(scene.get("label"), 160)).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("Scene {number}"));
        let notes: Vec<Value> = scene
            .get("timeline_notes")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .take(8)
                    .filter_map(Value::as_object)
                    .map(|item| {
                        let label = ["label", "type"].iter().map(|k| text_or_empty(item.get(*k)).trim().to_string()).find(|t| !t.is_empty()).unwrap_or_else(|| "note".into());
                        json!({"label": label.chars().take(160).collect::<String>(), "note": clip(item.get("note"), 800), "start": item.get("start").cloned().unwrap_or(Value::Null), "end": item.get("end").cloned().unwrap_or(Value::Null)})
                    })
                    .collect()
            })
            .unwrap_or_default();
        let singers: Vec<Value> = scene
            .get("lyric_singers")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(|i| text_or_empty(Some(i)).trim().chars().take(160).collect::<String>()).filter(|s| !s.is_empty()).take(8).map(Value::from).collect())
            .unwrap_or_default();
        let mode = |key: &str| -> String { scene.get(key).filter(|v| truthy(v)).map_or_else(|| "none".to_string(), |v| py_str(Some(v))).trim().chars().take(80).collect() };
        cleaned.push(json!({
            "scene_number": number.max(1),
            "label": label.chars().take(160).collect::<String>(),
            "lyric_text": clip(scene.get("lyric_text"), 1200),
            "lyric_singers": singers,
            "lyric_instrumental": scene.get("lyric_instrumental").is_some_and(truthy),
            "lyric_no_lip_sync": scene.get("lyric_no_lip_sync").is_some_and(truthy),
            "no_character_present": scene_flag(scene, &["no_character_present", "no_subject", "no_visible_subject"]),
            "mapped_subjects": clip(scene.get("mapped_subjects"), 1600),
            "director_note": clip(scene.get("director_note"), 1200),
            "scene_note": clip(scene.get("scene_note"), 1200),
            "timeline_notes": notes,
            "subject_reference_mode": mode("subject_reference_mode"),
            "location_reference_mode": mode("location_reference_mode"),
        }));
    }
    if cleaned.is_empty() {
        return Err("No valid scenes were provided for concept prompt creation.".into());
    }
    Ok(cleaned)
}

/// The instruction for a batch of scenes.
pub fn instruction(payload: &Obj) -> Result<String, String> {
    let source_mode = Some(text(payload, "source_mode").to_lowercase()).filter(|m| !m.is_empty()).unwrap_or_else(|| "all".into());
    let story_idea = text(payload, "story_idea");
    let theme_style = text(payload, "theme_style");
    let previous = text(payload, "previous_summary");
    let scenes = cleaned_scenes(payload)?;
    let all = "Use all available notes: director notes, raw scene notes, lyric notes, subject/singer mapping, and overlapping timeline notes.";
    let source_note = match source_mode.as_str() {
        "director" => "Use director notes as the main scene notes.",
        "scene" => "Use raw scene notes as the main scene notes.",
        "timeline" => "Use overlapping timeline notes as the main scene notes.",
        "lyrics" => "Use lyric notes as the main scene notes.",
        "subjects" => "Use subject and singer mapping as the main scene notes.",
        "lyrics_subjects" => "Use lyric notes plus subject and singer mapping as the main scene notes.",
        "director_scene" => "Use director notes and raw scene notes as the main scene notes.",
        _ => all,
    };
    let scenes_json = h3_core::pyjson::dumps(&Value::Array(scenes));
    let none = |text: &str, empty: &str| if text.is_empty() { empty.to_string() } else { text.to_string() };
    Ok(format!(
        "You will create concept prompts from scene notes and/or director notes, timeline notes, story idea, and style/theme.\n\
Return only valid JSON in this exact flat format:\n\
{{\n  \"Scene1\": \"\",\n  \"Scene2\": \"\"\n}}\n\n\
Source mode: {source_mode}. {source_note}\n\n\
The story idea explains the overall music video concept, emotional arc, setting, and visual direction. Use it to make the prompts feel connected and to make each scene flow naturally like a music video.\n\
The style/theme explains the visual language, mood, palette, texture, and production design. Use it for consistency.\n\
Character descriptions or character reference images may be provided separately. Do not describe characters' hair, face, clothing, makeup, or exact appearance in the concept prompts.\n\n\
Create one prompt for each provided scene.\n\
Each concept prompt must include who is in the scene, shot type, Location:, and scene details that fit the story world.\n\n\
Subject rules:\n\
- If lyric_singers contains names, those are the subjects/performers for that scene.\n\
- If mapped_subjects contains character reference names, use those as the visible subjects for that scene.\n\
- If no_character_present is true, write only the location/background, environment, architecture, props, objects, weather, lighting, atmosphere, environmental motion, and camera. Do not include, mention, imply, describe, silhouette, reflect, or refer to any person, character, subject, singer, performer, band member, crowd, body part, face, voice, or character reference.\n\
- If lyric_no_lip_sync is true or lyric_instrumental is true, the scene should not be treated as a singing/lip-sync performance scene.\n\
- If the scene note says female, start with \"Female only\".\n\
- If the scene note says male, start with \"Male only\".\n\
- If the scene note says female and male, start with \"Female and male together in frame\".\n\
- If the scene note is instrumental only or has no clear subject, start with \"No main subject\".\n\n\
Music video flow rules:\n\
- Make the prompts feel like connected shots from the same music video.\n\
- Let locations and details progress with the story instead of feeling random.\n\
- Instrumental scenes can be establishing shots, transitions, symbolic visuals, or world-building.\n\
- Character scenes should feel like performance, duet, reaction, memory, confrontation, longing, or story moments.\n\
- Keep the visual world consistent across all prompts.\n\n\
Shot rules:\n\
- Instrumental scenes should usually be wide establishing shots or wide environment shots.\n\
- Female-only or male-only scenes should vary between wide, medium-wide, medium, waist-up, and upper body shots. Use a close-up only when the scene note calls for it.\n\
- Female and male together should usually be full body, two-shot, medium-wide, or wide shot.\n\
- Make the shot type fit the scene note and story moment.\n\n\
Location rules:\n\
- Include \"Location:\" in every prompt.\n\
- Pick locations that fit the story idea, mood, and style/theme.\n\
- If a location reference is available for that scene, use it as the location identity instead of inventing an unrelated location.\n\
- If no location reference is available, create the location from the story idea, style/theme, and notes.\n\
- Keep locations visually connected so the scenes feel like one continuous music video world.\n\n\
Style rules:\n\
- Do not write full text-to-image prompts.\n\
- Do not include character descriptions.\n\
- Do not include camera settings, aspect ratio, render style, or quality tags.\n\
- Keep each prompt as one clear sentence or short paragraph.\n\
- Make prompts detailed enough for another LLM to expand into image prompts.\n\
- Return only the JSON object.\n\n\
Story idea:\n{}\n\n\
Style/theme:\n{}\n\n\
Previous batch visual progression summary:\n{}\n\n\
Scenes:\n{scenes_json}",
        none(&story_idea, "[none provided]"),
        none(&theme_style, "[none provided]"),
        none(&previous, "[none yet]"),
    ))
}

/// `POST generate_concept_prompts`.
pub async fn generate_concept_prompts(llm: &dyn Llm, payload: &Obj) -> Reply {
    let instruction = instruction(payload)?;
    let call = TextCall::new(instruction, number(payload, "temperature", 0.45)?, number(payload, "top_p", 0.95)?, whole(payload, "max_new_tokens", 1200)?, "Concept Prompt Creator").paragraphs();
    let (text, info) = llm.text(call).await?;
    let raw = clean_plain_text(&text);
    let mut prompts = serde_json::Map::new();
    let key = rx(r"(?i)scene\s*(\d+)");
    if let Ok(Value::Object(parsed)) = extract_json_object(&raw) {
        for (name, value) in &parsed {
            let Some(number) = key.captures(name).ok().flatten().and_then(|c| c.get(1)).and_then(|m| m.as_str().parse::<i64>().ok()) else { continue };
            let prompt = text_or_empty(Some(value)).trim().to_string();
            if !prompt.is_empty() {
                prompts.insert(format!("Scene{number}"), json!(prompt));
            }
        }
    }
    if prompts.is_empty() {
        for caps in rx(r#"(?is)"?\bScene\s*(\d+)"?\s*:\s*"([^"]+)""#).captures_iter(&raw).flatten() {
            let (Some(number), Some(body)) = (caps.get(1).and_then(|m| m.as_str().parse::<i64>().ok()), caps.get(2)) else { continue };
            let prompt = body.as_str().trim().to_string();
            if !prompt.is_empty() {
                prompts.insert(format!("Scene{number}"), json!(prompt));
            }
        }
    }
    if prompts.is_empty() {
        return Err("Gemma did not return any SceneN concept prompts.".into());
    }
    let mut keys: Vec<&String> = prompts.keys().collect();
    keys.sort_by_key(|k| rx(r"\d+").find(k).ok().flatten().and_then(|m| m.as_str().parse::<i64>().ok()).unwrap_or(0));
    let lines: Vec<String> = keys.iter().map(|k| format!("{k}: {}", py_str(prompts.get(*k)).chars().take(180).collect::<String>())).collect();
    let summary: String = lines.join("\n").chars().take(1200).collect();
    Ok(json!({"prompts": Value::Object(prompts.clone()), "raw": raw, "summary": summary, "run_info": Value::Object(info)}))
}
