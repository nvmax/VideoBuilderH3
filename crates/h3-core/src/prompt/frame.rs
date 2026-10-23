//! Reference definitions around the shot text, prompt validation and picture labels. Port of the reference part of
//! `minimax/shot_prompt.py`. Items are the scene's ordered reference images (`scene::ordered_reference_items`).

use serde::Serialize;
use serde_json::Value;

use super::plan::{shot_plan, HARD_LIMIT};
use super::rx::{find_all, find_all_group, group, is_match, rx, sub, sub_with};
use super::text::assemble_prompt;
use crate::error::CoreError;
use crate::pyval::{text_or_empty, truthy, Obj};

pub const AUDIO_DEFINITION: &str = concat!(
    "<Audio 1> is the complete synchronized song and vocal track for the target video, reused as the target video's ",
    "complete final soundtrack and timing reference."
);
const DEFAULT_SOUNDSCAPE: &str =
    "Subtle location-appropriate ambience, physical movement sounds, breathing, and expressive performance sounds support the scene.";

fn purpose_for(kind: &str) -> Option<&'static str> {
    match kind {
        "subject" => Some("character identity, face, hair, clothing, and body-proportion reference"),
        "extra" => Some("character identity, face, hair, clothing, and body-proportion reference for this non-speaking extra"),
        "location" => Some("environment, location, architecture, layout, and atmosphere reference"),
        "ingredients" => Some("ordered ingredients, props, identity, or visual-detail reference"),
        _ => None,
    }
}

fn kind_of(item: &Obj) -> String {
    text_or_empty(item.get("kind"))
}

fn first_text(item: &Obj, keys: &[&str]) -> String {
    let pick = keys.iter().filter_map(|k| item.get(*k)).find(|v| truthy(v));
    text_or_empty(pick)
}

/// A reference description cut at a clause or word boundary.
pub fn compact_description(text: Option<&Value>, limit: usize) -> String {
    let clean = sub(rx!(r"\s+"), &text_or_empty(text), " ");
    let clean = sub(rx!(r"[.;,\s]+$"), clean.trim(), "");
    let chars: Vec<char> = clean.chars().collect();
    if chars.len() <= limit {
        return clean;
    }
    let window: Vec<char> = chars[..(limit + 1).min(chars.len())].to_vec();
    let rfind = |targets: &[char]| window.iter().rposition(|c| targets.contains(c)).map(|i| i as isize).unwrap_or(-1);
    let comma = rfind(&[',']).max(rfind(&[';']));
    let cut = if comma >= (limit as f64 * 0.55) as isize { comma } else { rfind(&[' ']) };
    let end = if cut > 0 { cut as usize } else { limit };
    let head: String = window[..end].iter().collect();
    sub(rx!(r"[.;,\s]+$"), &head, "").trim().to_string()
}

/// The text a reference-to-video prompt needs around its `detailed_description` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Frame {
    pub head: String,
    pub tail: String,
}

