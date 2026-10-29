//! The pure steps of the Story Arc: reading the song's sections out of the lyrics, matching scenes to sections, and checking the model's reply
//! against the required headings. Port of the helpers in `storyboard/story_layer.py`.

use fancy_regex::Regex;
use serde_json::Value;

use crate::pyval::{collapse_ws, text_or_empty, truthy, Obj};

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn is_match(pattern: &str, text: &str) -> bool {
    rx(pattern).is_match(text).unwrap_or(false)
}

const STRUCTURAL: &str = r"(?i)\A(?:intro|verse|pre[\s-]?chorus|chorus|post[\s-]?chorus|bridge|outro|refrain|hook|breakdown|drop|interlude|instrumental(?:\s+break)?|solo|break|spoken(?:\s+word)?|rap)(?:\s+(?:\d+|[ivxlcdm]+))?\z";
const ANNOTATION: &str = r"(?i)\A(?:whispered|spoken|sung|dark atmosphere|building energy|high energy|emotional climax|explosive|quiet arrangement|falling tension|rising tension|silence|soft|loud|gentle|intense|energetic|calm|dramatic|atmospheric)\z";

/// A lyric line as (section label, lyric rest of the line, whether it ends the song). Only a line that starts with `[` can be a header.
fn parse_header_line(raw_line: &str) -> (String, String, bool) {
    let stripped = raw_line.trim();
    if !stripped.starts_with('[') {
        return (String::new(), raw_line.to_string(), false);
    }
    let bracket = rx(r"\A\s*\[([^\]\n]{1,80})\]");
    let mut labels: Vec<String> = Vec::new();
    let mut position = 0;
    while position < stripped.len() {
        let Some(found) = bracket.captures(&stripped[position..]).ok().flatten() else { break };
        labels.push(collapse_ws(&found[1]));
        position += found.get(0).map_or(0, |m| m.end());
    }
    if labels.is_empty() {
        return (String::new(), raw_line.to_string(), false);
    }
    let remainder = stripped[position..].trim().to_string();
    let ends = |label: &str| matches!(label.to_lowercase().as_str(), "end" | "end of song");
    let terminal = labels.iter().any(|l| ends(l));
    let mut structural = labels.iter().find(|l| is_match(STRUCTURAL, l)).cloned().unwrap_or_default();
    if structural.is_empty() {
        let first = &labels[0];
        // Custom section names such as [Part A] stay, common performance and mood tags beside real headers do not.
        if !is_match(ANNOTATION, first) && !ends(first) {
            structural = first.clone();
        }
    }
    let terminal = terminal && structural.is_empty();
    (structural, remainder, terminal)
}

fn join_lines(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("\n").trim().to_string()
}

/// `_parse_story_arc_lyric_sections`: the ordered (display label, body) pairs from the bracketed lyric headers. A section that comes back later is
/// numbered ("Chorus 2").
pub fn parse_lyric_sections(lyrics: &str, collapse_adjacent: bool) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_label = String::new();
    let mut current_lines: Vec<String> = Vec::new();
    let text = lyrics.replace("\r\n", "\n").replace('\r', "\n");
    for raw_line in text.split('\n') {
        let (header, remainder, terminal) = parse_header_line(raw_line);
        if !header.is_empty() {
            if !current_label.is_empty() {
                sections.push((current_label.clone(), current_lines.join("\n").trim().to_string()));
            }
            current_label = header;
            current_lines = if remainder.is_empty() { Vec::new() } else { vec![remainder] };
        } else if terminal {
            if !current_label.is_empty() {
                sections.push((current_label.clone(), current_lines.join("\n").trim().to_string()));
            }
            current_label = String::new();
            current_lines = Vec::new();
        } else if !current_label.is_empty() {
            current_lines.push(remainder);
        }
    }
    if !current_label.is_empty() {
        sections.push((current_label, current_lines.join("\n").trim().to_string()));
    }
    // The timeline repeats a scene's section header before every lyric chunk, so adjacent copies are one real section.
    let mut collapsed: Vec<(String, String)> = Vec::new();
    for (label, body) in sections {
        match collapsed.last_mut() {
            Some((previous, previous_body)) if collapse_adjacent && previous.to_lowercase() == label.to_lowercase() => {
                let merged = join_lines(&[previous_body.as_str(), body.as_str()]);
                *previous_body = merged;
            }
            _ => collapsed.push((label, body)),
        }
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut numbered = Vec::new();
    for (label, body) in collapsed {
        let key = label.to_lowercase();
        let occurrence = match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, count)) => {
                *count += 1;
                *count
            }
            None => {
                counts.push((key, 1));
                1
            }
        };
        numbered.push((if occurrence == 1 { label } else { format!("{label} {occurrence}") }, body));
    }
    numbered
}

