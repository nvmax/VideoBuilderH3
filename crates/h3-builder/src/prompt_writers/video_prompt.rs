//! The MiniMax H3 prompt writer behind the Builder's prompt buttons, the batch prompt runs and Auto Build. Port of the MiniMax H3 part of
//! `_generate_builder_t2v_prompt` in `llm/video_prompt_generation.py`.
//!
//! The Builder builds the scene's creative context itself ("MiniMax H3 shot-description task." and what goes with it) and sends it as
//! `t2i_prompt`. This writer puts the mode's instruction, the picture guidance, that context and the notes into one request, asks the
//! selected runner (with the attached pictures when there are any), cleans the reply and returns it. The Builder reads the shots from it.
//!
//! Left out because this app makes MiniMax H3 videos only: the other video modes' instruction keys and the First Last Frame mode (both
//! answer with a message), and the formatter the pack applies to an H3 reply that is not a shot-description task.

use std::path::PathBuf;

use h3_core::fresh_context::fresh_prompt_context_text;
use h3_core::prompt::emotion::emotion_expression_instruction;
use h3_core::prompt::lyric_free;
use h3_core::pyval::{py_str, repr_text, text_or_empty, truthy, Obj};
use h3_llm::clean::clean_plain_text;
use h3_llm::instructions as defaults;
use h3_llm::prompts;
use h3_llm::runner::{output_token_limit, runner_kind};
use h3_project::atomic::atomic_write_json;
use h3_project::derived::safe_project_name;
use image::imageops::FilterType;
use image::DynamicImage;
use serde_json::{json, Value};

use super::image_prompts::read_text_file;
use super::images::{image_from_data_url, image_from_payload};
use super::{flag, number, text, whole, Llm, TextCall, VisionCall};
use crate::files::resolve_existing_file;
use crate::instructions::effective_instruction;

type Reply = Result<Value, String>;

fn absolute(path: &std::path::Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

const SHOT_TASK_MARKER: &str = "MiniMax H3 shot-description task.";
const DEFAULT_NOTES: &str = "Create cinematic camera movement and natural subject/environment motion that fits the scene.";
const NO_SUBJECT: &str = "Subject visibility:\nNo main character, singer, performer, person, mapped subject, or character reference is present in this scene. Use location, props, objects, atmosphere, and camera motion instead.";
const FALLBACK_CONCEPT: &str = "Use the available scene notes, mapped references, lyrics/performance context, location details, and motion notes as the scene concept.";
const LYRIC_FREE: &str = "\n\nLYRIC-FREE CUSTOM AUDIO: Follow the scene shot contract. Omit all lyric quotations and <d> tags. Use passionate singing in sync with <Audio 1> only for assigned vocal intervals. Instrumental shots describe only visual action. For multiple shots, never describe mouth, lip or jaw movement anywhere. For a single shot, articulation may follow audible vocals only. Use expressive eyes and brows and the selected facial acting direction. Preserve supplied audio unchanged.";
const MAX_PICTURE_HEIGHT: u32 = 512;

fn external(kind: &str) -> bool {
    matches!(kind, "lm_studio" | "llm_api" | "own_server")
}

/// `str(a or b or default).strip()`: the first value that is not empty, as text.
fn first_truthy(payload: &Obj, keys: &[&str], default: &str) -> String {
    keys.iter().filter_map(|k| payload.get(*k)).find(|v| truthy(v)).map_or_else(|| default.to_string(), |v| text_or_empty(Some(v)).trim().to_string())
}

/// The singer cue rows of `lyric_cue_map` as lines for the notes.
fn cue_lines(cues: &[Value], omit_lyrics: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, cue) in cues.iter().enumerate() {
        let Some(cue) = cue.as_object() else { continue };
        let index = index + 1;
        let instrumental = text_or_empty(cue.get("type")).trim().to_lowercase() == "instrumental";
        let singer = first_truthy(cue, &["singer_name", "speaker_name"], "the assigned singer");
        let words = text_or_empty(cue.get("text")).trim().to_string();
        let timing = match (cue.get("start"), cue.get("end")) {
            (Some(start), Some(end)) if !start.is_null() && !end.is_null() => format!(" [{}s-{}s]", py_str(Some(start)), py_str(Some(end))),
            _ => String::new(),
        };
        lines.push(if instrumental {
            format!("Cue {index}{timing}: INSTRUMENTAL — no subject sings, speaks, or lip-syncs.")
        } else if omit_lyrics {
            format!("Cue {index}{timing}: {singer} sings with passion in sync with <Audio 1> only during this vocal interval; omit lyric words.")
        } else {
            format!("Cue {index}{timing}: {singer} is the only singer for the exact words: {}.", repr_text(&words))
        });
    }
    lines
}

