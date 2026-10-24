//! The Builder's image prompt writers: text to image prompts, NanoBanana and Flux/Klein prompts from reference pictures,
//! prompt edits and notes about Story Builder pictures. Ports of the functions of the same names in
//! `llm/image_prompt_generation.py` and `_image_prompt_edit_instructions` in `llm/prompts/image.py`.

use std::path::Path;

use fancy_regex::Regex;
use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_llm::checks::{clean_visual_text, looks_like_repeat_failure};
use h3_llm::clean::clean_plain_text;
use h3_llm::prompt_text::extract_prompt_text;
use h3_llm::prompts;
use h3_llm::runner::{output_token_limit, runner_kind};
use image::DynamicImage;
use serde_json::{json, Value};

use super::images::{combine_flux_ingredients, combine_story_batch, image_from_data_url, image_from_payload, open_rgb};
use super::repair::repair_and_validate;
use super::{first_text, flag, number, object, run_fields, text, whole, Llm, TextCall, VisionCall};
use crate::files::resolve_existing_file;
use crate::instructions::effective_instruction;

type Reply = Result<Value, String>;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn sub(pattern: &str, text: &str, with: &str) -> String {
    rx(pattern).replace_all(text, fancy_regex::NoExpand(with)).into_owned()
}

fn sub_once(pattern: &str, text: &str, with: &str) -> String {
    rx(pattern).replace(text, fancy_regex::NoExpand(with)).into_owned()
}

/// `_read_text_file`: the trimmed text of a file the request names, "" when it names none.
fn read_text_file(payload: &Obj, key: &str, label: &str) -> Result<String, String> {
    let raw = text_or_empty(payload.get(key));
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    let path = resolve_existing_file(&raw, label)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').trim().to_string())
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `int(float(reference_context.get("subject_reference_count") or 1)) if has_subject_reference else 0`, kept 0 to 99.
fn subject_reference_count(context: &Obj, has_subject_reference: bool) -> i64 {
    if !has_subject_reference {
        return 0;
    }
    let count = context.get("subject_reference_count").filter(|v| truthy(v)).map_or(Some(1.0), to_float).map_or(1, |f| f as i64);
    count.clamp(0, 99)
}

fn character_phrase(count: i64) -> &'static str {
    if count > 1 { "character reference images" } else { "character reference image" }
}

fn ctx_text(context: &Obj, key: &str) -> String {
    text_or_empty(context.get(key)).trim().to_string()
}

/// A line of "Reference Builder priorities" shared by the Flux/Klein prompts.
fn reference_priorities(subject_line: Option<String>, location_line: Option<String>) -> String {
    let rules: Vec<String> = [subject_line, location_line].into_iter().flatten().collect();
    if rules.is_empty() {
        return String::new();
    }
    format!("\nReference Builder priorities:\n{}\n- Use the user's notes/concept for action, pose, mood, lighting, story beat, and details after respecting the reference priorities.\n", rules.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n"))
}

fn location_details(name: &str, description: &str) -> String {
    [name, description].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("; ")
}

/// The list of picture entries of a request: a list, JSON text of one, or one path per line.
fn ingredients(payload: &Obj, key: &str, list_error: &str) -> Result<Vec<Value>, String> {
    let raw = payload.get(key).filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([]));
    let value = match raw {
        Value::String(text) => match h3_core::pyjson::parse(&text) {
            Ok(parsed) => parsed,
            Err(_) => Value::Array(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| json!({"path": l})).collect()),
        },
        other => other,
    };
    match value {
        Value::Array(items) => Ok(items),
        _ => Err(list_error.to_string()),
    }
}

/// The pictures of a list of entries (`{"path"}`, `{"data"}` or a bare path).
fn load_ingredients(items: &[Value], label: &str) -> Result<Vec<DynamicImage>, String> {
    let mut images = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let item = match item {
            Value::String(path) => json!({"path": path}),
            other => other.clone(),
        };
        let Some(map) = item.as_object() else { continue };
        images.push(image_from_payload(&text_or_empty(map.get("path")), &text_or_empty(map.get("data")), &format!("{label} {}", index + 1))?);
    }
    Ok(images)
}

pub(super) fn vision_runner(payload: &Obj) -> &'static str {
    match runner_kind(payload) {
        "llm_api" => "llm_api_vision",
        "own_server" => "own_server_vision",
        "lm_studio" => "lm_studio_vision",
        _ => "builtin",
    }
}

pub(super) fn is_external(payload: &Obj) -> bool {
    matches!(runner_kind(payload), "llm_api" | "own_server" | "lm_studio")
}

