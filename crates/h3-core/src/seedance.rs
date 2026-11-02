//! Converting a MiniMax prompt to a Seedance 2.5 Reference to Video prompt without losing a word of it. Port of
//! `llm/seedance_conversion.py` (everything but the files and the LLM call).
//!
//! The converter never lets the LLM rewrite the scene. It retags the references, translates the MiniMax delivery marks and
//! splits the prompt into immutable blocks. The LLM only answers with a layout plan (which block goes under which heading).
//! [`assemble_seedance_prompt`] checks that every block appears exactly once and in order before the plan is applied.

use serde::Serialize;
use serde_json::{json, Value};

use crate::prompt::rx::{group, is_match, rx, sub, sub_with};
use crate::pyval::{py_str, text_or_empty, truthy, Obj};
use crate::timeline::py_splitlines;

/// The headings a converted prompt is laid out under.
pub const HEADINGS: [&str; 7] = ["References", "Scene", "Preservation", "Shots", "Sound", "Music", "Final assembly"];

/// Source headings of a saved prompt and the heading each belongs under.
const SOURCE_HEADINGS: [(&str, &str); 8] = [
    ("subject_definitions", "References"),
    ("reference_definitions", "References"),
    ("summary", "Scene"),
    ("retention_analysis", "Preservation"),
    ("detailed_description", "Shots"),
    ("integrated_multimodal_description", "Shots"),
    ("overall_soundscape", "Sound"),
    ("non_diegetic_music", "Music"),
];

/// Added to a scene that sings in scene-audio mode, so instrumentalists never get a vocal part.
pub const VOCAL_ROLE_GUIDANCE: &str = concat!(
    "Follow the singer/speaker roles explicitly assigned in this scene. Instrument-only performers keep their mouths closed unless the source explicitly gives them a vocal part. ",
    "Keep each lyric phrase assigned to its source performer, including parenthetical words. Backing singers already identified in the scene perform their distinct backing-vocal responses where supported by the source assignment and @Audio 1, at their actual recorded times. ",
    "Preserve the source's lead-vocal assignment for ambiguous phrases. Each performer follows their own assigned vocal or instrumental part. ",
    "Keep the active singer's face and mouth readable during each vocal phrase."
);

const SCENE_AUDIO_DEFINITION: &str = "@Audio 1 is the supplied project/custom audio excerpt for this scene, beginning at scene time 0. Follow its original words, vocal phrasing, pauses and tempo. The full original soundtrack is retained by the Builder during final assembly.";

/// The JSON schema the LLM's layout plan must follow.
pub fn plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"sections": {"type": "array", "minItems": 1, "items": {
            "type": "object",
            "properties": {
                "heading": {"type": "string", "enum": HEADINGS},
                "block_ids": {"type": "array", "minItems": 1, "items": {"type": "string"}},
            },
            "required": ["heading", "block_ids"],
            "additionalProperties": false,
        }}},
        "required": ["sections"],
        "additionalProperties": false,
    })
}

/// A piece of the source prompt that is placed under a heading as it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Block {
    pub id: String,
    pub heading_hint: String,
    pub text: String,
}

/// Quoted lyrics and dialogue: a straight quote pair (with escapes) or a curly one.
const QUOTED: &str = r#"(?s)"(?:\\.|[^"\\])*"|“[^”]*”"#;

/// Apply `transform` to the narration only, never to the quoted lyrics or dialogue.
fn narration_only(text: &str, transform: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut last = 0;
    for found in rx!(QUOTED).find_iter(text).filter_map(Result::ok) {
        out.push_str(&transform(&text[last..found.start()]));
        out.push_str(found.as_str());
        last = found.end();
    }
    out.push_str(&transform(&text[last..]));
    out
}

/// One paragraph can hold several shots: the repairs are scoped to each shot.
fn normalize_shot_safety(text: &str) -> String {
    let mut cuts = vec![0];
    cuts.extend(rx!(r"(?i)\[Shot \d+\]").find_iter(text).filter_map(Result::ok).map(|found| found.start()));
    cuts.push(text.len());
    cuts.windows(2).map(|pair| normalize_single_shot(&text[pair[0]..pair[1]])).collect()
}

