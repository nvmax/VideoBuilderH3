//! MiniMax H3 "Enhance Prompts": put an LLM's rewritten `[Shot N]` blocks back into the original prompt. Port of `minimax/prompt_enhance.py`.
//!
//! The LLM sees the whole finished prompt, but only its shot blocks are taken from the reply. Everything before the first shot and after the
//! last one stays as it was, except for the scene-wide continuity paragraph and duplicate music-video wording in the style line. A rewrite that
//! loses a label, a cue time, a sung word or a "from X s to Y s" timing is refused, so a bad reply never replaces a working prompt. The
//! separate reference-image task may also replace mapped visual definitions before this same shot validation.

use std::collections::BTreeSet;

use fancy_regex::{Captures, Regex};
use serde_json::Value;

use super::plan::HARD_LIMIT;
use crate::pyval::{text_or_empty, truthy};

pub const CONTINUOUS_PERFORMANCE: &str = "All shots follow one continuous performance, preserving every person's exact position, body orientation, posture, and ongoing movement across every camera cut; each new shot continues directly from the final physical moment of the previous shot, with only the camera angle and framing changing.";
pub const CONTINUATION: &str = "continuing directly from the previous shot's final moment,";

/// The enhanced prompt broke the prompt's fixed parts. The original prompt is kept.
#[derive(Debug, Clone, PartialEq)]
pub struct EnhanceError(pub String);

impl std::fmt::Display for EnhanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<EnhanceError> for String {
    fn from(error: EnhanceError) -> Self {
        error.0
    }
}

type Outcome<T> = Result<T, EnhanceError>;

fn fail<T>(message: impl Into<String>) -> Outcome<T> {
    Err(EnhanceError(message.into()))
}

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

// Definitions and retention may mention [Shot N] inline. Only a line-opening label begins a shot block.
const SHOT_LABEL: &str = r"(?m)^[ \t]*\[Shot\s+(\d+)\]";
const REFERENCE: &str = r"<(?:Subject|Picture|Audio|Video)\s+\d+>";

fn labels_in(text: &str) -> BTreeSet<String> {
    rx(REFERENCE).find_iter(text).flatten().map(|m| m.as_str().to_string()).collect()
}

fn timings_in(text: &str) -> Vec<String> {
    rx(r"(?i)\bfrom\s+\d+(?:\.\d+)?\s*s\s+to\s+\d+(?:\.\d+)?\s*s\b").find_iter(text).flatten().map(|m| m.as_str().to_lowercase()).collect()
}

/// Sung words only: tags, emotion headers and punctuation removed.
fn words(text: &str) -> Vec<String> {
    let plain = rx(r"\[[^\]]*\]|<[^>]*>").replace_all(text, " ").to_lowercase();
    rx(r"[a-z0-9]+(?:['\u{2019}][a-z0-9]+)*").find_iter(&plain).flatten().map(|m| m.as_str().to_string()).collect()
}

fn cues_in(text: &str) -> Vec<Vec<String>> {
    rx(r"(?i)<d>([\s\S]*?)</d>").captures_iter(text).flatten().map(|c| words(&c[1])).collect()
}

fn quotes_in(text: &str) -> Vec<Vec<String>> {
    rx("\"([^\"\\n]+)\"|\u{201c}([^\u{201d}\\n]+)\u{201d}").captures_iter(text).flatten().map(|c| words(c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str()))).collect()
}

fn shot_number(block: &str) -> i64 {
    rx(SHOT_LABEL).captures(block).ok().flatten().and_then(|c| c[1].parse().ok()).unwrap_or(0)
}

