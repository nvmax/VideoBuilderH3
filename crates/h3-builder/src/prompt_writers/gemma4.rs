//! Gemma 4 drafts for the Prompt Creator and the Reference Builder: style, story idea, subjects and locations, lyrics, concept to image or video
//! prompt, advanced prompt details and a detailed location description. Port of `llm/gemma4.py`. The pack runs its built-in runner on a local GGUF
//! file. Here every runner, the built-in one included, is the runner the request selects (the built-in one is ComfyUI's Gemma).

use fancy_regex::Regex;
use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_llm::prompts::text as constant;
use h3_llm::runner::runner_kind;
use serde_json::{json, Value};

use super::{flag, number, text, whole, Llm, TextCall};
use crate::instructions::prompt_creator_instruction;

type Reply = Result<Value, String>;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

/// A model reply without a code fence around it.
fn clean_text(value: &str) -> String {
    let text = value.trim();
    let text = rx(r"(?i)^\s*```(?:text)?\s*").replace(text, "");
    rx(r"\s*```\s*$").replace(&text, "").trim().to_string()
}

fn gemma(name: &str) -> &'static str {
    constant("gemma4", name)
}

/// The first of several texts that is not empty.
fn first(values: &[&str]) -> String {
    values.iter().find(|v| !v.is_empty()).map_or_else(String::new, |v| v.to_string())
}

/// `_build_gemma4_prompt`: the full prompt a target sends.
fn build_prompt(target: &str, payload: &Obj) -> Result<String, String> {
    let notes = text(payload, "notes");
    let lyrics = text(payload, "lyrics");
    let style_theme = text(payload, "style_theme");
    let story_idea = text(payload, "story_idea");
    let instructions = |key: &str, name: &str| prompt_creator_instruction(payload, key, gemma(name));
    let mut prompt;
    match target {
        "builder_style_theme" => {
            let idea = first(&[&notes, &lyrics, &style_theme, &story_idea]);
            return Ok(format!(
                "{}\n\nCreate the style/theme block from normal user ideas instead of lyrics.\nUse the user's rough idea as the full creative direction.\nIf the idea is short, infer a useful cinematic visual style without adding extra sections.\n\nUser idea:\n{idea}",
                instructions("style_theme", "_VRGDG_GEMMA4_STYLE_INSTRUCTIONS")
            ));
        }
        "builder_story_idea" => {
            let idea = first(&[&notes, &story_idea, &lyrics]);
            prompt = format!(
                "{}\n\nCreate the story idea from normal user ideas instead of lyrics.\nTreat the user idea as the full creative foundation.\nDo not ask for lyrics. Output only the story concept.\n\nUser idea:\n{idea}",
                instructions("story_idea", "_VRGDG_GEMMA4_STORY_INSTRUCTIONS")
            );
            if !style_theme.is_empty() {
                prompt.push_str(&format!("\n\nStyle/theme:\n{style_theme}"));
            }
        }
        "builder_subjects_and_scenes" => {
            let idea = first(&[&notes, &story_idea, &lyrics]);
            prompt = format!(
                "{}\n\nCreate the subject and location list from normal user ideas instead of lyrics.\nUse the user idea as the highest priority creative direction.\n",
                instructions("subject_locations", "_VRGDG_GEMMA4_SUBJECTS_INSTRUCTIONS")
            );
            if !style_theme.is_empty() {
                prompt.push_str(&format!("\n\nStyle/theme:\n{style_theme}"));
            }
            prompt.push_str(&format!("\n\nStory or user idea:\n{idea}"));
        }
        "style_theme" => {
            prompt = format!("{}\n\nfull lyrics:\n{lyrics}", instructions("style_theme", "_VRGDG_GEMMA4_STYLE_INSTRUCTIONS"));
            if !notes.is_empty() {
                prompt.push_str(&format!("\n\nother notes:\n{notes}"));
            }
        }
        "story_idea" => {
            prompt = format!("{}\n\nLyrics:\n{lyrics}", instructions("story_idea", "_VRGDG_GEMMA4_STORY_INSTRUCTIONS"));
            if !style_theme.is_empty() {
                prompt.push_str(&format!("\n\nStyle/theme:\n{style_theme}"));
            }
            if !notes.is_empty() {
                prompt.push_str(&format!("\n\nOptional notes:\n{notes}"));
            }
        }
        "subjects_and_scenes" => {
            prompt = instructions("subject_locations", "_VRGDG_GEMMA4_SUBJECTS_INSTRUCTIONS");
            if !notes.is_empty() {
                prompt.push_str(&format!("\n\nUser notes - highest priority:\n{notes}"));
            }
            prompt.push_str(&format!("\n\nStory idea:\n{story_idea}"));
        }
        "song_lyrics" => {
            let duration = text(payload, "duration");
            prompt = instructions("full_lyrics", "_VRGDG_GEMMA4_LYRICS_INSTRUCTIONS");
            if !duration.is_empty() {
                prompt.push_str(&format!("\n\nRequested duration seconds:\n{duration}"));
            }
            prompt.push_str(&format!("\n\nSong idea and notes:\n{notes}"));
            if notes.is_empty() {
                prompt.push_str("\nCreate an original short song about refusing to give up.");
            }
        }
        "text_to_image_from_concept" => {
            let concept = text(payload, "concept_prompt");
            if concept.is_empty() {
                return Err("No concept prompt was provided for text-to-image generation.".into());
            }
            let extra = text(payload, "extra_user_input");
            prompt = format!("{}\n\nStory idea:\n{story_idea}\n\nStyle/theme:\n{style_theme}\n\nCurrent visual prompt:\n{concept}", gemma("_VRGDG_GEMMA4_T2I_FROM_CONCEPT_INSTRUCTIONS"));
            if !extra.is_empty() {
                prompt.push_str(&format!("\n\nExtra user input:\n{extra}"));
            }
        }
        "text_to_video_from_concept" => {
            let concept = text(payload, "concept_prompt");
            if concept.is_empty() {
                return Err("No concept prompt was provided for text-to-video generation.".into());
            }
            let subjects = text(payload, "subjects_and_scenes");
            let extra = text(payload, "extra_user_input");
            prompt = format!("{}\n\nSubject and location list:\n{subjects}\n\nStyle/theme:\n{style_theme}\n\nConcept prompt:\n{concept}", gemma("_VRGDG_GEMMA4_T2V_FROM_CONCEPT_INSTRUCTIONS"));
            if !extra.is_empty() {
                prompt.push_str(&format!("\n\nUser input:\n{extra}"));
            }
        }
        "advanced_prompt_detail" => {
            let label = Some(text(payload, "label")).filter(|l| !l.is_empty()).unwrap_or_else(|| "Custom".into());
            let lines: Vec<String> = match payload.get("prompts") {
                Some(Value::Array(items)) => items.iter().enumerate().filter_map(|(index, item)| Some(text_or_empty(Some(item)).trim().to_string()).filter(|t| !t.is_empty()).map(|t| format!("{}. {t}", index + 1))).collect(),
                _ => Vec::new(),
            };
            if lines.is_empty() {
                return Err("No scene prompts were provided for Gemma4 advanced list generation.".into());
            }
            prompt = format!("{}\n\nDetail label:\n{label}\n\n", gemma("_VRGDG_GEMMA4_ADVANCED_PROMPT_DETAIL_INSTRUCTIONS"));
            if !notes.is_empty() {
                prompt.push_str(&format!("Optional user guidance for all lists:\n{notes}\n\n"));
            }
            prompt.push_str(&format!("Scene prompts:\n{}", lines.join("\n")));
        }
        "location_description_detail" => {
            let name = text(payload, "location_name");
            let description = match payload.get("location_description").filter(|v| truthy(v)) {
                Some(value) => text_or_empty(Some(value)).trim().to_string(),
                None => notes.clone(),
            };
            if name.is_empty() || description.is_empty() {
                return Err("A location label and short description are required.".into());
            }
            prompt = format!("{}\n\nLocation label:\n{name}\n\nShort location description:\n{description}", instructions("location_description_detail", "_VRGDG_GEMMA4_LOCATION_DETAIL_INSTRUCTIONS"));
        }
        other => return Err(format!("Unsupported Gemma4 target: {other}")),
    }
    Ok(prompt)
}