pub fn reference_frame(items: &[Obj], cut_plan: &Obj, style: &str, audio_mode: &str, audio_direction: &str) -> Frame {
    let built_in = audio_mode == "built_in_audio";
    let plan = shot_plan(cut_plan);
    let shots = plan.iter().map(|s| format!("[Shot {}]", s.number)).collect::<Vec<_>>().join(", ");
    let shots = if shots.is_empty() { "[Shot 1]".to_string() } else { shots };
    let mut definitions: Vec<String> = Vec::new();
    let mut retention: Vec<String> = Vec::new();
    let mut featuring: Vec<String> = Vec::new();
    let mut number = 0;
    for (zero, item) in items.iter().enumerate() {
        let index = zero + 1;
        let kind = kind_of(item);
        let picture = format!("<Picture {index}>");
        if kind == "start_frame" {
            definitions.push(format!("{picture} is the first frame of [Shot 1], used as the exact opening composition and visual-state anchor."));
            retention.push(format!("{picture} ([Shot 1] first frame): fully_preserved - the opening composition, subject placement, environment, lighting, and visual state are retained as the starting frame."));
            continue;
        }
        if kind == "end_frame" {
            let last = plan.last().map_or(1, |s| s.number);
            definitions.push(format!("{picture} is the last frame of [Shot {last}], used as the exact final composition and visual-state anchor."));
            retention.push(format!("{picture} ([Shot {last}] last frame): fully_preserved - the final composition, subject placement, environment, lighting, and visual state are reached exactly at the end."));
            continue;
        }
        number += 1;
        let label = format!("<Subject {number}>");
        let raw_name = sub(rx!(r"\s+"), &first_text(item, &["label", "name"]), " ").trim().to_string();
        let raw_name = if raw_name.is_empty() { if kind == "location" { "location".to_string() } else { "reference".to_string() } } else { raw_name };
        let name = if kind == "location" {
            "environment".to_string()
        } else {
            sub(rx!(r"(?i)^(?:the\s+)+"), &raw_name, "the ")
        };
        let noun = if matches!(kind.as_str(), "subject" | "extra" | "location") { name.clone() } else { format!("{name} reference") };
        let phrase = if is_match(rx!(r"(?i)^(?:the|a|an)\s+"), &noun) { noun.clone() } else { format!("the {noun}") };
        let purpose = match item.get("purpose").filter(|v| truthy(v)) {
            Some(v) => text_or_empty(Some(v)),
            None => purpose_for(&kind).unwrap_or("visual reference").to_string(),
        };
        let description = compact_description(item.get("description"), 240);
        if kind == "subject" && number == 1 {
            definitions.push(format!("{label} is {phrase} in {picture}; {picture} is the visual authority for {purpose}."));
        } else {
            let tail = if description.is_empty() { ".".to_string() } else { format!(": {description}.") };
            definitions.push(format!("{label} is {phrase} in {picture}, used as {purpose}{tail}"));
        }
        let keep = if kind == "subject" || kind == "extra" {
            if kind == "subject" { "reference identity, face, hair, wardrobe, and accessories".to_string() } else { "identity and wardrobe".to_string() }
        } else {
            let short = compact_description(item.get("description"), 140);
            format!("{} and its reference identity", if short.is_empty() { name.clone() } else { short })
        };
        retention.push(format!("{label} (appears in {shots}): fully_preserved - {keep} remain consistent."));
        featuring.push(format!("{label} ({name})"));
    }
    let mut tasks: Vec<&str> = Vec::new();
    if items.iter().any(|i| kind_of(i) == "start_frame") {
        tasks.push("keyframe completion");
    }
    tasks.push("reference generation");
    if !built_in {
        tasks.push("audio reuse");
        definitions.push(AUDIO_DEFINITION.to_string());
        retention.push("<Audio 1>: fully_copy - <Audio 1> is reused 1:1 as the target video's complete final audio track.".to_string());
    }
    let subject_text = if featuring.is_empty() { "the described target scene".to_string() } else { featuring.join(" and ") };
    let audio_text = if built_in {
        " MiniMax generates the native audio requested by the scene."
    } else {
        " <Audio 1> is reused as the complete soundtrack and timing reference."
    };
    let style_text = if style.is_empty() { "photorealistic cinematic" } else { style };
    let head = format!(
        "subject_definitions:\n{}\n\nsummary:\n[{}] The target video is a {} scene featuring {}.{}\n\nretention_analysis:\n{}\n\n",
        definitions.join("\n"),
        tasks.join(" + "),
        style_text,
        subject_text,
        audio_text,
        retention.join("\n")
    );
    let tail = if built_in {
        let direction = audio_direction.trim();
        format!(
            "\n\noverall_soundscape:\n{}\n\nnon_diegetic_music:\nThe scene's native soundtrack follows the requested musical and atmospheric direction.",
            if direction.is_empty() { DEFAULT_SOUNDSCAPE } else { direction }
        )
    } else {
        concat!(
            "\n\noverall_soundscape:\n<Audio 1> remains the sole complete audience-facing soundtrack with its original ",
            "mix, timing, vocals, music, and dynamics intact.\n\nnon_diegetic_music:\n<Audio 1> is reused as the ",
            "complete audience-facing song/music track."
        )
        .to_string()
    };
    Frame { head, tail }
}

/// The saved prompt with its reference definitions and soundscape. A prompt that already has them is returned as is.
pub fn wrap_reference_prompt(prompt: &str, frame: &Frame) -> String {
    let text = prompt.trim();
    if text.is_empty() || text.contains("subject_definitions:") {
        return text.to_string();
    }
    format!("{}{}{}", frame.head, text, frame.tail)
}

