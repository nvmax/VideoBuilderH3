//! The Thumbnail dialog's LLM button: look at a scene image and write the edit prompt that turns it into a YouTube-style
//! thumbnail. Port of `llm/thumbnail_prompt.py`. The instruction is the pack's text (`llm/prompts/thumbnail.py`, in
//! `h3-llm/data/prompt_constants.json`).

use fancy_regex::Regex;
use h3_core::pyval::{to_float, truthy, Obj};
use h3_llm::prompts;
use h3_llm::runner::output_token_limit;
use serde_json::{json, Value};

use super::images::image_from_payload;
use super::{text, Llm, VisionCall};

type Reply = Result<Value, String>;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `thumbnail_instruction`: the instruction with the rule for the title text filled in, or the no-text rule when there is no title.
pub fn thumbnail_instruction(title: &str) -> String {
    let title = collapse(title);
    let rule = if title.is_empty() {
        prompts::text("thumbnail", "THUMBNAIL_NO_TEXT_RULE").to_string()
    } else {
        prompts::text("thumbnail", "THUMBNAIL_TEXT_RULE").replace("{title}", &title.replace('"', "'"))
    };
    prompts::text("thumbnail", "THUMBNAIL_EDIT_PROMPT_INSTRUCTIONS").replace("{text_rule}", &rule)
}

/// `clean_thumbnail_prompt`: strip code fences, a leading label and wrapping quotes from the model's reply.
pub fn clean_thumbnail_prompt(raw: &str) -> String {
    let fenced = rx(r"^```[a-zA-Z]*\s*|\s*```$").replace_all(raw.trim(), "").trim().to_string();
    let labelled = rx(r"(?i)^(?:here(?:'s| is)[^:\n]*:|prompt\s*:|edit prompt\s*:)\s*").replace(&fenced, "").trim().to_string();
    let mut chars = labelled.chars();
    let unquoted = match (chars.next(), chars.next_back()) {
        (Some(first), Some(last)) if first == last && (first == '"' || first == '\'') && labelled.chars().count() >= 2 => chars.as_str().trim().to_string(),
        _ => labelled.clone(),
    };
    collapse(&unquoted)
}

/// The notes the instruction is followed by: the title, the style notes, the project and the scene's lyrics.
fn user_block(payload: &Obj) -> String {
    let lines: Vec<String> = [("THUMBNAIL TITLE", "title"), ("STYLE NOTES", "style_notes"), ("PROJECT", "project_name"), ("SCENE LYRICS", "scene_lyrics")]
        .iter()
        .filter_map(|(label, key)| {
            let value = collapse(&text(payload, key));
            (!value.is_empty()).then(|| format!("{label}: {}", value.chars().take(600).collect::<String>()))
        })
        .collect();
    if lines.is_empty() { "No extra notes. Pick a strong, fitting look from the image.".to_string() } else { lines.join("\n") }
}

