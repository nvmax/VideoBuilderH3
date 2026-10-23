//! Lyric splitting and `<d>` tagging. Port of the lyric part of `minimax/shot_prompt.py`.

use super::cues::cue_text;
use super::rx::{find_all, find_all_group, group, is_match, rx, sub};
use crate::pyval::collapse_ws;

/// The scene's lyric as clean single lines (section tags and blank lines dropped).
pub fn lyric_lines(lyric_text: &str) -> Vec<String> {
    rx!(r"[\r\n]+")
        .split(lyric_text)
        .filter_map(Result::ok)
        .map(collapse_ws)
        .filter(|line| !line.is_empty() && !is_match(rx!(r"^\[[^\]]*\]$"), line))
        .collect()
}

/// Split the lyric lines, in order, into `shot_count` groups. A single shot gets all of it.
pub fn lyric_chunks(lyric_text: &str, shot_count: i64) -> Vec<String> {
    let lines = lyric_lines(lyric_text);
    let count = shot_count.max(1) as usize;
    if lines.is_empty() {
        return vec![String::new(); count];
    }
    if count == 1 {
        return vec![lines.join(" ")];
    }
    let (size, extra) = (lines.len() / count, lines.len() % count);
    let mut chunks = Vec::new();
    let mut start = 0;
    for index in 0..count {
        let take = size + usize::from(index < extra);
        chunks.push(lines[start..start + take].join(" "));
        start += take;
    }
    chunks
}

fn words(text: &str) -> Vec<String> {
    find_all(rx!(r"[a-z0-9']+"), &text.to_lowercase().replace('’', "'"))
}

fn dialogue_tag(lyric: &str) -> String {
    format!("<d>[English] {}</d>", cue_text(lyric.trim().trim_end_matches([' ', ',', ';', ':'])))
}

/// Byte offset of char index `n` in `text`.
fn byte_at(text: &str, n: usize) -> usize {
    text.char_indices().nth(n).map_or(text.len(), |(i, _)| i)
}

/// Turn a quoted or plain copy of `lyric` in the description into a `<d>` tag. `None` when it is not there.
fn tag_existing(description: &str, lyric: &str) -> Option<String> {
    let lyric_words = words(lyric);
    if lyric_words.is_empty() {
        return None;
    }
    let lead = lyric_words.iter().take(6).cloned().collect::<Vec<_>>().join(" ");
    let quoted = rx!(r#"(?i)(\bthe lyric line,?\s*)?["“]([^"”]+)["”],?"#);
    for caps in quoted.captures_iter(description).filter_map(Result::ok) {
        if words(&group(&caps, 2)).join(" ").contains(&lead) {
            let whole = caps.get(0)?;
            return Some(format!(
                "{}{}{}",
                &description[..whole.start()],
                dialogue_tag(&group(&caps, 2)),
                &description[whole.end()..]
            ));
        }
    }
    let pattern = format!("(?i){}", lyric_words.iter().map(|w| fancy_regex::escape(w).into_owned()).collect::<Vec<_>>().join(r"[\W_]*"));
    let re = fancy_regex::Regex::new(&pattern).ok()?;
    // The search runs on a copy with curly apostrophes made straight. Both have the same number of characters.
    let normalized = description.replace('’', "'");
    let m = re.find(&normalized).ok().flatten()?;
    let start_char = normalized[..m.start()].chars().count();
    let end_char = start_char + normalized[m.start()..m.end()].chars().count();
    let (start, end) = (byte_at(description, start_char), byte_at(description, end_char));
    Some(format!("{}{}{}", &description[..start], dialogue_tag(&description[start..end]), &description[end..]))
}

fn normalize_for_match(value: &str) -> String {
    sub(rx!(r"[^\w']+"), &value.to_lowercase().replace('’', "'"), " ").trim().to_string()
}

/// Make every shot carry its lyric as `<d>[English] words.</d>`: tag a quoted or plain copy, or add the sentence.
pub fn ensure_quoted_lyrics(descriptions: &[String], lyric_text: &str, performer: &str) -> Vec<String> {
    let chunks = lyric_chunks(lyric_text, descriptions.len() as i64);
    let mut result = Vec::new();
    for (description, chunk) in descriptions.iter().zip(chunks.iter()) {
        let tagged_lyrics = find_all_group(rx!(r"(?is)<d>\s*\[[^\]]+\]\s*(.*?)</d>"), description, 1);
        let normalized_chunk = normalize_for_match(chunk);
        if !normalized_chunk.is_empty()
            && tagged_lyrics
                .iter()
                .any(|tagged| normalize_for_match(&sub(rx!(r"<[^>]+>"), tagged, "")).contains(&normalized_chunk))
        {
            result.push(description.clone());
            continue;
        }
        if chunk.is_empty() {
            result.push(description.clone());
            continue;
        }
        if let Some(tagged) = tag_existing(description, chunk) {
            result.push(tagged);
            continue;
        }
        let sentence = format!("{performer} sings {} in sync with <Audio 1>.", dialogue_tag(chunk));
        let base = description.trim_end();
        result.push(if base.is_empty() { sentence } else { format!("{base} {sentence}") });
    }
    result
}

/// The last `[Shot N]` description of a saved prompt: what is on screen when the next scene starts.
pub fn last_shot_text(saved_prompt: &str, limit: usize) -> String {
    let marker = rx!(r"\[Shot\s+\d+\]\s*(?:At\s+[0-9:.]+,\s*)?");
    let Some(start) = marker.find_iter(saved_prompt).filter_map(Result::ok).last().map(|m| m.end()) else {
        return String::new();
    };
    let tail = &saved_prompt[start..];
    let block = tail.split("\n\n").next().unwrap_or("");
    let block = collapse_ws(block);
    if block.chars().count() > limit {
        format!("{}...", block.chars().take(limit).collect::<String>().trim_end())
    } else {
        block
    }
}