/// How many characters the shot descriptions may use in total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CharacterBudget {
    pub hard_limit: i64,
    pub target_limit: i64,
    pub fixed_chars: i64,
    pub shot_chars: i64,
    pub shot_count: i64,
}

pub fn character_budget(cut_plan: &Obj, style: &str, target_limit: i64) -> CharacterBudget {
    let hard = HARD_LIMIT as i64;
    let target = target_limit.clamp(0, hard);
    let count = shot_plan(cut_plan).len();
    let blanks = vec![String::new(); count];
    let fixed = assemble_prompt(&blanks, cut_plan, style, "").map_or(0, |p| p.chars().count() as i64);
    CharacterBudget {
        hard_limit: hard,
        target_limit: target,
        fixed_chars: fixed,
        shot_chars: (target - fixed).max(0),
        shot_count: count as i64,
    }
}

/// Fail unless the prompt follows the H3 format. Returns the trimmed prompt.
pub fn validate_prompt(prompt: &str, cut_plan: &Obj) -> Result<String, CoreError> {
    let text = prompt.trim();
    if text.is_empty() {
        return Err(CoreError::shot("The assembled MiniMax H3 prompt is empty.", "EMPTY_PROMPT"));
    }
    let anchors = [
        rx!(r"(?i)\b(?:in|from|matching|copying|reproducing|preserving|using|following)\s+(?:the\s+)?(?:exact\s+)?(?:composition|framing|camera\s+angle|pose)\s+(?:of|from|in)\s+<Picture\s+\d+>"),
        rx!(r"(?i)<Picture\s+\d+>\s+(?:is|defines|controls|sets)\s+(?:the\s+)?(?:exact\s+)?(?:first|start|opening)\s+(?:frame|composition|framing)"),
        rx!(r"(?i)\b(?:begin|begins|start|starts|open|opens)\s+(?:exactly\s+)?(?:from|on|with)\s+<Picture\s+\d+>"),
    ];
    if anchors.iter().any(|p| is_match(p, text)) {
        return Err(CoreError::shot(
            "Reference to Video cannot use a reference picture as the opening frame or composition.",
            "MINIMAX_H3_REFERENCE_COMPOSITION_LEAK",
        ));
    }
    let length = text.chars().count();
    if length > HARD_LIMIT {
        let mut e = CoreError::shot(
            format!("The MiniMax H3 prompt is {length} characters, over the {HARD_LIMIT} maximum by {}.", length - HARD_LIMIT),
            "MINIMAX_H3_PROMPT_TOO_LONG",
        );
        e.length = length as i64;
        return Err(e);
    }
    let marker = "detailed_description:";
    let Some(at) = text.find(marker) else {
        return Err(CoreError::shot("The MiniMax H3 prompt is missing the detailed_description section.", "MISSING_SECTION"));
    };
    let creative = &text[at + marker.len()..];
    let plan = shot_plan(cut_plan);
    for shot in &plan {
        let found = fancy_regex::Regex::new(&format!(r"\[Shot\s+{}\]", shot.number))
            .map(|re| find_all(&re, creative).len())
            .unwrap_or(0);
        if found != 1 {
            return Err(CoreError::shot(format!("Expected exactly one [Shot {}] block, found {}.", shot.number, found), "SHOT_COUNT"));
        }
        if shot.number > 1 {
            let pattern = format!(r"\[Shot\s+{}\]\s+At\s+{},", shot.number, fancy_regex::escape(&shot.timecode));
            let ok = fancy_regex::Regex::new(&pattern).map(|re| is_match(&re, creative)).unwrap_or(false);
            if !ok {
                return Err(CoreError::shot(format!("[Shot {}] must begin with At {},.", shot.number, shot.timecode), "SHOT_TIMING"));
            }
        }
    }
    let unexpected: Vec<String> = find_all_group(rx!(r"\[Shot\s+(\d+)\]"), creative, 1)
        .into_iter()
        .filter(|m| m.parse::<usize>().is_ok_and(|n| n > plan.len()))
        .collect();
    if let Some(first) = unexpected.first() {
        return Err(CoreError::shot(format!("The MiniMax H3 prompt contains unexpected [Shot {first}]."), "SHOT_COUNT"));
    }
    let last = format!("[Shot {}]", plan.last().map_or(1, |s| s.number));
    let final_text = creative.find(&last).map(|i| creative[i + last.len()..].trim().to_string()).unwrap_or_default();
    if final_text.chars().count() < 24 || !is_match(rx!(r"[.!?…]\s*$"), &final_text) {
        return Err(CoreError::shot("The MiniMax H3 prompt has an incomplete final shot description.", "INCOMPLETE_FINAL_SHOT"));
    }
    if is_match(rx!(r"(?im)<(?:Subject|Picture|Video|Audio)\b[^>\n]{0,40}(?:$|\n)"), text) {
        return Err(CoreError::shot("The MiniMax H3 prompt contains an incomplete reference label.", "BAD_REFERENCE_LABEL"));
    }
    Ok(text.to_string())
}

