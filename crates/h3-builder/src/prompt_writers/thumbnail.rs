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

/// `thumbnail_instruction`: the instruction for `kind` (`"edit"`, `"moss_edit"`, `"qwen_edit"` or `"restyle"`; anything else is `"edit"`), with the
/// rule for the title text filled in, or the no-text rule when there is no title. An "edit" workflow follows an instruction such as
/// "recrop the attached image". "moss_edit" (MOSS-Image Edit) follows a short one in the form its edit LoRA was trained on: what to
/// add or change, then a "Keep ... unchanged" list that names what stays. A "restyle" workflow starts from the frame as an image to
/// image source and is given a description of the finished picture instead. The title text is mandatory in every kind.
pub fn thumbnail_instruction(title: &str, kind: &str) -> String {
    let title = collapse(title).replace('"', "'");
    let (template, text_rule, no_text_rule) = match kind.trim().to_ascii_lowercase().as_str() {
        "restyle" => ("THUMBNAIL_RESTYLE_PROMPT_INSTRUCTIONS", "THUMBNAIL_TEXT_RULE", "THUMBNAIL_NO_TEXT_RULE"),
        "moss_edit" => ("THUMBNAIL_MOSS_EDIT_PROMPT_INSTRUCTIONS", "THUMBNAIL_MOSS_TEXT_RULE", "THUMBNAIL_NO_TEXT_RULE"),
        "qwen_edit" => ("THUMBNAIL_QWEN_EDIT_PROMPT_INSTRUCTIONS", "THUMBNAIL_QWEN_TEXT_RULE", "THUMBNAIL_QWEN_NO_TEXT_RULE"),
        _ => ("THUMBNAIL_EDIT_PROMPT_INSTRUCTIONS", "THUMBNAIL_TEXT_RULE", "THUMBNAIL_NO_TEXT_RULE"),
    };
    let rule = if title.is_empty() {
        prompts::text("thumbnail", no_text_rule).to_string()
    } else {
        prompts::text("thumbnail", text_rule).replace("{title}", &title)
    };
    prompts::text("thumbnail", template).replace("{text_rule}", &rule)
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

/// `POST thumbnail/prompt`: look at the chosen frame (`image_path` or `image_data`) and write the prompt that makes it a thumbnail.
/// `engine_kind` says which kind of image workflow will use it: `"edit"` (the default) for an instruction an editor follows,
/// `"moss_edit"` for the short "Add ... Keep the ... unchanged." instruction MOSS-Image Edit was trained on, `"restyle"` for a
/// description of the finished picture that an image to image workflow draws.
pub async fn generate_thumbnail_prompt(llm: &dyn Llm, payload: &Obj) -> Reply {
    let (path, data) = (text(payload, "image_path"), text(payload, "image_data"));
    if path.is_empty() && data.is_empty() {
        return Err("Choose a scene with an image first. The LLM needs the picture to write the thumbnail prompt.".into());
    }
    let image = image_from_payload(&path, &data, "Thumbnail source image")?;
    let instruction = format!("{}\n\n{}", thumbnail_instruction(&text(payload, "title"), &text(payload, "engine_kind")), user_block(payload));
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
        let with_title = thumbnail_instruction("NEVER \"LOOK\" BACK", "edit");
        assert!(with_title.contains("NEVER 'LOOK' BACK"));
        assert!(!with_title.contains("{text_rule}"));
        let without = thumbnail_instruction("   ", "edit");
        assert!(without.contains("Do not add any text"));
        assert!(!without.contains("{text_rule}"));
    }

    #[test]
    fn a_restyle_workflow_gets_a_description_of_the_finished_picture_not_an_edit_instruction() {
        let edit = thumbnail_instruction("RUN", "edit");
        let restyle = thumbnail_instruction("RUN", "restyle");
        assert!(edit.contains("Recrop the attached image"));
        assert!(!restyle.contains("Recrop the attached image"));
        assert!(restyle.contains("describes the finished thumbnail"));
        assert!(restyle.to_lowercase().contains("do not mention the attached image"));
        assert!(restyle.contains("\"RUN\""));
        assert!(!restyle.contains("{text_rule}"));
        assert!(thumbnail_instruction("", "restyle").contains("Do not add any text"));
        assert_eq!(thumbnail_instruction("RUN", " Restyle "), restyle, "the kind is not case or space sensitive");
        for other in ["", "edit", "anything else"] {
            assert_eq!(thumbnail_instruction("RUN", other), edit);
        }
    }

    #[test]
    fn moss_image_edit_gets_the_short_add_and_keep_instruction_it_was_trained_on() {
        let moss = thumbnail_instruction("HEAT", "moss_edit");
        assert!(moss.contains("in the form MOSS-Image Edit was trained on"));
        assert!(moss.contains("one sentence that starts with \"Keep\""));
        assert!(moss.contains("always name the headwear, hair and clothes"));
        assert!(moss.contains("leave out any cropping or reframing"));
        assert!(!moss.contains("Recrop the attached image"));
        assert!(!moss.contains("{text_rule}"));
        assert_ne!(moss, thumbnail_instruction("HEAT", "edit"));
        assert!(thumbnail_instruction("", "moss_edit").contains("Do not add any text"));
        assert_eq!(thumbnail_instruction("HEAT", " MOSS_EDIT "), moss, "the kind is not case or space sensitive");
    }

    #[test]
    fn qwen_image_edit_gets_a_prompt_about_the_change_not_the_picture() {
        let qwen = thumbnail_instruction("HEAT", "qwen_edit");
        assert!(!qwen.contains("# Role and Objective"), "Qwen does not use the brief: it already sees the frame");
        assert!(qwen.contains("The model already sees the frame, so the prompt describes only what to ADD or CHANGE, never the picture itself"));
        assert!(qwen.contains("most of your words go there"));
        assert!(qwen.contains("Add the text \"<the title>\" as"));
        for feature in ["lettering style", "fill colors or gradient", "outline and glow", "drips, splatter, sparks or shine", "decorative lines, swirls"] {
            assert!(qwen.contains(feature), "{feature}");
        }
        for placement in ["fully inside the frame", "how big it is", "never covers the person's face"] {
            assert!(qwen.contains(placement), "{placement}");
        }
        assert!(qwen.contains("starts with \"Keep\""));
        assert!(qwen.contains("no recropping or reframing, no blur or depth of field, no film grain"));
        assert!(qwen.contains("The title text is mandatory: the prompt must contain the text \"HEAT\" in double quotes, spelled exactly as given"));
        assert!(qwen.contains("wildstyle graffiti lettering with an orange-to-red gradient fill"));
        assert!(!qwen.contains("{text_rule}"));
        assert_ne!(qwen, thumbnail_instruction("HEAT", "edit"));
        assert_eq!(thumbnail_instruction("HEAT", " QWEN_EDIT "), qwen, "the kind is not case or space sensitive");
        assert!(thumbnail_instruction("NEVER \"LOOK\" BACK", "qwen_edit").contains("the text \"NEVER 'LOOK' BACK\" in double quotes"));
        let none = thumbnail_instruction("  ", "qwen_edit");
        assert!(none.contains("There is no title text: skip step 1, do not add any text, letters or numbers"));
        assert!(none.contains("Ignore the example below: it is about a title."));
        assert!(!none.contains("is mandatory"));
    }

    #[test]
    fn every_kind_starts_with_the_project_owners_brief_unchanged_and_then_asks_for_one_prompt() {
        let brief = prompts::text("thumbnail", "THUMBNAIL_ROLE_INSTRUCTIONS");
        assert!(brief.starts_with("# Role and Objective\nYou are an expert AI image editor and creative director"));
        assert!(brief.ends_with("optimized for small mobile screens."));
        for sentence in [
            "# Core Enhancement Guidelines",
            "# Output Constraints",
            "Crop or frame tightly on the subject (chest-up or waist-up) with intense, engaging eye contact toward the camera.",
            "Maintain intentional **negative space** on one side of the frame (left or right) so a title text overlay can be added later.",
            "Deepen shadows and boost mid-tone clarity on the artist\u{2019}s face and clothing to create a high-end editorial look.",
            "- Keep the face, likeness, and key identity of the subject from the input image recognizable.",
        ] {
            assert!(brief.contains(sentence), "{sentence}");
        }
        for kind in ["edit", "restyle", "moss_edit"] {
            let instruction = thumbnail_instruction("HEAT", kind);
            assert!(instruction.starts_with(brief), "{kind}: the brief comes first, untouched");
            let after = &instruction[brief.len()..];
            assert!(after.starts_with("\n\n# Your Answer\n"), "{kind}");
            assert!(after.contains("write the one prompt it will receive"), "{kind}");
            assert!(after.contains("Output only the prompt: no title, no quotes around it, no explanation, no markdown."), "{kind}");
            assert!(!instruction.contains("{text_rule}"), "{kind}");
        }
    }

    #[test]
    fn the_title_is_mandatory_and_quoted_in_every_kind() {
        for kind in ["edit", "restyle", "moss_edit"] {
            let instruction = thumbnail_instruction("NEVER \"LOOK\" BACK", kind);
            assert!(instruction.contains("is mandatory"), "{kind}");
            assert!(instruction.contains("in double quotes"), "{kind}");
            assert!(instruction.contains("\"NEVER 'LOOK' BACK\" in double quotes"), "{kind}");
            assert!(instruction.contains("Follow any lettering style in the style notes"), "{kind}");
            assert!(!thumbnail_instruction("", kind).contains("is mandatory"), "{kind}");
        }
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
    async fn the_kind_of_workflow_decides_which_instruction_the_llm_gets() {
        let dir = tempfile::tempdir().unwrap();
        let picture = dir.path().join("scene.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9])).save(&picture).unwrap();
        let llm = Canned::new(&["A close-up of a singer.", "Recrop the attached image."]);
        let path = picture.display().to_string();
        generate_thumbnail_prompt(&llm, &obj(json!({"image_path": path, "engine_kind": "restyle"}))).await.unwrap();
        generate_thumbnail_prompt(&llm, &obj(json!({"image_path": path}))).await.unwrap();
        let asked = llm.asked.lock().unwrap();
        let instruction = |index: usize| match &asked[index] {
            Asked::Vision { instruction, .. } => instruction.clone(),
            other => panic!("not a vision call: {other:?}"),
        };
        assert!(instruction(0).contains("describes the finished thumbnail"), "restyle describes the picture");
        assert!(!instruction(0).contains("Recrop the attached image"));
        assert!(instruction(1).contains("Recrop the attached image"), "no kind means edit");
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