/// The character index of the last `needle` in `text`, or -1.
fn rfind_char(text: &str, needle: &str) -> i64 {
    text.rfind(needle).map_or(-1, |byte| text[..byte].chars().count() as i64)
}

/// `_cap_story_arc_words`: at most `maximum` words, cut at a sentence end when there is one late enough. A section written as "Scene N" entries
/// keeps one entry per line, each cut to its share.
pub fn cap_words(text: &str, maximum: i64) -> String {
    let entry_lines: Vec<&str> = text.split('\n').map(str::trim).filter(|line| !line.is_empty()).collect();
    if entry_lines.len() > 1 && entry_lines.iter().all(|line| is_match(r"\A(?:[-*\x{2022}][ \t]*)?Scene[ \t]+\d+\b", line)) {
        let per_entry = (-((-maximum).div_euclid(entry_lines.len() as i64))).max(12);
        return entry_lines.iter().map(|line| cap_words(line, per_entry)).collect::<Vec<_>>().join("\n");
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() as i64 <= maximum {
        return words.join(" ");
    }
    let clipped = words[..maximum.max(0) as usize].join(" ");
    let sentence_end = [". ", "! ", "? "].iter().map(|needle| rfind_char(&clipped, needle)).max().unwrap_or(-1);
    if sentence_end >= 80.max(clipped.chars().count() as i64 / 2) {
        return clipped.chars().take(sentence_end as usize + 1).collect::<String>().trim().to_string();
    }
    format!("{}\u{2026}", clipped.trim_end_matches([' ', ',', ';', ':']))
}

/// `_story_arc_section_word_limit`: long song structures stay within the word budget of the detail level.
pub fn section_word_limit(section_count: usize, per_section_max: i64, total_budget: i64) -> i64 {
    if section_count == 0 {
        return per_section_max;
    }
    30.max(per_section_max.min(total_budget / section_count as i64))
}

fn base_section(value: Option<&Value>) -> String {
    let collapsed = collapse_ws(&text_or_empty(value));
    rx(r"\s+\d+\z").replace(&collapsed, "").to_lowercase()
}

/// `_story_arc_scene_map`: the timeline scenes grouped under the lyric section each one belongs to, in the order of `labels`. Empty when the scenes
/// cannot be matched to the sections.
pub fn scene_map(rows: &[Value], labels: &[String]) -> Vec<(String, Vec<Value>)> {
    let rows: Vec<&Value> = rows.iter().filter(|row| row.is_object()).collect();
    if labels.is_empty() || rows.is_empty() {
        return Vec::new();
    }
    let mut runs: Vec<(String, Vec<Value>)> = Vec::new();
    for row in rows {
        let key = base_section(row.get("lyric_section"));
        match runs.last_mut() {
            Some((last, group)) if *last == key => group.push(row.clone()),
            _ => runs.push((key, vec![row.clone()])),
        }
    }
    if runs.len() == labels.len() {
        return labels.iter().cloned().zip(runs.into_iter().map(|(_, group)| group)).collect();
    }
    // The runs and the sections differ in number (instrumental scenes, say): match by name and occurrence, then give every unmatched run to the
    // nearest earlier matched section.
    let mut owner_of_run: Vec<Option<usize>> = vec![None; runs.len()];
    for (label_index, label) in labels.iter().enumerate() {
        let key = base_section(Some(&Value::String(label.clone())));
        let Some(run_index) = runs.iter().enumerate().position(|(index, (name, _))| *name == key && owner_of_run[index].is_none()) else { return Vec::new() };
        owner_of_run[run_index] = Some(label_index);
    }
    let mut owners: Vec<Option<usize>> = Vec::new();
    let mut current = None;
    for owner in &owner_of_run {
        if owner.is_some() {
            current = *owner;
        }
        owners.push(current);
    }
    let first = owners.iter().flatten().next().copied();
    let mut grouped: Vec<Vec<Value>> = vec![Vec::new(); labels.len()];
    for ((_, group), owner) in runs.into_iter().zip(owners) {
        if let Some(index) = owner.or(first) {
            grouped[index].extend(group);
        }
    }
    labels.iter().cloned().zip(grouped).collect()
}

/// `_split_story_arc_sections`: "Label:\nbody" arc text as (label, body) pairs, using the required labels.
pub fn split_sections(text: &str, labels: &[String]) -> Vec<(String, String)> {
    let mut wanted: Vec<(String, &String)> = Vec::new();
    for label in labels {
        let key = label.trim().to_lowercase();
        match wanted.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = label,
            None => wanted.push((key, label)),
        }
    }
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut current: Option<usize> = None;
    for line in text.split('\n') {
        let stripped = line.trim();
        let key = stripped.strip_suffix(':').map(|heading| heading.trim().to_lowercase()).unwrap_or_default();
        let label = wanted.iter().find(|(k, _)| *k == key).map(|(_, label)| (*label).clone());
        match label {
            Some(label) if !sections.iter().any(|(l, _)| *l == label) => {
                sections.push((label, Vec::new()));
                current = Some(sections.len() - 1);
            }
            _ => {
                if let Some(index) = current {
                    sections[index].1.push(line);
                }
            }
        }
    }
    sections.into_iter().map(|(label, lines)| (label, lines.join("\n").trim().to_string())).collect()
}

