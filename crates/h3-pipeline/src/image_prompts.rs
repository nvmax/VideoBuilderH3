//! Image prompts written by the project's LLM: a scene's still frame, a character reference sheet and a location
//! reference image. One neutral instruction per kind. A short model hint (Z-Image, FLUX or none) sets the style of the
//! text. The pack has one writer per image model. This app picks the image model by workflow preset, so the writer is
//! the same for all of them.

use h3_core::lyric_scenes::is_instrumental_lyric_text;
use h3_core::pyval::{collapse_ws, text_or_empty, truthy, Obj};
use h3_core::scene::scene_cast_and_location;
use h3_core::timeline::sort_segments;
use h3_llm::checks::{clean_visual_text, looks_like_repeat_failure, looks_like_unfilled_template};
use h3_llm::{chat, ChatRequest, LlmConfig};
use serde_json::{json, Value};

use crate::minimax_prompts::PromptError;

const SCENE: &str = include_str!("../data/image_scene.txt");
const SUBJECT_SHEET: &str = include_str!("../data/image_subject_sheet.txt");
const LOCATION: &str = include_str!("../data/image_location.txt");
const MAX_ATTEMPTS: usize = 3;

const HINT_ZIMAGE: &str = "Style of the prompt: one descriptive natural-language paragraph of about 80 to 140 words in plain sentences. Concrete visible detail. No tag lists, no weights, no negative prompt.";
const HINT_FLUX: &str = "Style of the prompt: one natural-language paragraph under 120 words. Start with the shot type and the subject. Concrete visible detail. No tag lists, no weights, no negative prompt.";
const HINT_NEUTRAL: &str = "Style of the prompt: one natural-language paragraph of about 80 to 140 words. Concrete visible detail. No tag lists, no negative prompt.";

/// The text style for an image model name or preset id: `zimage`, `flux`, or neutral for anything else.
pub fn model_hint(name: &str) -> &'static str {
    let lower = name.to_lowercase().replace(['-', ' '], "_");
    if lower.contains("flux") || lower.contains("klein") {
        HINT_FLUX
    } else if lower.contains("zimage") || lower.contains("z_image") || lower == "z" || lower.starts_with("z_") {
        HINT_ZIMAGE
    } else {
        HINT_NEUTRAL
    }
}

#[derive(Debug, Clone)]
pub struct ImagePromptParams {
    pub temperature: f64,
    pub top_p: f64,
    pub max_new_tokens: u32,
    /// An image model name or preset id that picks the text style.
    pub model: String,
    /// Extra direction from the caller, added to the input.
    pub extra_notes: String,
}

impl Default for ImagePromptParams {
    fn default() -> Self {
        Self { temperature: 0.6, top_p: 0.95, max_new_tokens: 1200, model: String::new(), extra_notes: String::new() }
    }
}

#[derive(Debug, Clone)]
pub struct WrittenImagePrompt {
    pub prompt: String,
    pub model: String,
    pub attempts: usize,
}

fn field(obj: &Obj, key: &str) -> String {
    text_or_empty(obj.get(key)).trim().to_string()
}

fn describe(card: &Obj) -> String {
    let (name, description) = (field(card, "name"), field(card, "description"));
    match (name.is_empty(), description.is_empty()) {
        (_, true) => name,
        (true, false) => description,
        (false, false) => format!("{name}: {description}"),
    }
}

/// The scene input the LLM reads: notes, mapped characters and location, lyric as mood, story beat and story context.
pub fn scene_notes(session: &Obj, segment: &Obj, index: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut add = |title: &str, value: String| {
        let value = value.trim().to_string();
        if !value.is_empty() {
            parts.push(format!("{title}:\n{value}"));
        }
    };
    let session_value = Value::Object(session.clone());
    let (cast, location) = scene_cast_and_location(&session_value, segment, index);
    let no_character = segment.get("no_character_present").is_some_and(truthy);
    let cast: Vec<&Obj> = if no_character { vec![] } else { cast.iter().collect() };
    let lyric_raw = ["lyric_text", "lyrics"].iter().map(|k| field(segment, k)).find(|t| !t.is_empty()).unwrap_or_default();
    let lyric = if is_instrumental_lyric_text(&lyric_raw) { String::new() } else { collapse_ws(&lyric_raw) };

    add("Scene notes", field(segment, "notes"));
    add("Director note", field(segment, "timeline_note"));
    add("Mapped characters", cast.iter().map(|c| field(c, "name")).filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", "));
    add("Character descriptions", cast.iter().map(|c| describe(c)).filter(|d| !d.is_empty()).collect::<Vec<_>>().join("\n"));
    if no_character {
        add("Characters", "No character is present in this scene.".into());
    }
    if let Some(loc) = &location {
        add("Mapped location", describe(loc));
    }
    add("Lyric line (mood only, never quote it)", lyric);
    add("Lyric section", field(segment, "lyric_section"));
    add("Story beat", field(segment, "story_beat"));
    add("Still shot direction", field(segment, "shot_type"));
    let layer = session.get("builder_story_layer").or_else(|| session.get("builderStoryLayer")).and_then(Value::as_object);
    if let Some(layer) = layer.filter(|l| l.get("enabled") != Some(&json!(false))) {
        add("Story arc", field(layer, "user_story_arc"));
        add("Song story brief", field(layer, "song_story_brief"));
        let world = field(layer, "image_world_style");
        if !world.is_empty() && world != "natural" {
            add("Image world style", world);
        }
    }
    if parts.is_empty() {
        let label = Some(field(segment, "label")).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("Scene {}", index + 1));
        parts.push(format!("Scene:\n{label}"));
        parts.push("Direction:\nCreate a cinematic image prompt that fits this scene.".into());
    }
    parts.join("\n\n")
}

