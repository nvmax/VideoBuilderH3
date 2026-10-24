//! Describing a reference picture (a character, a location, a prop) with the LLM. Port of
//! `_generate_builder_reference_description` in `llm/image_prompt_generation.py`.

use fancy_regex::Regex;

use crate::checks::clean_visual_text;
use crate::client::{chat, ChatRequest, LlmError};
use crate::config::LlmConfig;
use crate::prompt_text::validate_reference_description;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const OBJECT_TYPES: [&str; 9] = ["prop", "object", "vehicle", "creature", "animal", "outfit", "style", "environment", "other"];

/// What to ask for a kind of reference. Unknown kinds are asked for as a character.
pub fn normalize_reference_type(kind: &str) -> String {
    let kind = kind.trim().to_lowercase();
    if matches!(kind.as_str(), "subject" | "character" | "face" | "location" | "extra") || OBJECT_TYPES.contains(&kind.as_str()) { kind } else { "subject".into() }
}

/// The instruction for a picture of `reference_type`. `name` labels the reference, `count` and `style` are for extras.
pub fn instruction(reference_type: &str, name: &str, count: u32, style: &str) -> String {
    let subject_label = {
        let label = collapse(name);
        if label.to_lowercase().starts_with("the ") { label.to_lowercase() } else { label }
    };
    match reference_type {
        "face" => concat!(
            "Study the visible person's face and write one precise face-identity description for an image enhancement prompt.\n",
            "Return only one plain-text paragraph. It must begin exactly with: a photo of\n",
            "Describe stable facial identity details only: apparent age range, face shape, eyes, eyebrows, nose, lips, jawline, cheeks, distinctive facial features, hair color, hairline, and face-framing hair.\n",
            "Include makeup or facial accessories only when clearly visible and useful for preserving identity.\n",
            "Do not mention the background, location, clothing, body, pose, camera angle, action, mood, or facial expression.\n",
            "Do not describe smiling, frowning, an open mouth, gaze direction, or head direction.\n",
            "Do not add markdown, a label, quotation marks, instructions, or commentary.\n",
            "Do not invent details that are not visible. Keep it under 100 words."
        )
        .to_string(),
        "subject" | "character" => {
            let mut text = concat!(
                "Look at the image and write one concise character appearance description.\n",
                "Output only the description, one paragraph, no markdown, no label, no bullet points.\n",
                "Describe only the character's full visible appearance: hair, makeup, accessories, jewelry, clothing, outfit materials, colors, shoes, and distinctive visual identity details.\n",
                "Do not mention skin color, skin tone, ethnicity, race, or complexion.\n",
                "Do not describe the background, location, pose, camera angle, facial expression, mood, action, or what the character is doing.\n",
                "Do not invent hidden or unseen details.\n",
                "Keep it under 100 words."
            )
            .to_string();
            if !subject_label.is_empty() {
                text.push_str(&format!("\nRefer to the subject as {subject_label}. Do not call them the character or the subject in the final description."));
            }
            text
        }
        "extra" => {
            let mut group = format!("one of a group of {} background performers", count.clamp(1, 100));
            let style = collapse(style);
            if !style.is_empty() {
                group.push_str(&format!(" in a {style}-appropriate wardrobe"));
            }
            let mut text = format!(
                "{}{}{}{}{}{}{}{}{group}.",
                "Look at the image and write one very short background-performer identity description.\n",
                "Output only the description, one paragraph, no markdown, no label, no bullet points.\n",
                "Include apparent gender presentation, hair color and hairstyle, a concise outfit description with its main colors, and at most one clearly visible distinguishing trait when available, such as build, face shape, facial hair, or one prominent accessory.\n",
                "Use one complete grammatical sentence of no more than 35 words.\n",
                "Do not mention skin color, skin tone, ethnicity, race, or complexion.\n",
                "Do not describe the background, location, pose, camera angle, facial expression, mood, action, or what the performer is doing.\n",
                "Do not list minor jewelry, every accessory, fabric micro-details, or unseen details.\n",
                "Treat the pictured person as "
            );
            if !subject_label.is_empty() {
                text.push_str(&format!("\nUse this label only as the performer name if needed: {subject_label}"));
            }
            text
        }
        "location" => {
            let mut text = concat!(
                "Look at the image and write one concise location/environment description.\n",
                "Output only the description, one paragraph, no markdown, no label, no bullet points.\n",
                "Describe the place itself: environment, architecture, layout, major objects, materials, colors, lighting, atmosphere, and visible setting details.\n",
                "Do not describe a main character, pose, performance, camera angle, or story action.\n",
                "Do not invent hidden or unseen details.\n",
                "Keep it under 100 words."
            )
            .to_string();
            let hint = name.trim();
            if !hint.is_empty() {
                text.push_str(&format!("\nUse this label only as the location name if needed: {hint}"));
            }
            text
        }
        other => {
            let label = other.replace('_', " ");
            let mut text = format!(
                "Look at the image and write one concise {label} reference description.\n{}{}{}{}",
                "Output only the description, one paragraph, no markdown, no label, no bullet points.\n",
                "Describe only the visible reference item: shape, form, materials, colors, markings, texture, scale cues, construction, accessories, and distinctive visual identity details.\n",
                "Do not describe the background, location, pose, camera angle, facial expression, mood, action, story meaning, or a person unless the reference item itself is a person.\n",
                "Do not invent hidden or unseen details.\nKeep it under 100 words."
            );
            let hint = name.trim();
            if !hint.is_empty() {
                text.push_str(&format!("\nUse this label only as the reference name if needed: {hint}"));
            }
            text
        }
    }
}