fn normalize_single_shot(text: &str) -> String {
    let continuity = |part: &str| {
        let part = sub(rx!(r"(?i)\bHard cut,\s*the same moment on the same stage\b"), part, "Hard cut, continuing immediately from the preceding shot on the same stage");
        sub(rx!(r"(?i)\bHard cut,\s*(?:the )?same moment\b"), &part, "Hard cut, continuing immediately from the preceding shot")
    };
    let mut text = narration_only(text, continuity);
    // A vocal close-up is detected from the narration, leaving the quoted performance out.
    let narration = sub(rx!(QUOTED), &text, "");
    if is_match(rx!(r"(?i)\bclose[- ]?up\b"), &narration) && is_match(rx!(r"(?i)\b(?:sings?|singing|speaks?|speaking|lip[- ]?syncs?)\b"), &narration) {
        text = narration_only(&text, |part| sub(rx!(r"(?i)\bspinning (?:once|a full turn)\b"), part, "making a partial turn toward the camera while keeping the singing mouth visible"));
    }
    text
}

/// Translate MiniMax delivery marks without changing a single sung or spoken word.
fn clean_performance_syntax(text: &str) -> String {
    let delivery = |caps: &fancy_regex::Captures| {
        let body = group(caps, 1).trim().to_string();
        let metadata = rx!(r"^\[([^\]]+)\]\s*").captures(&body).ok().flatten();
        let (directions, lyric) = match &metadata {
            Some(found) => (group(found, 1), body[found.get(0).map_or(0, |whole| whole.end())..].to_string()),
            None => (String::new(), body.clone()),
        };
        let emphasis: Vec<String> = rx!(r"(?is)<i>(.*?)</i>").captures_iter(&lyric).filter_map(Result::ok).map(|found| group(&found, 1)).collect();
        let lyric = sub(rx!(r"(?i)</?i>"), &lyric, "");
        let mut result = if directions.is_empty() { String::new() } else { format!("({directions}) ") };
        result.push_str(&format!("\"{lyric}\""));
        if !emphasis.is_empty() {
            result.push_str(&format!(" (emphasize {})", emphasis.iter().map(|word| format!("\"{word}\"")).collect::<Vec<_>>().join(", ")));
        }
        result
    };
    let text = sub_with(rx!(r"(?is)<d>(.*?)</d>"), text, delivery);
    let text = sub(rx!(r"(?is)<i>(.*?)</i>"), &text, "$1");
    let text = sub(rx!(r"(?i)</?d>"), &text, "");
    text.replace(
        "preserving every person's exact position, posture, and movement across every camera cut",
        "continuing every person's position, posture, and movement naturally across every camera cut without resetting or freezing the action",
    )
}

/// `str.title()` for a single English word.
fn title(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or(String::new(), |first| first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase())
}