fn reply_info(prompt: String, info: &Obj, used_reference_image: Option<bool>) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("prompt".into(), json!(prompt));
    if let Some(used) = used_reference_image {
        map.insert("used_reference_image".into(), json!(used));
    }
    map.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    map.insert("used_mmproj".into(), json!(""));
    map.extend(run_fields(info).into_iter().filter(|(k, _)| k != "used_model"));
    Value::Object(map)
}

// -- text to image --------------------------------------------------------------------------------------------------------

/// `POST generate_t2i`: an image prompt from scene notes, or from a reference picture.
pub async fn generate_t2i(llm: &dyn Llm, payload: &Obj) -> Reply {
    let ref_image_path = text(payload, "ref_image_path").trim_matches('"').to_string();
    let ref_image_data = text(payload, "ref_image_data");
    let mut user_notes = text(payload, "user_notes");
    let prompt_mode = text(payload, "prompt_mode").to_lowercase();
    let instruction_key = first_text(payload, &["builder_instruction_key", "instruction_key"]);
    let context = object(payload, "reference_context");
    let theme = read_text_file(payload, "theme_style_path", "Theme/style file")?;
    let story = read_text_file(payload, "story_idea_path", "Story idea file")?;
    let subject_scene = read_text_file(payload, "subject_scene_path", "Subject/scene file")?;
    let mut parts = Vec::new();
    if !subject_scene.is_empty() {
        parts.push(format!("Subject/scene:\n{subject_scene}"));
    }
    if !theme.is_empty() {
        parts.push(format!("Theme/style:\n{theme}"));
    }
    if !story.is_empty() {
        parts.push(format!("Story idea:\n{story}"));
    }
    if !parts.is_empty() {
        if !user_notes.is_empty() {
            parts.push(format!("Segment notes:\n{user_notes}"));
        }
        user_notes = parts.join("\n\n");
    }
    let use_vision = flag(payload, "use_vision");
    let has_ref_image = use_vision && ((!ref_image_path.is_empty() && Path::new(&ref_image_path).is_file()) || !ref_image_data.is_empty());
    if use_vision && !has_ref_image {
        return Err("Choose a valid reference image path/data or turn off vision reference.".into());
    }
    if !has_ref_image && user_notes.is_empty() {
        return Err("Enter scene notes or provide a reference image.".into());
    }
    let image = if has_ref_image {
        Some(if !ref_image_data.is_empty() { image_from_data_url(&ref_image_data)? } else { open_rgb(Path::new(&ref_image_path))? })
    } else {
        None
    };
    let prompt = if has_ref_image {
        format!("{}\n\nUser notes:\n{}", prompts::text("image", "_VISUAL_T2I_INSTRUCTIONS"), if user_notes.is_empty() { "Use the reference image as the guide." } else { &user_notes })
    } else if prompt_mode == "flux_klein" {
        let (subject, location_name, location_description) = (ctx_text(&context, "subject_description"), ctx_text(&context, "location_name"), ctx_text(&context, "location_description"));
        let subject_line = (!subject.is_empty()).then(|| format!("Use the subject description for the main subject identity, face/body details, outfit, and visible character consistency. Subject description: {subject}"));
        let location_line = (!location_name.is_empty() || !location_description.is_empty()).then(|| format!("Use the mapped location as the required setting/background. Do not replace it with a different location from the concept or notes. Mapped location: {}", location_details(&location_name, &location_description)));
        let reference_text = reference_priorities(subject_line, location_line);
        let default = prompts::text("image", "_FLUX_KLEIN_T2I_INSTRUCTIONS");
        let base = if instruction_key.is_empty() { default.to_string() } else { effective_instruction(payload, &instruction_key, default) };
        format!("{base}\n{reference_text}\n\nUser notes:\n{}", if user_notes.is_empty() { "Create a cinematic image using the available scene notes." } else { &user_notes })
    } else if prompt_mode == "nano_banana" {
        let (subject, location_name, location_description) = (ctx_text(&context, "subject_description"), ctx_text(&context, "location_name"), ctx_text(&context, "location_description"));
        let has_subject = context.get("has_subject_reference").is_some_and(truthy) || !subject.is_empty();
        let has_location = context.get("has_location_reference").is_some_and(truthy) || !location_name.is_empty() || !location_description.is_empty();
        let mut blocks = Vec::new();
        if !subject.is_empty() {
            blocks.push(format!("Character reference description:\n{subject}"));
        }
        if !location_name.is_empty() || !location_description.is_empty() {
            blocks.push(format!("Location reference description:\n{location_name}\n{location_description}").trim().to_string());
        }
        if !user_notes.is_empty() {
            blocks.push(format!("User input:\n{user_notes}"));
        }
        let mut start_rules = vec![
            "- Start with: Using the provided character reference and location reference, create...".to_string(),
            "- If only a character reference description is available, start with: Using the provided character reference, create...".to_string(),
            "- If only a location reference description is available, start with: Using the provided location reference, create...".to_string(),
        ];
        if !has_subject && !has_location {
            start_rules = vec!["- Start directly with the shot and subject; do not claim a provided reference exists.".to_string()];
        }
        let default = prompts::text("image", "_NANO_B_T2I_INSTRUCTIONS");
        let base = if instruction_key.is_empty() { default.to_string() } else { effective_instruction(payload, &instruction_key, default) };
        format!(
            "{base}\n\nReference wording rules:\n{}\n- Preserve the character identity from the character reference description: face, hair, outfit, makeup, and overall identity.\n- Preserve the location identity from the location reference description: environment, architecture, layout, atmosphere, and major visible setting details.\n\n{}",
            start_rules.join("\n"),
            if blocks.is_empty() { "User input: Create a cinematic image using the available scene notes.".to_string() } else { blocks.join("\n") }
        )
    } else {
        let default = prompts::text("image", "_TEXT_ONLY_T2I_INSTRUCTIONS");
        let base = if instruction_key.is_empty() { default.to_string() } else { effective_instruction(payload, &instruction_key, default) };
        format!("{base}\n\nUser notes:\n{user_notes}")
    };
    let temperature = number(payload, "temperature", if has_ref_image { 0.25 } else { 0.6 })?;
    let top_p = number(payload, "top_p", 0.95)?;
    let requested = whole(payload, "max_new_tokens", if has_ref_image { 1000 } else { 1200 })?;
    let max_new_tokens = output_token_limit(payload, requested);
    let (raw, info) = match image {
        Some(image) => llm.vision(VisionCall { instruction: prompt, images: vec![image], temperature, top_p, max_new_tokens }).await?,
        None => llm.text(TextCall::new(prompt, temperature, top_p, max_new_tokens, "Gemma")).await?,
    };
    let cleaned = clean_visual_text(&raw);
    let extracted = extract_prompt_text(&cleaned, payload.get("scene_number"));
    let label = match prompt_mode.as_str() {
        "flux_klein" => "Flux/Klein",
        "nano_banana" => "NanoBanana",
        "flow_gpt" => "Flow/GPT",
        _ => "T2I",
    };
    let finished = repair_and_validate(llm, payload, &extracted, label).await?;
    Ok(reply_info(finished, &info, Some(has_ref_image)))
}