/// `split_prompt`: the text before the shots, the `[Shot N]` blocks, and the text after them.
pub fn split_prompt(text: &str) -> Outcome<(String, Vec<String>, String)> {
    let source = text.replace("\r\n", "\n");
    let label = rx(SHOT_LABEL);
    let Some(first) = label.find(&source).ok().flatten() else { return fail("The prompt has no [Shot N] blocks to enhance.") };
    let rest = &source[first.start()..];
    let section = rx(r"\n\s*\n(?=[a-z][a-z_]*:[ \t]*(?:\n|$))");
    let (body, suffix) = match section.find(rest).ok().flatten() {
        Some(end) => (&rest[..end.start()], &rest[end.start()..]),
        None => (rest, ""),
    };
    let starts: Vec<usize> = label.find_iter(body).flatten().map(|m| m.start()).collect();
    let mut blocks = Vec::new();
    let mut cuts = starts.clone();
    cuts.push(body.len());
    if let Some(&head) = starts.first().filter(|s| **s > 0) {
        blocks.push(body[..head].trim().to_string());
    }
    for pair in cuts.windows(2) {
        blocks.push(body[pair[0]..pair[1]].trim().to_string());
    }
    blocks.retain(|b| !b.is_empty());
    Ok((source[..first.start()].to_string(), blocks, suffix.to_string()))
}

fn clean_reply(text: &str) -> String {
    let reply = rx(r"(?i)```[a-z]*").replace_all(text, "").into_owned();
    rx(r"\*\*(\[Shot\s+\d+\])\*\*").replace_all(&reply, "$1").replace("\r\n", "\n").trim().to_string()
}

const LEAD: &str = r"^\[Shot\s+\d+\]\s*(At\s+\d{1,2}:\d{2}(?:\.\d+)?,)?\s*";

/// Give the rewritten shot the original "[Shot N] At 00:00.000," opening.
fn keep_lead(original: &str, enhanced: &str, multi_shot: bool, first: bool) -> String {
    let lead = rx(LEAD).find(original).ok().flatten().map_or("", |m| m.as_str()).trim().to_string();
    let mut body = rx(LEAD).replace(enhanced, "").trim().to_string();
    if multi_shot {
        if first {
            // Migrate older replies that placed the global rule inside Shot 1.
            while body.starts_with(CONTINUOUS_PERFORMANCE) {
                body = body[CONTINUOUS_PERFORMANCE.len()..].trim().to_string();
            }
        } else {
            let rest = rx(r"(?i)^continuing\s+directly\s+from\s+the\s+previous\s+shot['\u{2019}]s\s+final\s+moment\s*,?\s*").replace(&body, "").into_owned();
            body = format!("{CONTINUATION} {rest}");
        }
    }
    format!("{lead} {body}")
}

/// Keep the global rule after the detailed-description opening, before Shot 1.
fn scene_opening(prefix: &str, multi_shot: bool) -> String {
    let style_line = rx(r"(?m)^(The target video is in a [^\n]*)$");
    let doubled = rx(r"(?i)\bmusic[ -]video(?:\s+music[ -]video)+(?=\s+style\b)");
    let mut prefix = style_line.replace_all(prefix, |caps: &Captures| doubled.replace_all(&caps[0], "music-video").into_owned()).into_owned();
    if multi_shot {
        prefix = prefix.replace(CONTINUOUS_PERFORMANCE, "").trim_end().to_string();
        prefix = format!("{}{CONTINUOUS_PERFORMANCE}\n\n", if prefix.is_empty() { String::new() } else { format!("{prefix}\n\n") });
    }
    prefix
}

