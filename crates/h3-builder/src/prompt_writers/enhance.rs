//! Enhance a finished MiniMax H3 prompt with the LLM, with or without the mapped reference pictures. Port of `_enhance_minimax_h3_prompt` in
//! `llm/video_prompt_generation.py` and `llm/minimax_reference_enhance.py`. A reply that loses a label, a cue time or a sung word is refused, so
//! the scene keeps its prompt (`h3_core::prompt::enhance`).

use fancy_regex::Regex;
use font8x8::{UnicodeFonts, BASIC_FONTS};
use h3_core::prompt::enhance::{merge_enhanced_prompt, merge_reference_enhanced_prompt};
use h3_core::pyjson::dumps_inline;
use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_llm::locations::extract_json_object;
use h3_llm::prompts::text as constant;
use h3_llm::runner::output_token_limit;
use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};
use serde_json::{json, Value};

use super::images::image_from_payload;
use super::{flag, number, text, whole, Llm, TextCall, VisionCall};
use crate::instructions::effective_instruction;

type Reply = Result<Value, String>;

fn matches_whole(pattern: &str, value: &str) -> bool {
    Regex::new(&format!("^(?:{pattern})$")).expect("valid pattern").is_match(value).unwrap_or(false)
}

fn contract() -> &'static str {
    constant("minimax", "MINIMAX_H3_ENHANCE_CONTINUITY_CONTRACT")
}