// -- editing a prompt -----------------------------------------------------------------------------------------------------

/// `_image_prompt_edit_instructions`.
pub fn edit_instruction(payload: &Obj) -> Result<String, String> {
    let current = text(payload, "current_prompt");
    let request = text(payload, "edit_request");
    let mode_label = Some(text(payload, "mode_label")).filter(|m| !m.is_empty()).unwrap_or_else(|| "image".into());
    let prompt_mode = text(payload, "prompt_mode").to_lowercase();
    let use_full = flag(payload, "use_full_scene_context");
    let use_vision = flag(payload, "use_vision_reference");
    let scene = object(payload, "scene_context");
    let context = object(payload, "reference_context");
    if current.is_empty() {
        return Err("Current image prompt is empty.".into());
    }
    if request.is_empty() {
        return Err("Edit request is empty.".into());
    }
    let has_subject = context.get("has_subject_reference").is_some_and(truthy);
    let has_location = context.get("has_location_reference").is_some_and(truthy);
    let phrase = character_phrase(subject_reference_count(&context, has_subject));
    let browser_mode = matches!(prompt_mode.as_str(), "nano_banana" | "flow_gpt");
    let opening = if !browser_mode {
        String::new()
    } else if has_subject && has_location {
        format!("Using the provided {phrase} and location reference image")
    } else if has_subject {
        format!("Using the provided {phrase}")
    } else if has_location {
        "Using the provided location reference image".to_string()
    } else {
        String::new()
    };
    let mode_rules = if browser_mode {
        let mut rules = format!(
            "This is a {} prompt. Preserve any required wording about provided character or location reference images unless the user explicitly asks to change reference usage. Use advanced Krea 2-style image prompting: concrete subject identity, wardrobe, hair, makeup, pose, camera framing, lens feel, lighting setup, environment, materials, atmosphere, color palette, texture, and cinematic finish. Keep it practical as one image-generation prompt paragraph.",
            if prompt_mode == "flow_gpt" { "Flow/GPT browser image" } else { "NanoBanana" }
        );
        if !opening.is_empty() {
            rules.push_str(&format!(" The revised prompt must start with exactly this reference opening before the rest of the prompt: \"{opening}, create\"."));
        }
        rules
    } else if prompt_mode == "flux_klein" {
        "This is a Flux/Klein image prompt. Preserve mapped subject and location identity from the prompt/reference context unless the user explicitly asks to change them. Do not mention image indexes or internal reference labels.".to_string()
    } else if prompt_mode == "krea2_2pass" {
        "This is a Krea 2 image prompt. Keep enough concrete detail for pose, wardrobe, lighting, camera framing, environment, materials, and atmosphere.".to_string()
    } else {
        "This is a text-to-image prompt. Keep it visually specific and usable directly by an image generation model.".to_string()
    };
    let context_text = if use_full {
        let line = |key: &str| Some(text_or_empty(scene.get(key)).trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "(none)".into());
        format!(
            "\n\nFull scene context is enabled. Use this context only when it helps satisfy the requested edit. Do not rewrite unrelated parts just because context is present.\nScene label: {}\nScene notes: {}\nDirector note: {}\nLyric section: {}\nLyric text: {}\nSubject context: {}\nLocation context: {}\nNo character present: {}",
            line("label"),
            line("scene_notes"),
            line("director_note"),
            line("lyric_section"),
            line("lyric_text"),
            line("subject_context"),
            line("location_context"),
            if scene.get("no_character_present").is_some_and(truthy) { "True" } else { "False" }
        )
    } else {
        String::new()
    };
    let mut bits = Vec::new();
    for (label, key) in [("Reference subject description", "subject_description"), ("Reference location name", "location_name"), ("Reference location description", "location_description")] {
        let value = ctx_text(&context, key);
        if !value.is_empty() {
            bits.push(format!("{label}: {value}"));
        }
    }
    let reference_text = if bits.is_empty() { String::new() } else { format!("\n\nReference Builder context:\n{}", bits.join("\n")) };
    Ok(format!(
        "You are editing an existing {mode_label} image generation prompt.\nMake the smallest useful edit that satisfies the user's request.\nPreserve the subject, setting, identity, style, lighting, wardrobe, camera framing, mood, and continuity unless the user explicitly asks to change them.\nIf a reference image is provided, use it only as visual reference for subject, setting, composition, color, and concrete visible details.\nWhen full scene context is disabled, use only the current prompt and user requested change.\nWhen full scene context is enabled, you may use the supplied scene context for a larger but still controlled rewrite.\n{mode_rules}\nDo not add captions, text overlays, markdown, labels, bullet points, explanations, or commentary.\nDo not mention this edit request. Do not describe what changed.\nReturn only one clean revised image prompt paragraph.\n\nCurrent prompt:\n{current}\n\nUser requested change:\n{request}\n\nReference image provided: {}{context_text}{reference_text}",
        if use_vision { "True" } else { "False" }
    ))
}