/// `_story_arc_scene_entries_enabled`: pass 2 (one short note for each scene) runs for the Detailed and Rich levels unless the request says otherwise.
pub fn scene_entries_enabled(payload: &Obj, storyboard: &Obj, detail: &str) -> bool {
    let explicit = payload.get("story_arc_scene_entries").or_else(|| payload.get("storyArcSceneEntries")).or_else(|| storyboard.get("story_arc_scene_entries"));
    match explicit {
        Some(value) if !value.is_null() && value.as_str() != Some("") => match value {
            Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
            other => truthy(other),
        },
        _ => matches!(detail, "detailed" | "rich"),
    }
}

/// A reply the arc cannot use: the message the user sees.
type Failure = String;

const NON_STRUCTURAL: [&str; 12] = ["instrumental", "instrumental section", "instrumental break", "break", "interlude", "solo", "music only", "no vocals", "no vocal", "silence", "b roll", "b-roll"];

fn limited(items: &[String], count: usize) -> String {
    format!("{}{}", items.iter().take(count).cloned().collect::<Vec<_>>().join(", "), if items.len() > count { "\u{2026}" } else { "" })
}

/// `_normalize_story_arc_output`: the model's reply with exactly the required headings, in order, each section cut to the word limit.
pub fn normalize_output(text: &str, required_labels: &[String], maximum_words: i64, runner_label: &str) -> Result<String, Failure> {
    let mut raw = text.trim().to_string();
    let runner = Some(runner_label.trim()).filter(|r| !r.is_empty()).unwrap_or("LLM");
    if !required_labels.is_empty() {
        // A stray colon in a scene entry ("Scene 12 (Place): ...") must not look like a heading.
        let safe_keys: Vec<String> = required_labels.iter().map(|label| collapse_ws(label).to_lowercase()).collect();
        let entry = rx(r"\A([ \t]*(?:[-*\x{2022}][ \t]*)?Scene[ \t]+\d+\b[^:\n]*?):[ \t]*(.*)\z");
        let lines: Vec<String> = raw
            .split('\n')
            .map(|line| match entry.captures(line).ok().flatten() {
                Some(found) if !safe_keys.contains(&collapse_ws(&found[1]).to_lowercase()) => format!("{} \u{2014} {}", found[1].trim_end(), &found[2]).trim_end().to_string(),
                _ => line.to_string(),
            })
            .collect();
        raw = lines.join("\n");
    }
    // Local runners may put the section body on the same line as its heading.
    // A lyric tag such as [Verse 1: Male Rap] gives a required heading with its own colon: match those headings whole before the first colon.
    let mut colon_labels: Vec<String> = required_labels.iter().map(|label| collapse_ws(label)).filter(|label| label.contains(':')).collect();
    colon_labels.sort_by_key(|label| std::cmp::Reverse(label.len()));
    colon_labels.dedup();
    let whole_labels: String = colon_labels
        .iter()
        .map(|label| format!(r"\[?(?i:{})\]?|", label.split(' ').map(fancy_regex::escape).collect::<Vec<_>>().join(r"[ \t]+")))
        .collect();
    let heading = rx(&format!(r"(?m)^[ \t]*({whole_labels}[^\n:]{{1,80}}):[ \t]*"));
    let matches: Vec<_> = heading.captures_iter(&raw).flatten().collect();
    if matches.is_empty() {
        if !required_labels.is_empty() {
            let preview: String = collapse_ws(&raw).chars().take(240).collect();
            let expected = format!("{}{}", required_labels.iter().take(8).cloned().collect::<Vec<_>>().join(", "), if required_labels.len() > 8 { ", \u{2026}" } else { "" });
            return Err(format!(
                "{runner} did not return the required lyric section headings. No heading lines were detected. Expected: {expected}. Response preview: {}.",
                if preview.is_empty() { "[empty]" } else { &preview }
            ));
        }
        return Ok(cap_words(&raw, maximum_words));
    }
    let mut blocks: Vec<(String, String)> = Vec::new();
    for (index, found) in matches.iter().enumerate() {
        let whole = found.get(0).expect("whole match");
        let mut label = collapse_ws(&found[1]);
        if let Some(inner) = rx(r"\A\[([^\]\n]{1,80})\]\z").captures(&label).ok().flatten() {
            label = collapse_ws(&inner[1]);
        }
        let end = matches.get(index + 1).and_then(|next| next.get(0)).map_or(raw.len(), |m| m.start());
        blocks.push((label, raw[whole.end()..end].trim().to_string()));
    }
    if !required_labels.is_empty() {
        let required: Vec<String> = required_labels.iter().map(|l| l.to_lowercase()).collect();
        // A timeline marker such as [instrumental] can be promoted to a heading of its own: fold its text into the nearest real section.
        let mut folded: Vec<(String, String)> = Vec::new();
        let mut pending: Vec<String> = Vec::new();
        for (label, body) in blocks {
            let key = label.to_lowercase();
            if !required.contains(&key) && NON_STRUCTURAL.contains(&key.as_str()) {
                if let Some((_, previous)) = folded.last_mut() {
                    let merged = join_lines(&[previous.as_str(), body.as_str()]);
                    *previous = merged;
                } else if !body.is_empty() {
                    pending.push(body);
                }
                continue;
            }
            let body = if pending.is_empty() { body } else { pending.drain(..).chain(Some(body).filter(|b| !b.is_empty())).collect::<Vec<_>>().join("\n").trim().to_string() };
            folded.push((label, body));
        }
        if let (false, Some((_, last))) = (pending.is_empty(), folded.last_mut()) {
            let mut parts: Vec<&str> = vec![last.as_str()];
            parts.extend(pending.iter().map(String::as_str));
            let merged = join_lines(&parts);
            *last = merged;
        }
        blocks = folded;
        // Some runners write one heading for every timeline scene. Merge those runs only when that gives exactly the required sections.
        if blocks.len() != required_labels.len() {
            let mut merged: Vec<(String, String)> = Vec::new();
            for (label, body) in &blocks {
                match merged.last_mut() {
                    Some((previous, previous_body)) if previous.to_lowercase() == label.to_lowercase() => {
                        let joined = join_lines(&[previous_body.as_str(), body.as_str()]);
                        *previous_body = joined;
                    }
                    _ => merged.push((label.clone(), body.clone())),
                }
            }
            if merged.len() == required_labels.len() {
                blocks = merged;
            }
        }
        // A short instruction heading echoed before the answer is ignored. Invented story sections stay failures.
        let meta = rx(r"(?i)\b(?:user|instruction|requirement|preserve|output|heading|exact sections?|requested format|response format|story arc request)\b");
        while let Some((first_label, _)) = blocks.first() {
            let first_key = first_label.to_lowercase();
            if first_key == required[0] || first_key == format!("{} 1", required[0]) || !meta.is_match(first_label).unwrap_or(false) {
                break;
            }
            blocks.remove(0);
        }
        let trailing_number = rx(r"\s+\d+\z");
        let mut returned: Vec<String> = blocks.iter().map(|(label, _)| label.to_lowercase()).collect();
        // Some local models number the first occurrence of a heading although only repeated ones are numbered.
        for (index, actual) in returned.iter_mut().enumerate() {
            if let Some(expected) = required.get(index) {
                if *actual == format!("{expected} 1") {
                    *actual = expected.clone();
                }
            }
        }
        for (index, actual) in returned.iter_mut().enumerate() {
            if let Some(expected) = required.get(index) {
                let expected_base = trailing_number.replace(expected, "").into_owned();
                if trailing_number.is_match(expected).unwrap_or(false) && !trailing_number.is_match(actual).unwrap_or(false) && expected_base == *actual {
                    *actual = expected.clone();
                }
            }
        }
        // Invented sections after a complete, valid arc are dropped instead of failing it.
        if blocks.len() > required_labels.len() && returned[..required.len()] == required[..] {
            blocks.truncate(required_labels.len());
            returned.truncate(required.len());
        }
        if returned != required {
            let mismatch = required.iter().zip(&returned).position(|(expected, actual)| expected != actual).unwrap_or(required.len().min(returned.len()));
            let expected_label = required_labels.get(mismatch).map_or("[none]", String::as_str);
            let returned_label = blocks.get(mismatch).map_or("[missing]", |(label, _)| label.as_str());
            let missing: Vec<String> = required_labels.iter().filter(|label| !returned.contains(&label.to_lowercase())).cloned().collect();
            let extra: Vec<String> = blocks.iter().filter(|(label, _)| !required.contains(&label.to_lowercase())).map(|(label, _)| label.clone()).collect();
            let detected: Vec<String> = blocks.iter().map(|(label, _)| label.clone()).collect();
            let mut details = vec![
                format!("Expected {} headings but {runner} returned {}.", required_labels.len(), blocks.len()),
                format!("First mismatch at section {}: expected '{expected_label}', received '{returned_label}'.", mismatch + 1),
                format!("Detected headings: {}{}.", if detected.is_empty() { "[none]".to_string() } else { detected[..detected.len().min(12)].join(", ") }, if blocks.len() > 12 { "\u{2026}" } else { "" }),
            ];
            if !missing.is_empty() {
                details.push(format!("Missing: {}.", limited(&missing, 8)));
            }
            if !extra.is_empty() {
                details.push(format!("Extra: {}.", limited(&extra, 8)));
            }
            return Err(format!("{runner} changed the lyric structure. {}", details.join(" ")));
        }
        blocks = blocks.into_iter().enumerate().map(|(index, (_, body))| (required_labels[index].clone(), body)).collect();
    }
    Ok(blocks.iter().filter(|(_, body)| !body.is_empty()).map(|(label, body)| format!("{label}:\n{}", cap_words(body, maximum_words))).collect::<Vec<_>>().join("\n\n"))
}