/// Python's `str.splitlines` separators.
fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The pictures the request attaches: the `image_references` list (paths or data URLs), else one single reference.
fn pictures(payload: &Obj, limit: usize) -> Result<Vec<DynamicImage>, String> {
    let mut references = payload.get("image_references").filter(|v| truthy(v)).cloned().unwrap_or_else(|| Value::Array(Vec::new()));
    if let Value::String(raw) = &references {
        references = match serde_json::from_str::<Value>(raw) {
            Ok(parsed) => parsed,
            Err(_) => Value::Array(raw.split(is_line_break).map(str::trim).filter(|line| !line.is_empty()).map(|line| json!({"path": line})).collect()),
        };
    }
    let mut found = Vec::new();
    if let Value::Array(items) = &references {
        for (index, item) in items.iter().take(limit).enumerate() {
            let item = match item {
                Value::String(path) => json!({"path": path}),
                other => other.clone(),
            };
            let Some(item) = item.as_object() else { continue };
            let label = format!("T2V Gemma image reference {}", index + 1);
            let picture = image_from_payload(&text_or_empty(item.get("path")), &text_or_empty(item.get("data")), &label).map_err(|e| format!("Could not load {label}: {e}"))?;
            found.push(picture);
        }
    }
    if found.is_empty() {
        let data = text(payload, "image_reference_data");
        let path = text(payload, "image_reference_path").trim_matches('"').to_string();
        if !data.is_empty() {
            found.push(image_from_data_url(&data)?);
        } else if !path.is_empty() {
            let file = resolve_existing_file(&path, "T2V Gemma image reference")?;
            found.push(super::images::open_rgb(&file)?);
        }
    }
    Ok(found)
}

/// A picture taller than 512 pixels is scaled down to that height before the model sees it.
fn shrink(picture: DynamicImage) -> DynamicImage {
    if picture.height() <= MAX_PICTURE_HEIGHT {
        return picture;
    }
    let width = ((f64::from(picture.width()) * (f64::from(MAX_PICTURE_HEIGHT) / f64::from(picture.height().max(1)))) as u32).max(1);
    picture.resize_exact(width, MAX_PICTURE_HEIGHT, FilterType::Lanczos3)
}

fn image_guidance(has_pictures: bool, frame_continuity: bool, shot_task: bool, inspiration: bool) -> &'static str {
    if !has_pictures {
        ""
    } else if frame_continuity {
        "Vision attachment mapping:\n- Picture 1 is the previous render's final frame and the authoritative opening state. It is prompt-writing input, not a renderer Image N label.\n- Later pictures are the supporting inputs documented by the resolved Scene concept and cannot replace Picture 1's opening state.\n\n"
    } else if shot_task {
        "MiniMax H3 JSON-shot visual-reference guidance:\n- Inspect the attached pictures only according to the exact assignments and limits in the Scene concept.\n- Use permitted visual facts directly inside the creative shot descriptions.\n- Return only the requested valid JSON object.\n\n"
    } else if inspiration {
        "MiniMax H3 prompt-only scene-inspiration guidance:\n- Attached <Picture 1> is vision input for prompt writing only. It is never supplied to the MiniMax renderer and must never appear as Image 1 or any Image N in the finished prompt.\n- Follow the scene context's exact environment-only or environment-plus-framing extraction limits for <Picture 1>. Always ignore all character identity, appearance, clothing, body, pose, placement, and activity visible in it.\n- Attached <Picture 2> corresponds to renderer Image 1, <Picture 3> to renderer Image 2, and so on. Inspect each renderer picture only for its assigned purpose.\n- In the finished prompt, use only renderer Image N labels. Convert permitted observations from <Picture 1> into direct scene prose without mentioning pictures, inspiration, analysis, or source imagery.\n- Do not merge a storyboard grid into a collage output; interpret its panels as ordered visual guidance.\n\n"
    } else {
        "MiniMax H3 ordered visual-reference guidance:\n- The attached pictures are in the exact <Picture 1>, <Picture 2>, and subsequent order stated in the scene context.\n- Inspect every attached picture and use it only for the purpose assigned to its matching tag.\n- Keep the exact <Picture N> tags in the final MiniMax prompt wherever the mode instructions require them.\n- Do not merge a storyboard grid into a collage output; interpret its panels as ordered visual guidance.\n\n"
    }
}