/// `_ensure_reference_opening_for_image_edit`: NanoBanana and Flow/GPT prompts start by naming the pictures they use.
fn ensure_reference_opening(prompt: &str, payload: &Obj) -> String {
    let text = prompt.trim().to_string();
    let prompt_mode = text_or_empty(payload.get("prompt_mode")).trim().to_lowercase();
    if !matches!(prompt_mode.as_str(), "nano_banana" | "flow_gpt") || text.is_empty() {
        return text;
    }
    let context = object(payload, "reference_context");
    let has_subject = context.get("has_subject_reference").is_some_and(truthy);
    let has_location = context.get("has_location_reference").is_some_and(truthy);
    let phrase = character_phrase(subject_reference_count(&context, has_subject));
    if !has_subject && !has_location {
        return text;
    }
    let opening = if has_subject && has_location {
        format!("Using the provided {phrase} and location reference image")
    } else if has_subject {
        format!("Using the provided {phrase}")
    } else {
        "Using the provided location reference image".to_string()
    };
    let mut text = sub_once(r"(?i)^Using the provided\s+(?:(?:character|location|scene|reference)\s+)+(?:images?|references?)\s*,?\s*(?:create\s+)?", &text, "").trim().to_string();
    if rx(r"(?i)^(?:create|make|generate)\b").is_match(&text).unwrap_or(false) {
        text = sub_once(r"(?i)^(?:create|make|generate)\b\s*", &text, "").trim().to_string();
    }
    if text.is_empty() {
        return format!("{opening}, create a cinematic still image.");
    }
    format!("{opening}, create {}", lower_first(&text)).trim().to_string()
}