/// `POST thumbnail/prompt`: look at the scene image (`image_path` or `image_data`) and write one edit prompt that makes it a thumbnail.
pub async fn generate_thumbnail_prompt(llm: &dyn Llm, payload: &Obj) -> Reply {
    let (path, data) = (text(payload, "image_path"), text(payload, "image_data"));
    if path.is_empty() && data.is_empty() {
        return Err("Choose a scene with an image first. The LLM needs the picture to write the thumbnail prompt.".into());
    }
    let image = image_from_payload(&path, &data, "Thumbnail source image")?;
    let instruction = format!("{}\n\n{}", thumbnail_instruction(&text(payload, "title")), user_block(payload));
    let temperature = payload.get("temperature").filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.75);
    let max_new_tokens = output_token_limit(payload, 900);
    let (raw, info) = llm.vision(VisionCall { instruction, images: vec![image], temperature, top_p: 0.95, max_new_tokens }).await?;
    let prompt = clean_thumbnail_prompt(&raw);
    if prompt.is_empty() {
        return Err("The LLM returned no thumbnail prompt. Try again or write the prompt yourself.".into());
    }
    Ok(json!({
        "prompt": prompt,
        "characters": prompt.chars().count(),
        "runner": info.get("runner").and_then(Value::as_str).unwrap_or("builtin"),
        "used_model": info.get("used_model").and_then(Value::as_str).unwrap_or(""),
        "unloaded": info.get("unloaded").and_then(Value::as_bool).unwrap_or(false),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Asked, Canned};
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_title_is_asked_for_exactly_and_no_title_means_no_text() {
        let with_title = thumbnail_instruction("NEVER \"LOOK\" BACK");
        assert!(with_title.contains("NEVER 'LOOK' BACK"));
        assert!(!with_title.contains("{text_rule}"));
        let without = thumbnail_instruction("   ");
        assert!(without.contains("Do not add any text"));
        assert!(!without.contains("{text_rule}"));
    }

    #[test]
    fn the_reply_is_cleaned_like_the_packs() {
        assert_eq!(clean_thumbnail_prompt("```text\n\"Recrop the attached image.\nMake it pop.\"\n```"), "Recrop the attached image. Make it pop.");
        assert_eq!(clean_thumbnail_prompt("Prompt: Recrop the image."), "Recrop the image.");
        assert_eq!(clean_thumbnail_prompt("Here is the prompt: Recrop the image."), "Recrop the image.");
        assert_eq!(clean_thumbnail_prompt("'It's fine'"), "It's fine");
        assert_eq!(clean_thumbnail_prompt("\""), "\"");
        assert_eq!(clean_thumbnail_prompt(""), "");
    }

    #[test]
    fn the_notes_follow_the_instruction_and_are_cut_to_600_characters() {
        let notes = user_block(&obj(json!({"title": "  Big   Night ", "style_notes": "neon", "scene_lyrics": "x".repeat(700)})));
        assert!(notes.starts_with("THUMBNAIL TITLE: Big Night\nSTYLE NOTES: neon\nSCENE LYRICS: "));
        assert_eq!(notes.lines().last().unwrap().chars().count(), "SCENE LYRICS: ".len() + 600);
        assert_eq!(user_block(&Obj::new()), "No extra notes. Pick a strong, fitting look from the image.");
    }

    #[tokio::test]
    async fn the_scene_image_is_shown_to_the_llm_and_the_reply_is_returned_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let picture = dir.path().join("scene.png");
        image::RgbImage::from_pixel(64, 36, image::Rgb([200, 10, 10])).save(&picture).unwrap();
        let llm = Canned::new(&["Prompt: \"Recrop the attached image tight on her face.\""]);
        let payload = obj(json!({"image_path": picture.display().to_string(), "title": "RUN", "style_notes": "neon city"}));
        let reply = generate_thumbnail_prompt(&llm, &payload).await.unwrap();
        assert_eq!(reply["prompt"], "Recrop the attached image tight on her face.");
        assert_eq!(reply["characters"], 44);
        assert_eq!(reply["runner"], "lm_studio_vision");
        let asked = llm.asked.lock().unwrap();
        let Asked::Vision { instruction, sizes, temperature, top_p, .. } = &asked[0] else { panic!("the LLM was not asked with the picture") };
        assert_eq!(sizes, &vec![(64, 36)]);
        assert!(instruction.contains("\"RUN\""));
        assert!(instruction.ends_with("THUMBNAIL TITLE: RUN\nSTYLE NOTES: neon city"));
        assert_eq!((*temperature, *top_p), (0.75, 0.95));
    }

    #[tokio::test]
    async fn an_image_is_required_and_an_empty_reply_is_refused() {
        let llm = Canned::new(&["   "]);
        let missing = generate_thumbnail_prompt(&llm, &obj(json!({"title": "x"}))).await.unwrap_err();
        assert!(missing.starts_with("Choose a scene with an image first"));
        let dir = tempfile::tempdir().unwrap();
        let picture = dir.path().join("scene.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0])).save(&picture).unwrap();
        let empty = generate_thumbnail_prompt(&llm, &obj(json!({"image_path": picture.display().to_string()}))).await.unwrap_err();
        assert!(empty.starts_with("The LLM returned no thumbnail prompt"));
    }
}