/// The model's reply as the saved description, or why it cannot be used.
pub fn clean_reference_description(raw: &str, reference_type: &str, name: &str) -> Result<String, String> {
    let subject_label = {
        let label = collapse(name);
        if label.to_lowercase().starts_with("the ") { label.to_lowercase() } else { label }
    };
    let mut text = clean_visual_text(raw);
    text = rx(r"(?i)^\s*(character|subject|location|description)\s*:\s*").replace(&text, "").trim().to_string();
    if reference_type == "face" {
        text = text.trim().trim_matches('"').trim_matches('\'').trim().to_string();
        let marker = rx(r"(?i)a\s+photo\s+of");
        let last = marker.find_iter(&text).flatten().last().map(|m| m.start());
        match last {
            Some(start) => text = text[start..].to_string(),
            None => {
                text = rx(r"(?i)^\s*(?:face prompt|prompt|face description)\s*:\s*").replace(&text, "").into_owned();
                text = format!("a photo of {}", text.trim_start_matches([' ', ',', '.', '-']));
            }
        }
        text = rx(r"(?i)^a\s+photo\s+of\b").replace(&text, "a photo of").into_owned();
        text = collapse(&text);
        if marker.find_iter(&text).flatten().count() != 1 {
            return Err("The face description repeated its required opening phrase.".into());
        }
        if rx(r"(?i)\b(?:assistant|analysis|reasoning|let me|i think|i thought|my thought)\b").is_match(&text).unwrap_or(false) {
            return Err("The face description leaked model reasoning.".into());
        }
    } else if matches!(reference_type, "subject" | "character") {
        if !subject_label.is_empty() {
            for phrase in [r"(?i)\bthe character\b", r"(?i)\bthe subject\b", r"(?i)\ba character\b", r"(?i)\ba subject\b"] {
                text = rx(phrase).replace_all(&text, fancy_regex::NoExpand(&subject_label)).into_owned();
            }
        }
        text = rx(r"(?i)\b(?:with|has|having|featuring)?\s*(?:very\s+|pale\s+|fair\s+|light\s+|medium\s+|tan\s+|tanned\s+|olive\s+|brown\s+|dark\s+|deep\s+|warm\s+|cool\s+|golden\s+|porcelain\s+|dusky\s+|caramel\s+|bronze\s+|dark-skinned\s+|light-skinned\s+)+(?:skin|skin tone|complexion)\b,?\s*(?:and\s+)?")
            .replace_all(&text, "")
            .into_owned();
        text = rx(r"(?i)\b(?:skin|skin tone|complexion)\s*(?:is|appears|looks)\s+[^,.]+,?\s*(?:and\s+)?").replace_all(&text, "").into_owned();
        text = rx(r"\s+,").replace_all(&text, ",").into_owned();
        text = rx(r"(?i)(?:^|\.\s*)and\s+").replace_all(&text, "").trim().to_string();
        text = rx(r"\s{2,}").replace_all(&text, " ").trim_matches([' ', ',']).to_string();
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 100 {
        text = format!("{}.", words[..100].join(" ").trim_end_matches([' ', ',', '.', ';', ':']));
    }
    if text.is_empty() {
        return Err("Gemma returned an empty reference description.".into());
    }
    validate_reference_description(&text, reference_type)?;
    Ok(text)
}

/// Ask the LLM to describe a picture (`data_url`) and return the cleaned description. A second, calmer try follows an
/// unusable first answer.
pub async fn describe_reference(cfg: &LlmConfig, reference_type: &str, name: &str, data_url: &str, count: u32, style: &str) -> Result<(String, String), LlmError> {
    let reference_type = normalize_reference_type(reference_type);
    let base = instruction(&reference_type, name, count, style);
    let mut last = String::new();
    for attempt in 0..2 {
        let mut text = base.clone();
        if attempt > 0 {
            text.push_str("\n\nPrevious output was rejected because it repeated words or was not a usable description. Write a normal visual description with varied concrete nouns. Do not repeat any word more than twice.");
        }
        let (temperature, top_p) = if attempt > 0 { (0.05, 0.75) } else { (0.2, 0.9) };
        let reply = chat(cfg, &ChatRequest::text(&text, temperature, top_p, 180).with_images(vec![data_url.to_string()])).await?;
        match clean_reference_description(&reply.text, &reference_type, name) {
            Ok(description) => return Ok((description, reply.model)),
            Err(why) => last = why,
        }
    }
    Err(LlmError::Empty(last))
}