/// The position of a scene (by id or 1-based number) in timeline order, which the writers expect.
pub fn sorted_index_of(session: &Obj, scene_ref: &str) -> Option<usize> {
    let mut segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    sort_segments(&mut segments);
    let wanted = scene_ref.trim();
    segments.iter().position(|s| text_or_empty(s.get("id")) == wanted).or_else(|| wanted.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= segments.len()).map(|n| n - 1))
}

fn strip_wrapping(text: &str) -> String {
    let mut out = text.trim().to_string();
    for label in ["Image prompt:", "Prompt:", "Final prompt:", "Text-to-image prompt:"] {
        if out.to_lowercase().starts_with(&label.to_lowercase()) {
            out = out[label.len()..].trim().to_string();
        }
    }
    let quotes: &[(char, char)] = &[('"', '"'), ('\u{201c}', '\u{201d}'), ('\'', '\'')];
    for (open, close) in quotes {
        if out.len() > 2 && out.starts_with(*open) && out.ends_with(*close) {
            out = out[open.len_utf8()..out.len() - close.len_utf8()].trim().to_string();
        }
    }
    out
}

/// A reply as one clean prompt, or why it cannot be used.
pub fn clean_reply(text: &str) -> Result<String, String> {
    let cleaned = strip_wrapping(&clean_visual_text(text));
    if cleaned.chars().filter(|c| c.is_alphabetic()).count() < 20 {
        return Err("The LLM answered with no usable prompt.".into());
    }
    if looks_like_unfilled_template(&cleaned) {
        return Err("The LLM left template brackets unfilled.".into());
    }
    if looks_like_repeat_failure(&cleaned) {
        return Err("The LLM answer repeats itself.".into());
    }
    Ok(cleaned)
}

async fn ask(llm: &LlmConfig, request: String, params: &ImagePromptParams) -> Result<WrittenImagePrompt, PromptError> {
    let mut last = String::from("The LLM answered with no usable prompt.");
    let mut model = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let reply = chat(llm, &ChatRequest::text(&request, params.temperature, params.top_p, params.max_new_tokens)).await?;
        model = reply.model.clone();
        match clean_reply(&reply.text) {
            Ok(prompt) => return Ok(WrittenImagePrompt { prompt, model, attempts: attempt }),
            Err(why) => last = why,
        }
    }
    Err(PromptError::Invalid(format!("{last} It was asked {MAX_ATTEMPTS} times (model {model}).")))
}

fn with_hint(instruction: &str, params: &ImagePromptParams) -> String {
    instruction.replace("{{MODEL_HINT}}", model_hint(&params.model))
}

/// Write the still image prompt of the scene at `index` (position in the timeline order).
pub async fn write_scene_image_prompt(llm: &LlmConfig, session: &Obj, index: usize, params: &ImagePromptParams) -> Result<WrittenImagePrompt, PromptError> {
    let mut segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    sort_segments(&mut segments);
    let segment = segments.get(index).and_then(Value::as_object).cloned().ok_or_else(|| PromptError::Invalid("The scene was not found.".into()))?;
    let mut notes = scene_notes(session, &segment, index);
    if !params.extra_notes.trim().is_empty() {
        notes = format!("Extra direction:\n{}\n\n{notes}", params.extra_notes.trim());
    }
    ask(llm, format!("{}\n\nScene input:\n{notes}", with_hint(SCENE, params)), params).await
}