/// `POST edit_image_prompt`.
pub async fn edit_image_prompt(llm: &dyn Llm, payload: &Obj) -> Reply {
    if text(payload, "current_prompt").is_empty() {
        return Err("Current image prompt is empty.".into());
    }
    let ref_path = first_text(payload, &["ref_image_path", "image_reference_path"]);
    let ref_path = ref_path.trim_matches('"').to_string();
    let ref_data = first_text(payload, &["ref_image_data", "image_reference_data"]);
    let use_vision = flag(payload, "use_vision_reference");
    let instruction = edit_instruction(payload)?;
    let image = if use_vision {
        if !ref_data.is_empty() {
            Some(image_from_data_url(&ref_data)?)
        } else if !ref_path.is_empty() {
            Some(open_rgb(&resolve_existing_file(&ref_path, "Image prompt edit reference image")?)?)
        } else {
            return Err("Prompt edit requested a reference image, but no image reference was provided.".into());
        }
    } else {
        None
    };
    let temperature = number(payload, "temperature", 0.25)?;
    let top_p = number(payload, "top_p", 0.9)?;
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 1200)?);
    let (raw, info) = match image {
        Some(image) => llm.vision(VisionCall { instruction, images: vec![image], temperature, top_p, max_new_tokens }).await?,
        None => llm.text(TextCall::new(instruction, temperature, top_p, max_new_tokens, "image prompt edit")).await?,
    };
    let cleaned = clean_visual_text(&raw);
    let extracted = extract_prompt_text(&cleaned, payload.get("scene_number"));
    let label = Some(text(payload, "mode_label")).filter(|m| !m.is_empty()).unwrap_or_else(|| "Image".into());
    let validated = repair_and_validate(llm, payload, &extracted, &label).await?;
    let prompt = ensure_reference_opening(&validated, payload);
    let mut map = serde_json::Map::new();
    map.insert("prompt".into(), json!(prompt));
    map.insert("runner".into(), info.get("runner").cloned().unwrap_or_else(|| json!("builtin")));
    map.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    map.insert("used_mmproj".into(), json!(""));
    map.insert("used_image_reference".into(), json!(use_vision));
    map.insert("unloaded".into(), info.get("unloaded").cloned().unwrap_or_else(|| json!(false)));
    Ok(Value::Object(map))
}

// -- Flux/Klein and NanoBanana from reference pictures ----------------------------------------------------------------------

/// `POST generate_flux_klein_prompt`.
pub async fn generate_flux_klein_prompt(llm: &dyn Llm, payload: &Obj) -> Reply {
    let user_notes = text(payload, "user_notes");
    let key = Some(first_text(payload, &["builder_instruction_key", "instruction_key"])).filter(|k| !k.is_empty()).unwrap_or_else(|| "flux_klein_t2i".into());
    let items = ingredients(payload, "image_ingredients", "Image ingredients must be a list.")?;
    let context = object(payload, "reference_context");
    let has_subject = context.get("has_subject_reference").is_some_and(truthy);
    let has_location = context.get("has_location_reference").is_some_and(truthy);
    let (subject, location_name, location_description) = (ctx_text(&context, "subject_description"), ctx_text(&context, "location_name"), ctx_text(&context, "location_description"));
    let images = load_ingredients(&items, "Image ingredient")?;
    let combined = combine_flux_ingredients(&images)?;
    let subject_line = has_subject.then(|| {
        let mut line = "Use the subject reference for the main subject identity, face/body details, outfit, and visible character consistency.".to_string();
        if !subject.is_empty() {
            line.push_str(&format!(" Subject description: {subject}"));
        }
        line
    });
    let location_line = has_location.then(|| {
        let mut line = "Use the mapped location reference as the required setting/background. Do not replace it with a different location from the concept or notes.".to_string();
        let details = location_details(&location_name, &location_description);
        if !details.is_empty() {
            line.push_str(&format!(" Mapped location: {details}"));
        }
        line
    });
    let reference_text = reference_priorities(subject_line, location_line);
    let base = effective_instruction(payload, &key, prompts::text("image", "_FLUX_KLEIN_T2I_INSTRUCTIONS"));
    let instruction = format!(
        "{base}\n\nThe image input contains the available reference images/visual ingredients. These may include a character, background, props, style references, or other visual ingredients.\n{reference_text}\nUser input:\n{}",
        if user_notes.is_empty() { "Create a new image using the available reference images." } else { &user_notes }
    );
    let temperature = number(payload, "temperature", 0.25)?;
    let top_p = number(payload, "top_p", 0.95)?;
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 350)?);
    let (raw, info) = llm.vision(VisionCall { instruction, images: vec![combined], temperature, top_p, max_new_tokens }).await?;
    let cleaned = clean_visual_text(&raw);
    let prompt = repair_and_validate(llm, payload, &cleaned, "Flux/Klein").await?;
    let external = is_external(payload);
    Ok(json!({
        "prompt": prompt,
        "used_model": info.get("used_model").cloned().unwrap_or_else(|| json!("")),
        "used_mmproj": "",
        "runner": vision_runner(payload),
        "unloaded": if external { json!(false) } else { info.get("unloaded").cloned().unwrap_or_else(|| json!(false)) },
    }))
}