/// A `<Subject N>` label for a reference picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefLabel {
    pub label: String,
    pub picture: String,
    pub kind: String,
    pub name: String,
}

/// Labels for the ordered reference images. Start and end frames are pictures, not subjects.
pub fn reference_labels(items: &[Obj]) -> Vec<RefLabel> {
    let mut labels = Vec::new();
    let mut number = 0;
    for (zero, item) in items.iter().enumerate() {
        let kind = kind_of(item);
        if kind == "start_frame" || kind == "end_frame" {
            continue;
        }
        number += 1;
        labels.push(RefLabel {
            label: format!("<Subject {number}>"),
            picture: format!("<Picture {}>", zero + 1),
            kind: match kind.as_str() {
                "location" => "location".into(),
                "extra" => "extra".into(),
                _ => "subject".into(),
            },
            name: first_text(item, &["label", "name"]).trim().to_string(),
        });
    }
    labels
}

/// Keep used character and picture assignments outside the shot action.
pub fn character_reference_bindings(creative: &str, items: &[Obj]) -> String {
    let mut bindings = Vec::new();
    let mut number = 0;
    for (zero, item) in items.iter().enumerate() {
        let kind = kind_of(item);
        if kind == "start_frame" || kind == "end_frame" {
            continue;
        }
        number += 1;
        let label = format!("<Subject {number}>");
        if !(kind == "subject" || kind == "extra") || !creative.contains(&label) {
            continue;
        }
        let purpose = match item.get("purpose").filter(|v| truthy(v)) {
            Some(v) => text_or_empty(Some(v)),
            None => purpose_for(&kind).unwrap_or("character appearance reference").to_string(),
        };
        let purpose = purpose.split(';').next().unwrap_or("").to_string();
        let purpose = sub(rx!(r"(?i)\s+reference(?:\s+only)?(?:\s+for.*)?$"), &purpose, "");
        bindings.push(format!("Use <Picture {}> for {label}'s {purpose}.", zero + 1));
    }
    bindings.join(" ")
}

/// Bind pictures within the shot prose, without a reference-definition paragraph.
pub fn compact_reference_prompt(prompt: &str, items: &[Obj]) -> String {
    let labels = reference_labels(items);
    let shot_start = rx!(r"\[Shot\s+\d+\]");
    // `re.split(r"(?=\[Shot\s+\d+\])", ...)`: cut before every shot label.
    let mut cuts: Vec<usize> = shot_start.find_iter(prompt).filter_map(Result::ok).map(|m| m.start()).collect();
    cuts.insert(0, 0);
    cuts.dedup();
    let mut blocks: Vec<String> = Vec::new();
    for (i, start) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).copied().unwrap_or(prompt.len());
        blocks.push(prompt[*start..end].to_string());
    }
    for block in blocks.iter_mut() {
        if !shot_start.find(block).ok().flatten().is_some_and(|m| m.start() == 0) {
            continue;
        }
        let mut text = block.trim().to_string();
        for item in &labels {
            if item.kind != "location" {
                let pattern = format!(r"{}\s*\(([^)]*)\)", fancy_regex::escape(&item.label));
                if let Ok(re) = fancy_regex::Regex::new(&pattern) {
                    text = sub_with(&re, &text, |c| {
                        let inner = group(c, 1);
                        if is_match(rx!(r"(?i)^\s*S\d+\s*$"), &inner) { group(c, 0) } else { item.label.clone() }
                    });
                }
            } else if !text.contains(&item.picture) {
                text.push_str(&format!(" The action takes place in the environment from {}.", item.picture));
            }
        }
        *block = text;
    }
    blocks.iter().map(|b| b.trim().to_string()).filter(|b| !b.is_empty()).collect::<Vec<_>>().join("\n\n")
}