/// Write the prompt for a character reference sheet (`subjects`) or a location reference image (`locations`).
pub async fn write_reference_image_prompt(llm: &LlmConfig, session: &Obj, kind: &str, card_id: &str, params: &ImagePromptParams) -> Result<WrittenImagePrompt, PromptError> {
    let refs = session.get("flux_reference_builder").and_then(Value::as_object).cloned().unwrap_or_default();
    let list = refs.get(kind).and_then(Value::as_array).cloned().unwrap_or_default();
    let card = list.iter().filter_map(Value::as_object).find(|c| field(c, "id") == card_id).cloned().ok_or_else(|| PromptError::Invalid(format!("Reference '{card_id}' was not found in the Reference Builder.")))?;
    let brief = describe(&card);
    if brief.is_empty() {
        return Err(PromptError::Invalid("Give the reference a name or description first.".into()));
    }
    let instruction = match kind {
        "subjects" => SUBJECT_SHEET,
        "locations" => LOCATION,
        _ => return Err(PromptError::Invalid("kind must be 'subjects' or 'locations'.".into())),
    };
    let layer = session.get("builder_story_layer").or_else(|| session.get("builderStoryLayer")).and_then(Value::as_object).cloned().unwrap_or_default();
    let world = field(&layer, "image_world_style");
    let theme = [params.extra_notes.trim().to_string(), if world == "natural" { String::new() } else { world }].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(". ");
    let theme = if theme.is_empty() { "(none)".to_string() } else { theme };
    let label = if kind == "subjects" { "Character brief" } else { "Location description" };
    ask(llm, format!("{}\n\n{label}:\n{brief}\n\nOptional global style or theme:\n{theme}", with_hint(instruction, params)), params).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> Obj {
        v.as_object().unwrap().clone()
    }

    fn session() -> Obj {
        obj(json!({
            "segments": [
                {"id": "s1", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "hold me close tonight", "story_beat": "She sings on a roof.", "notes": "wide shot"},
                {"id": "s2", "start": 4.0, "end": 8.0, "lyric_text": "[instrumental]", "no_character_present": true},
                {"id": "s3", "start": 8.0, "end": 12.0},
            ],
            "flux_reference_builder": {
                "subjects": [{"id": "a", "name": "Ava", "description": "short dark hair, denim jacket"}],
                "locations": [{"id": "l1", "name": "Roof", "description": "a rooftop at dusk"}],
                "subject_scene_map": {"s1": ["a"], "s2": ["a"], "s3": ["a"]},
                "scene_map": {"s1": "l1", "s2": "l1"},
                "use_subject_reference": true,
            },
            "builder_story_layer": {"user_story_arc": "a rise", "image_world_style": "natural"},
        }))
    }

    #[test]
    fn hints_follow_the_model_name() {
        assert_eq!(model_hint("flux_klein"), HINT_FLUX);
        assert_eq!(model_hint("FLUX.2 Klein"), HINT_FLUX);
        assert_eq!(model_hint("zimage"), HINT_ZIMAGE);
        assert_eq!(model_hint("Z-Image Turbo"), HINT_ZIMAGE);
        assert_eq!(model_hint("krea2"), HINT_NEUTRAL);
        assert_eq!(model_hint(""), HINT_NEUTRAL);
    }

    #[test]
    fn scene_input_lists_cast_location_and_context() {
        let s = session();
        let segments = s["segments"].as_array().unwrap();
        let notes = scene_notes(&s, segments[0].as_object().unwrap(), 0);
        assert!(notes.contains("Scene notes:\nwide shot"), "{notes}");
        assert!(notes.contains("Mapped characters:\nAva"), "{notes}");
        assert!(notes.contains("Ava: short dark hair, denim jacket"), "{notes}");
        assert!(notes.contains("Mapped location:\nRoof: a rooftop at dusk"), "{notes}");
        assert!(notes.contains("Lyric line (mood only, never quote it):\nhold me close tonight"), "{notes}");
        assert!(notes.contains("Story arc:\na rise") && !notes.contains("Image world style"), "{notes}");
    }

    #[test]
    fn instrumental_and_empty_scenes_are_handled() {
        let s = session();
        let segments = s["segments"].as_array().unwrap();
        let second = scene_notes(&s, segments[1].as_object().unwrap(), 1);
        assert!(second.contains("No character is present") && !second.contains("Mapped characters") && !second.contains("Lyric line"), "{second}");
        let third = scene_notes(&s, segments[2].as_object().unwrap(), 2);
        assert!(third.contains("Mapped characters:\nAva") && !third.contains("Mapped location"), "{third}");
        let bare = obj(json!({"segments": [{"id": "x", "start": 0.0, "end": 4.0, "label": "Intro"}]}));
        let notes = scene_notes(&bare, bare["segments"][0].as_object().unwrap(), 0);
        assert!(notes.contains("Scene:\nIntro") && notes.contains("Create a cinematic image prompt"), "{notes}");
    }

    #[test]
    fn replies_are_cleaned_and_bad_ones_refused() {
        let ok = clean_reply("<think>hm</think>\nPrompt: \"A woman with short dark hair stands on a rooftop at dusk, wind in her denim jacket, wide shot.\"").unwrap();
        assert_eq!(ok, "A woman with short dark hair stands on a rooftop at dusk, wind in her denim jacket, wide shot.");
        assert!(clean_reply("ok").is_err());
        assert!(clean_reply("A [subject] stands in [environment/location] at [time of day] with [lighting details].").is_err());
    }
}