/// `_cleanup_nb_reference_claims`: wording that names references the request does not have is taken out.
fn cleanup_claims(text: &str, has_subject: bool, has_location: bool, has_images: bool, phrase: &str) -> String {
    let fallback = if has_images { "Using the provided reference image" } else { "Create" };
    let mut text = text.to_string();
    let both = r"(?i)\bUsing the provided character reference and location reference\b";
    let both_reversed = r"(?i)\bUsing the provided location reference and character reference\b";
    if !has_subject && !has_location {
        text = sub(both, &text, fallback);
        text = sub(both_reversed, &text, fallback);
        text = sub(r"(?i)\bUsing the provided (?:character|location|scene) reference\b", &text, fallback);
    } else if !has_location {
        let only_subject = format!("Using the provided {phrase}");
        text = sub(both, &text, &only_subject);
        text = sub(both_reversed, &text, &only_subject);
        text = sub(r"(?i)\s+and (?:the\s+)?(?:provided\s+)?(?:location|scene) reference(?: image)?\b", &text, "");
    } else if !has_subject {
        text = sub(both, &text, "Using the provided location reference");
        text = sub(both_reversed, &text, "Using the provided location reference");
        text = sub(r"(?i)\s+and (?:the\s+)?(?:provided\s+)?character reference(?: image)?\b", &text, "");
        text = sub(r"(?i)\bcharacter reference(?: image)?\s+and\s+", &text, "");
    }
    text = sub(r"\s+,", &text, ",");
    text = sub(r"\s{2,}", &text, " ");
    text.trim().to_string()
}

/// `_ensure_nb_reference_opening`.
fn ensure_nb_opening(text: &str, has_subject: bool, has_location: bool, phrase: &str) -> String {
    let text = text.trim().to_string();
    if text.is_empty() {
        return text;
    }
    let opening = if has_subject && has_location {
        format!("Using the provided {phrase} and location reference image")
    } else if has_subject {
        format!("Using the provided {phrase}")
    } else if has_location {
        "Using the provided location reference image".to_string()
    } else {
        return text;
    };
    if rx(r"(?i)\bUsing the provided (?:character|location|scene|reference image)").is_match(&text).unwrap_or(false) {
        return text;
    }
    let text = if rx(r"(?i)^(?:create|make|generate)\b").is_match(&text).unwrap_or(false) { sub_once(r"(?i)^(?:create|make|generate)\b\s*", &text, "") } else { text };
    format!("{opening}, create {}", lower_first(&text)).trim().to_string()
}