fn count_of(scene: &Obj, key: &str) -> usize {
    scene.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

/// Retag the references of a saved scene prompt to the real upload order and split it into immutable blocks.
pub fn prepare_seedance_blocks(scene: &Obj) -> Result<Vec<Block>, String> {
    let text = text_or_empty(scene.get("source_prompt")).trim().to_string();
    if text.is_empty() {
        return Err("The source prompt is empty.".into());
    }
    let mut aliases: Obj = scene.get("reference_aliases").and_then(Value::as_object).cloned().unwrap_or_default();
    let images = count_of(scene, "image_references");
    for index in 1..=images {
        aliases.entry(format!("<Picture {index}>")).or_insert_with(|| json!(format!("@Image {index}")));
        aliases.entry(format!("<Image {index}>")).or_insert_with(|| json!(format!("@Image {index}")));
    }
    for (kind, key) in [("Video", "video_paths"), ("Audio", "audio_paths")] {
        for index in 1..=count_of(scene, key) {
            aliases.entry(format!("<{kind} {index}>")).or_insert_with(|| json!(format!("@{kind} {index}")));
        }
    }
    let audio_paths = scene.get("audio_paths").is_some_and(truthy);
    if scene.get("original_soundtrack").is_some_and(truthy) && !audio_paths {
        aliases.entry("<Audio 1>".to_string()).or_insert_with(|| json!("Original soundtrack"));
    }
    // Native tags and bare Picture/Image labels both occur in saved Builder prompts.
    let mut retagged = String::new();
    let mut last = 0;
    for found in rx!(r"(?i)(?<![\w@])<?(Subject|Picture|Image|Video|Audio)\s+(\d+)>?").captures_iter(&text) {
        let Ok(found) = found else { break };
        let whole = found.get(0).ok_or("no match")?;
        let kind = title(&group(&found, 1));
        let number = group(&found, 2).trim_start_matches('0').to_string();
        let number = if number.is_empty() { "0".to_string() } else { number };
        let key = format!("<{kind} {number}>");
        let replacement = match aliases.get(&key) {
            Some(value) => py_str(Some(value)),
            None if kind == "Subject" => format!("Subject {number}"),
            None => return Err(format!("Cannot map {key} to an actual reference. Check this scene's reference assignments.")),
        };
        retagged.push_str(&text[last..whole.start()]);
        retagged.push_str(&replacement);
        last = whole.end();
    }
    retagged.push_str(&text[last..]);
    let text = retagged
        .replace("fully_preserved", "preserve exactly")
        .replace("fully_copy", "retain without alteration in final assembly")
        .replace("[reference generation + audio reuse]", "[Reference-guided generation; original soundtrack retained in final assembly]");
    let text = clean_performance_syntax(&text);

    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut hint = "Scene".to_string();
    let flush = |blocks: &mut Vec<Block>, current: &mut Vec<String>, hint: &str| {
        if !current.is_empty() {
            blocks.push(Block { id: format!("b{}", blocks.len() + 1), heading_hint: hint.to_string(), text: current.join("\n") });
            current.clear();
        }
    };
    for line in py_splitlines(&text) {
        let heading_key = rx!(r"(?i)^\s*([a-z_ ]+):\s*$")
            .captures(&line)
            .ok()
            .flatten()
            .map(|found| group(&found, 1).trim().to_lowercase())
            .unwrap_or_default();
        let destination = SOURCE_HEADINGS
            .iter()
            .find(|(source, _)| *source == heading_key)
            .map(|(_, heading)| heading.to_string())
            .or_else(|| HEADINGS.iter().find(|heading| heading.to_lowercase() == heading_key).map(|heading| heading.to_string()))
            .filter(|_| !heading_key.is_empty());
        if let Some(destination) = destination {
            flush(&mut blocks, &mut current, &hint);
            hint = destination;
        } else if line.trim().is_empty() {
            flush(&mut blocks, &mut current, &hint);
        } else {
            current.push(line);
        }
    }
    flush(&mut blocks, &mut current, &hint);
    let shot_text = rx!(r"(?i)\[?Shot \d+");
    for block in &mut blocks {
        if block.heading_hint == "Shots" || is_match(shot_text, &block.text) {
            block.text = normalize_shot_safety(&block.text);
        }
    }
    let scene_audio = scene.get("use_scene_audio").is_some_and(truthy);
    if scene_audio {
        if audio_paths {
            return Err("Scene-audio mode requires an empty audio-reference file list so the scene excerpt can be Audio 1.".into());
        }
        for block in &mut blocks {
            if block.heading_hint == "Shots" || is_match(rx!(r"(?i)\[Shot \d+\]|(?:sings?|speaks?) in sync|lip.sync"), &block.text) {
                block.text = block.text.replace("Original soundtrack", "@Audio 1");
            }
        }
        // On reconversion this definition is already an immutable source block.
        if !blocks.iter().any(|block| block.text.contains(SCENE_AUDIO_DEFINITION)) {
            blocks.insert(0, Block { id: "scene_audio".into(), heading_hint: "References".into(), text: SCENE_AUDIO_DEFINITION.into() });
        }
        let narration: Vec<String> = blocks
            .iter()
            .filter(|block| block.heading_hint == "Shots" || is_match(shot_text, &block.text))
            .map(|block| sub(rx!(QUOTED), &block.text, ""))
            .collect();
        let sings = is_match(rx!(r"(?i)\b(?:sings?|singing|speaks?|speaking|lip[- ]?syncs?)\b"), &narration.join("\n"));
        if sings && !blocks.iter().any(|block| block.text.contains(VOCAL_ROLE_GUIDANCE)) {
            blocks.push(Block { id: "vocal_roles".into(), heading_hint: "Sound".into(), text: VOCAL_ROLE_GUIDANCE.into() });
        }
    }
    let videos = count_of(scene, "video_paths");
    let audios = count_of(scene, "audio_paths");
    for block in &blocks {
        for found in rx!(r"@(Image|Video|Audio)\s+(\d+)").captures_iter(&block.text).filter_map(Result::ok) {
            let (kind, number) = (group(&found, 1), group(&found, 2));
            let limit = match kind.as_str() {
                "Image" => images,
                "Audio" if scene_audio => 1,
                "Video" => videos,
                _ => audios,
            };
            let value: usize = number.parse().unwrap_or(usize::MAX);
            if !(1..=limit).contains(&value) {
                return Err(format!("@{kind} {number} does not have an attached reference. Check the scene's reference assignments."));
            }
        }
    }
    if blocks.is_empty() {
        return Err("No prompt content remains to format.".into());
    }
    Ok(blocks)
}

/// Lay the blocks out under the LLM's headings. Every block must come back once and in order, so nothing is lost.
pub fn assemble_seedance_prompt(blocks: &[Block], plan: &Value) -> Result<String, String> {
    let sections = plan.get("sections").and_then(Value::as_array).filter(|sections| !sections.is_empty()).ok_or("The LLM did not return a Seedance formatting plan. No prompts were changed.")?;
    let invalid = "The LLM returned an invalid formatting section. No prompts were changed.";
    let mut ids: Vec<String> = Vec::new();
    let mut layout: Vec<(String, Vec<String>)> = Vec::new();
    for section in sections {
        let section = section.as_object().ok_or(invalid)?;
        let heading = section.get("heading").and_then(Value::as_str).filter(|heading| HEADINGS.contains(heading)).ok_or(invalid)?;
        let wanted = section.get("block_ids").and_then(Value::as_array).filter(|ids| !ids.is_empty()).ok_or(invalid)?;
        let wanted: Vec<String> = wanted.iter().map(|id| py_str(Some(id))).collect();
        ids.extend(wanted.iter().cloned());
        layout.push((heading.to_string(), wanted));
    }
    if ids != blocks.iter().map(|block| block.id.clone()).collect::<Vec<_>>() {
        return Err("The LLM dropped, duplicated, or reordered source content. No prompts were changed.".into());
    }
    let text_of = |id: &str| blocks.iter().find(|block| block.id == id).map(|block| block.text.as_str()).unwrap_or_default();
    let prompt = layout
        .iter()
        .map(|(heading, ids)| format!("{heading}:\n{}", ids.iter().map(|id| text_of(id)).collect::<Vec<_>>().join("\n\n")))
        .collect::<Vec<_>>()
        .join("\n\n");
    if prompt.chars().count() > 15000 {
        return Err("The complete converted prompt exceeds Seedance's 15,000-character limit. Nothing was truncated.".into());
    }
    Ok(prompt)
}

/// Bind `@Audio 1` to the prepared scene excerpt in the prompt, without claiming a deterministic lip-sync API
/// (`_seedance_scene_audio_prompt`). A prompt that is not in scene-audio mode is returned as it is.
pub fn scene_audio_prompt(text: &str, payload: &Obj) -> Result<String, String> {
    if payload.get("audio_mode").and_then(Value::as_str) != Some("scene_audio") {
        return Ok(text.to_string());
    }
    let audios = payload.get("reference_audios").and_then(Value::as_array).map_or(0, Vec::len);
    if audios != 1 {
        return Err("Scene-audio mode requires exactly one prepared project/custom audio excerpt as Audio 1.".into());
    }
    let image = match payload.get("lip_sync_image").filter(|value| truthy(value)) {
        Some(value) => crate::pyval::to_int(value).ok_or("Select a lip-sync performer image that is actually attached to this scene.")?,
        None => 0,
    };
    let images = payload.get("reference_images").and_then(Value::as_array).map_or(0, Vec::len) as i64;
    if image < 0 || image > images {
        return Err("Select a lip-sync performer image that is actually attached to this scene.".into());
    }
    let performance = if payload.get("no_lip_sync").is_some_and(truthy) {
        "This is a non-vocal/instrumental performance shot. Keep every person's lips closed; use the audio only for musical timing and body/instrument movement.".to_string()
    } else if image != 0 {
        format!(
            "The performer from @Image {image} lip-syncs to the actual vocals/speech in @Audio 1. Match the supplied words, phrasing, phonemes, melody and pauses at their original timestamps. Keep the mouth visible during vocal lines. Other people keep their lips closed unless the scene explicitly assigns them a vocal part."
        )
    } else {
        "Use the existing scene's singer/speaker assignments to lip-sync to the actual vocals/speech in @Audio 1. Match the supplied words, phrasing, phonemes and pauses at their original timestamps. Keep the assigned singer's face and mouth readable during vocal phrases. Keep non-singers' lips closed and preserve all instrumental-only/lips-closed shot instructions.".to_string()
    };
    Ok(format!(
        "{text}\n\nScene audio timing:\n@Audio 1 is the exact project/custom audio excerpt for this scene, starting at scene time 0. Use its original timing and tempo; do not rewrite words, invent vocals, or stretch or retime the performance. Any trailing silence is padding after the scene ends. {performance} The Builder restores the supplied audio to the finished scene and uses the project soundtrack in final assembly."
    ))
}
