//! Cleaning a model's reply. Port of `_strip_builder_thinking_text` and `_clean_lm_studio_plain_text`.

use fancy_regex::Regex;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

/// Remove reasoning or control text a model emits although thinking is off.
pub fn strip_thinking_text(text: &str) -> String {
    let mut cleaned = text.replace("\r\n", "\n").replace('\r', "\n").trim().to_string();
    cleaned = rx(r"(?is)<(?:think|thought)>.*?</(?:think|thought)>").replace_all(&cleaned, "").into_owned();
    if let Ok(Some(closing)) = rx(r"(?i)</(?:think|thought)>").find(&cleaned) {
        cleaned = cleaned[closing.end()..].trim().to_string();
    }
    let prefix = rx(
        r"(?i)^\s*(?:<\|channel\|>\s*)?(?:<\|(?:thought|thinking|analysis|reasoning)\|>\s*)?(?:thought(?: process)?|thinking(?: process)?|analysis|reasoning)\s*:\s*",
    );
    let thought_prefix = prefix.find(&cleaned).ok().flatten().map(|m| m.end());
    let final_marker = rx(r"(?i)\n\s*(?:final(?: answer| response)?|answer|response)\s*:\s*").find(&cleaned).ok().flatten().map(|m| (m.start(), m.end()));
    match (thought_prefix, final_marker) {
        (Some(prefix_end), Some((start, end))) if start >= prefix_end => cleaned = cleaned[end..].trim().to_string(),
        _ => cleaned = prefix.replace(&cleaned, "").trim().to_string(),
    }
    cleaned
}

/// A reply as plain text: no thinking, no code fence, no leading `Answer:`.
pub fn clean_plain_text(text: &str) -> String {
    let mut cleaned = strip_thinking_text(text);
    cleaned = rx(r"(?i)^\s*```(?:text|json)?\s*").replace(&cleaned, "").trim().to_string();
    cleaned = rx(r"\s*```\s*$").replace(&cleaned, "").trim().to_string();
    cleaned = rx(r"(?i)^(?:Assistant|Answer|Final answer)\s*:\s*").replace(&cleaned, "").trim().to_string();
    cleaned
}