/// `POST generate_nb_image_prompt`: a NanoBanana (or Flow/GPT) prompt that names the references it is given.
pub async fn generate_nb_image_prompt(llm: &dyn Llm, payload: &Obj) -> Reply {
    let user_notes = text(payload, "user_notes");
    let mode = Some(text(payload, "prompt_mode").to_lowercase()).filter(|m| !m.is_empty()).unwrap_or_else(|| "nano_banana".into());
    let is_flow = mode == "flow_gpt";
    let (default_key, default_text, label) = if is_flow { ("flow_gpt_t2i", prompts::text("image", "_FLOW_GPT_T2I_INSTRUCTIONS"), "Flow/GPT") } else { ("nano_b_t2i", prompts::text("image", "_NANO_B_T2I_INSTRUCTIONS"), "NanoBanana") };
    let key = Some(first_text(payload, &["builder_instruction_key", "instruction_key"])).filter(|k| !k.is_empty()).unwrap_or_else(|| default_key.into());
    let items = ingredients(payload, "image_ingredients", &format!("{label} reference images must be a list."))?;
    let images = load_ingredients(&items, &format!("{label} reference image"))?;
    let context = object(payload, "reference_context");
    let (subject, location_name, location_description) = (ctx_text(&context, "subject_description"), ctx_text(&context, "location_name"), ctx_text(&context, "location_description"));
    let has_subject = context.get("has_subject_reference").is_some_and(truthy);
    let has_location = context.get("has_location_reference").is_some_and(truthy);
    let count = subject_reference_count(&context, has_subject);
    let phrase = character_phrase(count);
    let identity_phrase = if count > 1 { "character identities from the character references" } else { "character identity from the character reference" };
    let mut parts = Vec::new();
    if !subject.is_empty() {
        parts.push(format!("Subject description:\n{subject}"));
    }
    if !location_name.is_empty() || !location_description.is_empty() {
        parts.push(format!("Location reference:\n{location_name}\n{location_description}").trim().to_string());
    }
    if !user_notes.is_empty() {
        parts.push(format!("User input:\n{user_notes}"));
    }
    let mut flags = Vec::new();
    if has_subject {
        let n = if count == 0 { 1 } else { count };
        flags.push(format!("{n} character reference image{} available.", if n != 1 { "s are" } else { " is" }));
    }
    if has_location {
        flags.push("A scene/location reference image is available.".to_string());
    }
    if !flags.is_empty() {
        parts.push(flags.join("\n"));
    }
    let has_images = !images.is_empty();
    let unmapped = has_images && !has_subject && !has_location;
    let (rules, example) = if has_subject && has_location {
        (
            format!("- Start by mentioning both the provided {phrase} and location reference image.\n- Preserve the {identity_phrase}: face, hair, outfit, makeup, and overall identity.\n- Preserve the location identity from the location reference: environment, architecture, layout, atmosphere, and major visible setting details.\n- Use the user's scene/concept notes for action, pose, camera, mood, and story beat.\n- Do not paste the character into the location image.\n- Do not copy the exact pose, crop, camera angle, perspective, or composition from either reference.\n"),
            format!(
                "Using the provided {phrase} and location reference image, create a close-up profile shot of the woman in the misty forest. Preserve {} while using the forest reference for the white fibrous trees, mist, and eerie atmosphere. Use a new pose, new camera angle, soft bokeh, atmospheric haze, and dramatic rim lighting.",
                if count > 1 { "the subjects identities, hair, outfits, makeup, and overall identity details from the character references" } else { "her identity, hair, outfit, and crown from the character reference" }
            ),
        )
    } else if has_subject {
        (
            format!(
                "- Start by mentioning only the provided {phrase}. Do not mention a location reference image.\n- Preserve the {identity_phrase}: face, hair, outfit, makeup, and overall identity.\n- Create the setting, background, atmosphere, and location from the user's scene/concept notes.\n- Do not copy the {}.\n",
                if count > 1 { "character reference poses, studio backgrounds, crops, camera angles, or lens distances" } else { "character reference pose, studio background, crop, camera angle, or lens distance" }
            ),
            format!(
                "Using the provided {phrase}, create an intimate upper body shot of the woman in a misty white forest built from the scene concept. Preserve {} while placing the scene among pale gnarled trees, soft fog, shallow depth of field, and ethereal rim lighting.",
                if count > 1 { "the subjects identities, hair, outfits, and facial details" } else { "her identity, blonde hair, lace outfit, and delicate facial details" }
            ),
        )
    } else if has_location {
        (
            "- Start by mentioning only the provided location reference. Do not mention a character reference.\n- Use the location reference for environment, architecture, layout, atmosphere, and major visible setting details.\n- Create any subject, pose, outfit, camera, and story details from the user's scene/concept notes.\n- Do not copy the exact location reference camera angle, framing, perspective, or composition.\n".to_string(),
            "Using the provided location reference, create a cinematic medium shot of the scene's subject moving through the misty forest. Preserve the pale fibrous trees, narrow fog-covered path, and eerie atmosphere from the location reference while creating a new camera angle, new subject pose, soft bokeh, and dramatic haze.".to_string(),
        )
    } else if unmapped {
        (
            "- You may use the provided reference image or images only as loose visual guidance.\n- Do not call them character references or location references unless the context explicitly says that.\n- Use the user's scene/concept notes as the main source of subject, setting, action, pose, and mood.\n- Create a new camera angle, new pose, and new composition.\n".to_string(),
            "Create a cinematic medium close-up from the scene concept, using the provided visual reference only as loose style guidance. Build the subject, setting, lighting, and atmosphere from the notes, with soft bokeh, layered haze, a new composition, and high cinematic detail.".to_string(),
        )
    } else {
        (
            "- Do not mention provided references or reference images.\n- Treat this as normal text-to-image prompt writing from the user's scene/concept notes.\n- Create the subject, setting, action, pose, camera, lighting, and atmosphere from the notes.\n".to_string(),
            "Create a cinematic medium close-up in a vast cosmic void filled with drifting pearlescent particles and soft ethereal light. Use a new composition, shallow depth of field, gentle atmospheric haze, luminous highlights, and a quiet dreamlike mood.".to_string(),
        )
    };
    let base = effective_instruction(payload, &key, default_text);
    let instruction = format!(
        "{base}\n\nReference wording rules:\n{rules}\nGood output example:\n{example}\n\n{}",
        if parts.is_empty() { "User input: Create a new image from the notes.".to_string() } else { parts.join("\n") }
    );
    let temperature = number(payload, "temperature", 0.25)?;
    let top_p = number(payload, "top_p", 0.95)?;
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 900)?);
    let empty = || format!("{label} Gemma returned an empty prompt.");
    if !has_images {
        let (raw, info) = llm.text(TextCall::new(instruction, temperature, top_p, max_new_tokens, &format!("{label} text Gemma"))).await?;
        let cleaned = clean_plain_text(&raw);
        if cleaned.is_empty() {
            return Err(empty());
        }
        let cleaned = cleanup_claims(&cleaned, has_subject, has_location, has_images, phrase);
        let opened = ensure_nb_opening(&cleaned, has_subject, has_location, phrase);
        let finished = repair_and_validate(llm, payload, &opened, label).await?;
        let mut map = serde_json::Map::new();
        map.insert("prompt".into(), json!(finished));
        map.extend(info);
        return Ok(Value::Object(map));
    }
    let combined = combine_flux_ingredients(&images)?;
    let (raw, info) = llm.vision(VisionCall { instruction, images: vec![combined], temperature, top_p, max_new_tokens }).await?;
    let mut cleaned = clean_plain_text(&raw);
    cleaned = sub(r"(?im)^CAMERA\s+COMPOSI+TION\s*\(PRIORITY\)", &cleaned, "CAMERA COMPOSITION (PRIORITY)");
    cleaned = sub(r"(?im)^CHARACTER\s+REFEREN[CC]E\b", &cleaned, "CHARACTER REFERENCE");
    cleaned = sub(r"(?im)^SCE+NE\s+REFEREN[CC]E\b", &cleaned, "SCENE REFERENCE");
    cleaned = sub(r"(?i)\breference\s+imagae\b", &cleaned, "reference image");
    cleaned = sub(r"(?i)\breference\s+imagaes\b", &cleaned, "reference images");
    cleaned = cleanup_claims(&cleaned, has_subject, has_location, has_images, phrase);
    cleaned = ensure_nb_opening(&cleaned, has_subject, has_location, phrase);
    if cleaned.is_empty() {
        return Err(empty());
    }
    let finished = repair_and_validate(llm, payload, &cleaned, label).await?;
    let mut map = serde_json::Map::new();
    map.insert("prompt".into(), json!(finished));
    map.extend(info);
    Ok(Value::Object(map))
}