/// Where the request is kept for inspection: `llm_request_audits/last_minimax_h3_<scene>.json` in the project folder, when there is one.
fn audit_file(payload: &Obj) -> Option<PathBuf> {
    if !payload.get("project_folder").is_some_and(truthy) {
        return None;
    }
    let raw = text_or_empty(payload.get("project_folder")).trim().trim_matches('"').to_string();
    let folder = if raw.is_empty() { std::env::current_dir().ok()? } else { absolute(std::path::Path::new(&raw)) };
    if !folder.is_dir() {
        return None;
    }
    let scene = match payload.get("scene_id").filter(|v| truthy(v)) {
        Some(value) => safe_project_name(&text_or_empty(Some(value))),
        None => "scene".to_string(),
    };
    Some(folder.join("llm_request_audits").join(format!("last_minimax_h3_{scene}.json")))
}

fn or_empty_list(payload: &Obj, key: &str) -> Value {
    payload.get(key).filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([]))
}

/// `POST generate_t2v`.
pub async fn generate_t2v(llm: &dyn Llm, payload: &Obj) -> Reply {
    let model_file = text(payload, "model_file");
    // A new prompt is written from the scene's directions, not from the prompts saved on the card the request carries.
    let scene_prompt = fresh_prompt_context_text(&first_truthy(payload, &["t2i_prompt", "scene_prompt"], ""));
    let mut user_notes = fresh_prompt_context_text(&text(payload, "user_notes"));
    let performance = Some(text_or_empty(payload.get("performance_mode"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "singing".into());
    let audio_mode = Some(text_or_empty(payload.get("audio_mode"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "input_audio".into());
    let omit_lyrics = lyric_free::enabled(flag(payload, "omit_lyrics_from_video_prompts"), &performance, &audio_mode);
    if let Some(cues) = payload.get("lyric_cue_map").and_then(Value::as_array).filter(|c| !c.is_empty()) {
        // The cue rows go into the request text as well, so the singer and shot hand-off can be read there.
        let lines = cue_lines(cues, omit_lyrics);
        if !lines.is_empty() {
            let map = format!("AUTHORITATIVE SINGER CUE MAP — follow exactly:\n{}", lines.join("\n"));
            user_notes = [user_notes.as_str(), map.as_str()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n\n");
        }
    }
    let subject_context = text(payload, "subject_context");
    let location_context = text(payload, "location_context");
    let no_character = ["no_character_present", "no_subject", "no_visible_subject"].iter().any(|k| flag(payload, k));
    let raw_key = ["builder_instruction_key", "instruction_key"].iter().filter_map(|k| payload.get(*k)).find(|v| truthy(v)).map_or_else(|| "t2v".to_string(), |v| py_str(Some(v)));
    let instruction_key = defaults::safe_key(&raw_key)?;
    if !instruction_key.starts_with("minimax_h3_") {
        return Err("H3 Studio writes MiniMax H3 prompts only. Use a MiniMax H3 instruction.".into());
    }
    if ["first_last_frame_mode", "firstLastFrameMode"].iter().any(|k| flag(payload, k)) {
        return Err("First Last Frame prompts are not available in H3 Studio.".into());
    }
    let shot_task = scene_prompt.trim_start().starts_with(SHOT_TASK_MARKER);
    let inspiration = flag(payload, "prompt_only_scene_inspiration");
    let frame_continuity = flag(payload, "frame_continuity_prompt") || instruction_key == "minimax_h3_frame_continuity";
    let kind = runner_kind(payload);
    if model_file.is_empty() && !external(kind) {
        return Err("Choose a T2V Gemma model first.".into());
    }
    if !model_file.is_empty() && !external(kind) && !model_file.to_lowercase().ends_with(".gguf") {
        return Err("The T2V model field is not a GGUF model.".into());
    }
    let pictures = pictures(payload, if inspiration || frame_continuity { 10 } else { 9 })?;
    let has_pictures = !pictures.is_empty();
    if has_pictures && !external(kind) && model_file.is_empty() {
        return Err("Choose a T2V vision Gemma model first.".into());
    }
    let pictures: Vec<DynamicImage> = pictures.into_iter().map(shrink).collect();

    let theme_style = read_text_file(payload, "theme_style_path", "Theme/style file")?;
    let story_idea = read_text_file(payload, "story_idea_path", "Story idea file")?;
    let subject_scene = read_text_file(payload, "subject_scene_path", "Subject/scene file")?;
    let mut context_parts: Vec<String> = Vec::new();
    if no_character {
        context_parts.push(NO_SUBJECT.to_string());
    } else if !subject_scene.is_empty() {
        context_parts.push(format!("Subject/scene:\n{subject_scene}"));
    }
    if !subject_context.is_empty() && !no_character {
        context_parts.push(format!("Mapped scene character(s):\n{subject_context}"));
    }
    if !location_context.is_empty() {
        context_parts.push(format!("Mapped scene location:\n{location_context}"));
    }
    if !theme_style.is_empty() {
        context_parts.push(format!("Theme/style:\n{theme_style}"));
    }
    if !story_idea.is_empty() {
        context_parts.push(format!("Story idea:\n{story_idea}"));
    }
    if !context_parts.is_empty() {
        if !user_notes.is_empty() {
            context_parts.push(format!("Segment motion notes:\n{user_notes}"));
        }
        user_notes = context_parts.join("\n\n");
    }
    let scene_prompt = if !scene_prompt.is_empty() {
        scene_prompt
    } else if !user_notes.is_empty() || !subject_context.is_empty() || !location_context.is_empty() || !theme_style.is_empty() || !story_idea.is_empty() || !subject_scene.is_empty() || no_character {
        FALLBACK_CONCEPT.to_string()
    } else {
        return Err("Create or paste scene notes, mapped references, motion notes, or a T2I/concept prompt first.".into());
    };
    if !shot_task {
        return Err("H3 Studio writes MiniMax H3 prompts from the Builder's shot-description task. This request has none.".into());
    }

    let instructions = effective_instruction(payload, &instruction_key, prompts::text("video", "_T2V_INSTRUCTIONS"));
    let label = Some(defaults::label(&instruction_key)).filter(|l| *l != instruction_key).unwrap_or_else(|| "MiniMax H3".to_string());
    let guidance = image_guidance(has_pictures, frame_continuity, shot_task, inspiration);
    let notes = if user_notes.is_empty() { DEFAULT_NOTES } else { user_notes.as_str() };
    let mut prompt = format!("{instructions}\n\n{guidance}Scene concept:\n{scene_prompt}\n\nUser motion/camera notes:\n{notes}");
    if omit_lyrics {
        prompt.push_str(LYRIC_FREE);
    }
    if omit_lyrics && payload.get("lyric_text").is_some_and(truthy) {
        prompt.push_str(&format!("\n\nLYRIC CONTEXT FOR INTERPRETATION ONLY (omit these words from the final prompt):\n{}", text_or_empty(payload.get("lyric_text"))));
    }
    prompt.push_str("\n\n");
    prompt.push_str(&emotion_expression_instruction(payload));

    let audit_path = audit_file(payload);
    let mut audit = json!({
        "saved_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()),
        "scene_id": text_or_empty(payload.get("scene_id")),
        "instruction_key": instruction_key,
        "performance_mode": text_or_empty(payload.get("performance_mode")),
        "audio_mode": text_or_empty(payload.get("audio_mode")),
        "singers": or_empty_list(payload, "singers"),
        "lyric_text": text_or_empty(payload.get("lyric_text")),
        "lyric_cue_map": or_empty_list(payload, "lyric_cue_map"),
        "performer_assignment": payload.get("performer_assignment").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!({})),
        "scene_concept": scene_prompt,
        "user_notes": user_notes,
        "actual_llm_instruction": prompt,
    });
    if let Some(path) = &audit_path {
        atomic_write_json(path, &audit).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    // The pack reads these before it asks, so a value that is not a number fails at the same point.
    whole(payload, "n_ctx", 8000)?;
    whole(payload, "n_gpu_layers", 99)?;
    whole(payload, "n_threads", 8)?;
    let temperature = number(payload, "temperature", 0.7)?;
    let top_p = number(payload, "top_p", 0.95)?;
    let max_new_tokens = output_token_limit(payload, whole(payload, "max_new_tokens", 4000)?);
    let unload_after = payload.get("unload_after").is_none_or(truthy);

    let (reply, info) = if has_pictures {
        llm.vision(VisionCall { instruction: prompt, images: pictures, temperature, top_p, max_new_tokens }).await?
    } else {
        llm.text(TextCall::new(prompt, temperature, top_p, max_new_tokens, &format!("{label} Gemma")).paragraphs()).await?
    };
    if let Some(path) = &audit_path {
        audit["raw_llm_response"] = json!(reply);
        atomic_write_json(path, &audit).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let cleaned = clean_plain_text(&reply);
    if cleaned.is_empty() {
        return Err(format!("{label} LLM returned an empty prompt."));
    }
    Ok(json!({
        "prompt": cleaned,
        "used_model": info.get("used_model").cloned().unwrap_or_else(|| json!("")),
        "used_mmproj": "",
        "runner": info.get("runner").cloned().unwrap_or_else(|| json!("builtin")),
        "used_image_reference": has_pictures,
        "unloaded": info.get("unloaded").cloned().unwrap_or(json!(unload_after)),
        "llm_request_audit_path": audit_path.map_or_else(String::new, |p| p.display().to_string()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_writers::testing::Canned;

    fn payload(project: &std::path::Path) -> Obj {
        json!({
            "text_runner": "lm_studio",
            "t2i_prompt": "MiniMax H3 shot-description task.\nReturn exactly 1 JSON shot description.",
            "builder_instruction_key": "minimax_h3_reference_to_video",
            "project_folder": project.display().to_string(),
            "scene_id": "scene 3",
            "lyric_text": "la la",
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    #[tokio::test]
    async fn the_request_and_the_answer_are_kept_in_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let llm = Canned::new(&["{\"shots\":[{\"description\":\"x\"}]}"]);
        let reply = generate_t2v(&llm, &payload(dir.path())).await.unwrap();
        let path = PathBuf::from(reply["llm_request_audit_path"].as_str().unwrap());
        assert_eq!(path.file_name().unwrap(), "last_minimax_h3_scene 3.json");
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "llm_request_audits");
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved["saved_at"].is_number());
        assert_eq!(saved["scene_id"], "scene 3");
        assert_eq!(saved["instruction_key"], "minimax_h3_reference_to_video");
        assert_eq!(saved["lyric_text"], "la la");
        assert_eq!(saved["singers"], json!([]));
        assert_eq!(saved["performer_assignment"], json!({}));
        assert!(saved["actual_llm_instruction"].as_str().unwrap().contains("Scene concept:\nMiniMax H3 shot-description task."));
        assert_eq!(saved["raw_llm_response"], "{\"shots\":[{\"description\":\"x\"}]}");
    }

    #[tokio::test]
    async fn a_request_without_a_project_keeps_nothing() {
        let llm = Canned::new(&["{\"shots\":[]}"]);
        let mut request = payload(std::path::Path::new("."));
        request.remove("project_folder");
        let reply = generate_t2v(&llm, &request).await.unwrap();
        assert_eq!(reply["llm_request_audit_path"], "");
    }

    #[tokio::test]
    async fn the_modes_this_app_does_not_make_say_so() {
        let llm = Canned::new(&[]);
        let mut request = payload(std::path::Path::new("."));
        request.insert("builder_instruction_key".into(), json!("t2v"));
        assert_eq!(generate_t2v(&llm, &request).await.unwrap_err(), "H3 Studio writes MiniMax H3 prompts only. Use a MiniMax H3 instruction.");
        let mut request = payload(std::path::Path::new("."));
        request.insert("first_last_frame_mode".into(), json!(true));
        assert_eq!(generate_t2v(&llm, &request).await.unwrap_err(), "First Last Frame prompts are not available in H3 Studio.");
        let mut request = payload(std::path::Path::new("."));
        request.insert("t2i_prompt".into(), json!("A plain concept with no shot task."));
        assert!(generate_t2v(&llm, &request).await.unwrap_err().contains("shot-description task"));
    }
}