/// Draw `text` at `(x, y)` in white with the 8x8 bitmap font, each dot `scale` pixels wide.
fn draw_text(canvas: &mut RgbImage, text: &str, x: u32, y: u32, scale: u32) {
    for (index, ch) in text.chars().enumerate() {
        let Some(glyph) = BASIC_FONTS.get(ch) else { continue };
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..8u32 {
                if bits >> column & 1 == 1 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (px, py) = (x + index as u32 * 8 * scale + column * scale + dx, y + row as u32 * scale + dy);
                            if px < canvas.width() && py < canvas.height() {
                                canvas.put_pixel(px, py, Rgb([255, 255, 255]));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Keep every reference visible, including API runners that accept at most four pictures. More than four are tiled with their labels onto at
/// most four sheets.
pub fn prepare_reference_images(references: &[Value]) -> Result<(Vec<DynamicImage>, String), String> {
    let mut images: Vec<RgbImage> = Vec::new();
    for item in references {
        let label = format!("Enhance reference {}", text_or_empty(item.get("picture_label")));
        let picture = image_from_payload(&text_or_empty(item.get("path")), &text_or_empty(item.get("data")), &label)?;
        images.push(picture.to_rgb8());
    }
    if images.len() <= 4 {
        let map = references.iter().enumerate().map(|(i, item)| format!("Attached image {} = {} ({}).", i + 1, text_or_empty(item.get("picture_label")), text_or_empty(item.get("label")))).collect::<Vec<_>>().join("\n");
        return Ok((images.into_iter().map(DynamicImage::ImageRgb8).collect(), map));
    }
    // No silent truncation to four: tile all references, with their immutable labels, into at most four sheets.
    let mut sheets = Vec::new();
    let group_size = images.len().div_ceil(4);
    for start in (0..images.len()).step_by(group_size) {
        let group = &images[start..(start + group_size).min(images.len())];
        let mut sheet = RgbImage::from_pixel(1536, 808 * group.len().div_ceil(2) as u32, Rgb([32, 32, 32]));
        for (offset, picture) in group.iter().enumerate() {
            let item = &references[start + offset];
            let (x, y) = ((offset % 2) as u32 * 768, (offset / 2) as u32 * 808);
            draw_text(&mut sheet, &format!("{} / {}", text_or_empty(item.get("picture_label")), text_or_empty(item.get("label"))), x + 12, y + 10, 2);
            let scale = (768.0 / f64::from(picture.width())).min(768.0 / f64::from(picture.height()));
            let tile = image::imageops::resize(picture, ((f64::from(picture.width()) * scale) as u32).max(1), ((f64::from(picture.height()) * scale) as u32).max(1), FilterType::Lanczos3);
            image::imageops::replace(&mut sheet, &tile, i64::from(x + (768 - tile.width()) / 2), i64::from(y + 40 + (768 - tile.height()) / 2));
        }
        sheets.push(DynamicImage::ImageRgb8(sheet));
    }
    Ok((sheets, "Attached images are labeled contact sheets. Each tile heading gives its exact picture/subject assignment. Inspect every tile separately; do not treat tiles as one scene.".into()))
}

/// `enhance_with_references`: enhance with the mapped character and location pictures.
async fn enhance_with_references(llm: &dyn Llm, payload: &Obj) -> Reply {
    let current = text(payload, "current_prompt");
    if current.is_empty() {
        return Err("This scene has no MiniMax H3 prompt to enhance. Create the prompt first.".into());
    }
    let references: Vec<Value> = match payload.get("image_references") {
        Some(Value::Array(items)) if (1..=9).contains(&items.len()) => items.clone(),
        _ => return Err("Enhancement with images needs 1\u{2013}9 assigned character/location reference images.".into()),
    };
    let (mut labels, mut pictures): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for item in &references {
        let Some(map) = item.as_object() else { return Err("Invalid enhancement image reference.".into()) };
        let label = map.get("label").map(|v| text_or_empty(Some(v))).unwrap_or_default();
        let picture = map.get("picture_label").map(|v| text_or_empty(Some(v))).unwrap_or_default();
        if !matches_whole(r"<(?:Subject|Picture) [1-9]\d*>", &label) || !matches_whole(r"<Picture [1-9]\d*>", &picture) || labels.contains(&label) || pictures.contains(&picture) {
            return Err("Enhancement reference labels must be unique and match the assigned subject/picture order.".into());
        }
        let path = text_or_empty(map.get("path"));
        if !(map.get("path").is_some_and(truthy) || map.get("data").is_some_and(truthy)) || path.starts_with("refmod://") {
            return Err(format!("{label} has no inspectable reference image. Assign an image or enhance without images."));
        }
        labels.push(label);
        pictures.push(picture);
    }
    if !references.iter().any(|r| r.get("describe").is_some_and(truthy)) {
        return Err("No mapped character/location references are available to describe. Use reference-to-video or enhance without images.".into());
    }
    let mut instruction = effective_instruction(payload, "minimax_h3_prompt_enhance_with_references", constant("minimax", "MINIMAX_H3_PROMPT_ENHANCE_WITH_REFERENCES_INSTRUCTIONS"));
    if !instruction.contains(contract()) {
        instruction.push_str("\n\n");
        instruction.push_str(contract());
    }
    let (images, image_map) = prepare_reference_images(&references)?;
    let mapping: Vec<Value> = references
        .iter()
        .map(|item| {
            let mut entry = Obj::new();
            for key in ["label", "picture_label", "name", "kind", "purpose", "describe"] {
                entry.insert(key.into(), item.get(key).cloned().unwrap_or(Value::Null));
            }
            Value::Object(entry)
        })
        .collect();
    let question = format!("{instruction}\n\nIMAGE ASSIGNMENTS:\n{image_map}\n{}\n\nCURRENT PROMPT:\n{current}", dumps_inline(&Value::Array(mapping)));
    let temperature = number(payload, "temperature", 0.35)?;
    let top_p = number(payload, "top_p", 0.9)?;
    let tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 4500)?);
    let (reply, info) = llm.vision(VisionCall { instruction: question, images, temperature, top_p, max_new_tokens: tokens }).await?;
    let parsed = extract_json_object(&reply)?;
    let prompt = merge_reference_enhanced_prompt(&current, &parsed, &references)?;
    Ok(json!({
        "prompt": prompt,
        "characters": prompt.chars().count(),
        "used_image_reference": true,
        "reference_image_count": references.len(),
        "runner": info.get("runner").cloned().unwrap_or_else(|| json!("builtin")),
        "used_model": info.get("used_model").cloned().unwrap_or_else(|| json!("")),
        "unloaded": info.get("unloaded").cloned().unwrap_or_else(|| json!(false)),
    }))
}

/// `POST enhance_minimax_h3_prompt`: improve a finished MiniMax prompt using the selected text-only or reference-image task. Text-only uses just
/// the reply's shot blocks. Vision can also update mapped visual definitions.
pub async fn enhance_minimax_h3_prompt(llm: &dyn Llm, payload: &Obj) -> Reply {
    if flag(payload, "use_reference_images") {
        return enhance_with_references(llm, payload).await;
    }
    let current = text(payload, "current_prompt");
    if current.is_empty() {
        return Err("This scene has no MiniMax H3 prompt to enhance. Create the prompt first.".into());
    }
    let mut instruction = effective_instruction(payload, "minimax_h3_prompt_enhance", constant("minimax", "MINIMAX_H3_PROMPT_ENHANCE_INSTRUCTIONS"));
    // Saved project and scene instructions may predate the continuity fix. Both enhance buttons must use it.
    if !instruction.contains(contract()) {
        instruction = format!("{}\n\n{}", instruction.trim(), contract());
    }
    let tokens = whole(payload, "max_new_tokens", (4000i64).min(current.chars().count() as i64 / 2 + 600))?;
    let call = TextCall::new(format!("{}\n\nCURRENT PROMPT:\n{current}", instruction.trim()), number(payload, "temperature", 0.35)?, number(payload, "top_p", 0.9)?, tokens, "MiniMax H3 prompt enhance").paragraphs();
    let (reply, info) = llm.text(call).await?;
    let prompt = merge_enhanced_prompt(&current, &reply)?;
    let unload_default = payload.get("unload_after").is_none_or(truthy);
    Ok(json!({
        "prompt": prompt,
        "characters": prompt.chars().count(),
        "runner": info.get("runner").cloned().unwrap_or_else(|| json!("builtin")),
        "used_model": info.get("used_model").cloned().unwrap_or_else(|| json!("")),
        "unloaded": info.get("unloaded").cloned().unwrap_or_else(|| json!(unload_default)),
    }))
}