fn check_shot(number: i64, original: &str, enhanced: &str, allowed: &BTreeSet<String>) -> Outcome<()> {
    let where_ = format!("[Shot {number}]");
    let (before, after) = (labels_in(original), labels_in(enhanced));
    let missing: Vec<&String> = before.difference(&after).collect();
    if !missing.is_empty() {
        return fail(format!("{where_} lost {}.", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let unknown: Vec<&String> = after.difference(allowed).collect();
    if !unknown.is_empty() {
        return fail(format!("{where_} added {}, which the prompt does not define.", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if timings_in(original) != timings_in(enhanced) {
        return fail(format!("{where_} changed a 'from X s to Y s' timing."));
    }
    if cues_in(original) != cues_in(enhanced) {
        return fail(format!("{where_} changed the sung or spoken words inside <d>...</d>."));
    }
    let enhanced_quotes = quotes_in(enhanced);
    if quotes_in(original).iter().any(|q| !enhanced_quotes.contains(q)) {
        return fail(format!("{where_} changed a quoted lyric."));
    }
    Ok(())
}

/// `merge_enhanced_prompt`: the original prompt with its shot blocks replaced by the reply's.
pub fn merge_enhanced_prompt(original: &str, reply: &str) -> Outcome<String> {
    let (prefix, old_blocks, suffix) = split_prompt(original)?;
    let cleaned = clean_reply(reply);
    let Some(first) = rx(SHOT_LABEL).find(&cleaned).ok().flatten() else { return fail("The LLM reply has no [Shot N] blocks.") };
    let (_, new_blocks, _) = split_prompt(&cleaned[first.start()..])?;
    let old_numbers: Vec<i64> = old_blocks.iter().map(|b| shot_number(b)).collect();
    let new_numbers: Vec<i64> = new_blocks.iter().map(|b| shot_number(b)).collect();
    if new_numbers != old_numbers {
        return fail(format!("The LLM returned shots {}; the prompt has shots {}.", py_list(&new_numbers), py_list(&old_numbers)));
    }
    let allowed = labels_in(original);
    let mut merged: Vec<String> = Vec::new();
    for (index, ((number, old), new)) in old_numbers.iter().zip(&old_blocks).zip(&new_blocks).enumerate() {
        // Check the actual rewrite before adding boilerplate so an empty reply still fails.
        if rx(LEAD).replace(new, "").trim().chars().count() < 24 {
            return fail(format!("[Shot {number}] came back empty."));
        }
        let new = keep_lead(old, new, old_blocks.len() > 1, index == 0);
        check_shot(*number, old, &new, &allowed)?;
        merged.push(new);
    }
    let last = merged.last().map_or("", String::as_str);
    if !rx("[.!?\u{2026}\"\u{201d}]\\s*$").is_match(last).unwrap_or(false) {
        return fail(format!("[Shot {}] is cut off.", old_numbers.last().copied().unwrap_or(0)));
    }
    let body = merged.join("\n\n");
    let prefix = scene_opening(&prefix, old_blocks.len() > 1);
    let prompt = format!("{prefix}{body}{suffix}").trim().to_string();
    if prompt.chars().count() > HARD_LIMIT {
        return fail(format!("The enhanced prompt is {} characters, over the {HARD_LIMIT} maximum.", prompt.chars().count()));
    }
    Ok(prompt)
}

fn py_list(numbers: &[i64]) -> String {
    format!("[{}]", numbers.iter().map(i64::to_string).collect::<Vec<_>>().join(", "))
}

fn strip_marks(text: &str) -> String {
    rx(r"[<>\[\]\r\n]").replace_all(text, "").into_owned()
}

/// `merge_reference_enhanced_prompt`: allow only mapped visual definitions to change outside the usual validated shot rewrite.
pub fn merge_reference_enhanced_prompt(original: &str, reply: &Value, references: &[Value]) -> Outcome<String> {
    let prompt = match reply.get("prompt") {
        Some(Value::String(p)) if reply.is_object() => p.clone(),
        _ => return fail("Reference enhancement must return JSON with prompt and reference_descriptions."),
    };
    let described: Vec<&Value> = references.iter().filter(|i| i.get("describe").is_some_and(truthy)).collect();
    let descriptions = match reply.get("reference_descriptions") {
        Some(Value::Array(items)) if items.len() == described.len() => items,
        _ => return fail("Reference enhancement did not describe every mapped subject/location exactly once."),
    };
    let mut definitions: Vec<(String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for entry in descriptions {
        let Some(entry) = entry.as_object() else { return fail("Invalid reference description.") };
        let label_value = entry.get("label");
        let label = label_value.and_then(Value::as_str).unwrap_or("").to_string();
        let item = described.iter().find(|i| i.get("label").and_then(Value::as_str) == Some(label.as_str()));
        let (Some(item), false) = (item, seen.contains(&label)) else {
            return fail(format!("Unexpected or repeated reference description: {}.", label_value.map_or("None".to_string(), |v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })));
        };
        seen.push(label.clone());
        let description = match entry.get("description") {
            Some(Value::String(d)) if (12..=450).contains(&d.trim().chars().count()) && !rx(r"[<>\[\]\r\n]").is_match(d).unwrap_or(false) => d.clone(),
            _ => return fail(format!("{label} needs one brief plain-text visual description.")),
        };
        let name = strip_marks(&first_text(item, &["name", "kind"], "reference"));
        let purpose = strip_marks(&first_text(item, &["purpose"], "visual reference"));
        let picture = text_or_empty(item.get("picture_label"));
        definitions.push((label.clone(), format!("{label} is {name} in {picture}: {}. Reference role: {purpose}.", description.trim().trim_end_matches('.'))));
    }
    let (mut prefix, blocks, suffix) = split_prompt(original)?;
    // Compact image+reference prompts may not yet declare their start/end frame anchors. Cover the exact connected picture order when adding
    // subject definitions.
    for item in references {
        let kind = text_or_empty(item.get("kind"));
        if item.get("describe").is_some_and(truthy) || !matches!(kind.as_str(), "start_frame" | "end_frame") {
            continue;
        }
        let label = text_or_empty(item.get("picture_label"));
        if rx(&format!(r"(?m)^{}\s+is\b", fancy_regex::escape(&label))).is_match(&prefix).unwrap_or(false) {
            continue;
        }
        let first_frame = kind == "start_frame";
        let shot = shot_number(if first_frame { &blocks[0] } else { &blocks[blocks.len() - 1] });
        let position = if first_frame { "first" } else { "last" };
        definitions.push((label.clone(), format!("{label} is the {position} frame of [Shot {shot}], used as the exact {position}-frame visual-state anchor.")));
    }
    let section = rx(r"(?m)^subject_definitions:[ \t]*\n").find(&prefix).ok().flatten().map(|m| (m.start(), m.end()));
    if let Some((_, section_end)) = section {
        let rest = &prefix[section_end..];
        let stop = match rx(r"(?m)^[a-z][a-z_]*:[ \t]*(?:\n|$)").find(rest).ok().flatten() {
            Some(end) => section_end + end.start(),
            None => prefix.len(),
        };
        let mut content = prefix[section_end..stop].trim_end().to_string();
        for (label, definition) in &definitions {
            // Replace the mapped definition only. Preserve audio, other definitions and fixed sections.
            let pattern = rx(&format!(r"(?m)^{}[^\n]*$", fancy_regex::escape(label)));
            if pattern.is_match(&content).unwrap_or(false) {
                content = pattern.replace_all(&content, fancy_regex::NoExpand(definition.as_str())).into_owned();
            } else {
                content.push('\n');
                content.push_str(definition);
            }
        }
        prefix = format!("{}{content}\n\n{}", &prefix[..section_end], &prefix[stop..]);
    } else {
        prefix = format!("subject_definitions:\n{}\n\n{prefix}", definitions.iter().map(|(_, line)| line.as_str()).collect::<Vec<_>>().join("\n"));
    }
    let enriched = format!("{prefix}{}{suffix}", blocks.join("\n\n"));
    merge_enhanced_prompt(&enriched, &prompt)
}

fn first_text(item: &Value, keys: &[&str], default: &str) -> String {
    keys.iter().filter_map(|k| item.get(*k)).find(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| default.to_string())
}