/// The targets whose replies keep their paragraph breaks.
const PARAGRAPH_TARGETS: [&str; 7] = ["builder_style_theme", "style_theme", "builder_story_idea", "story_idea", "builder_subjects_and_scenes", "subjects_and_scenes", "song_lyrics"];

/// `POST /vrgdg/gemma4/generate`.
pub async fn generate(llm: &dyn Llm, payload: &Obj) -> Reply {
    let target = text(payload, "target");
    let runner = runner_kind(payload);
    if text(payload, "model_file").is_empty() && runner == "builtin" {
        return Err("No Gemma4 model_file was selected.".into());
    }
    let prompt = build_prompt(&target, payload)?;
    if prompt.trim().is_empty() {
        return Err("Gemma4 prompt was empty.".into());
    }
    // The pack reads its GGUF settings here, so a value that is not a number fails the request before any model runs.
    for (key, default) in [("n_ctx", 13000), ("n_gpu_layers", 99), ("n_threads", 8)] {
        whole(payload, key, default)?;
    }
    let mut call = TextCall::new(prompt, number(payload, "temperature", 0.75)?, number(payload, "top_p", 0.95)?, whole(payload, "max_new_tokens", 32000)?, "Gemma4");
    if PARAGRAPH_TARGETS.contains(&target.as_str()) {
        call = call.paragraphs();
    }
    let (reply, info) = llm.text(call).await?;
    let reply = clean_text(&reply);
    if reply.is_empty() {
        return Err("Gemma4 returned an empty response.".into());
    }
    let used_model = info.get("used_model").cloned().unwrap_or_else(|| json!(""));
    if runner == "builtin" {
        let unloaded = info.get("unloaded").cloned().unwrap_or_else(|| json!(flag(payload, "unload_after")));
        return Ok(json!({"text": reply, "used_model": used_model, "unloaded": unloaded}));
    }
    Ok(json!({"text": reply, "used_model": used_model, "runner": info.get("runner").cloned().unwrap_or_else(|| json!(runner)), "unloaded": false}))
}