// -- notes about Story Builder pictures ------------------------------------------------------------------------------------

/// `POST analyze_story_references`: planning notes about the Story Builder's reference pictures, four to a sheet.
pub async fn analyze_story_references(llm: &dyn Llm, payload: &Obj) -> Reply {
    let user_notes = text(payload, "user_notes");
    let items = ingredients(payload, "image_ingredients", "Story reference images must be a list.")?;
    let images = load_ingredients(&items, "Story reference image")?;
    if images.is_empty() {
        return Err("Add at least one Story Builder reference image first.".into());
    }
    let batches: Vec<&[DynamicImage]> = images.chunks(4).collect();
    let temperature = number(payload, "temperature", 0.25)?;
    let top_p = number(payload, "top_p", 0.95)?;
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 500)?);
    let mut used_model = String::new();
    let mut notes = Vec::new();
    for (index, batch) in batches.iter().enumerate() {
        let combined = combine_story_batch(batch, 512)?;
        let instruction = format!(
            "Analyze these reference images for a music video Story Builder. Each image has been resized into a 512px tile; this batch contains at most four images. Write compact reusable planning notes for a text-only agent. Identify likely singers/characters, clothing, faces/hair/body details, location ideas, props, color palette, lighting, mood, genre, and overall aesthetic. Do not write an image generation prompt. Do not mention image grids or panels. Use short labeled lines. Keep it under 180 words.\n\nBatch {} of {}.\nUser notes:\n{}",
            index + 1,
            batches.len(),
            if user_notes.is_empty() { "Summarize the characters, locations, style, and aesthetic shown in the images." } else { &user_notes }
        );
        let (raw, info) = llm.vision(VisionCall { instruction, images: vec![combined], temperature, top_p, max_new_tokens }).await?;
        if let Some(model) = info.get("used_model").and_then(Value::as_str) {
            used_model = model.to_string();
        }
        let cleaned = clean_visual_text(&raw);
        if looks_like_repeat_failure(&cleaned) {
            return Err(format!("Gemma returned repeated/thought junk instead of usable Story reference notes for batch {}.", index + 1));
        }
        if !cleaned.trim().is_empty() {
            let prefix = if batches.len() > 1 { format!("Reference batch {}: ", index + 1) } else { String::new() };
            notes.push(format!("{prefix}{}", cleaned.trim()));
        }
    }
    let external = is_external(payload);
    Ok(json!({
        "notes": notes.join("\n\n").trim(),
        "used_model": used_model,
        "used_mmproj": "",
        "runner": vision_runner(payload),
        "unloaded": !external,
        "batches": batches.len(),
    }))
}
