//! Scenes from timed lyric lines. Port of `segments_from_timestamped_payload`, `normalize_scene_durations`,
//! `apply_lyric_sections` and `clean_timestamped_lyric_text` in `builder/lyric_scenes.py`, and of
//! `build_scenes_from_payload` and the output parser in `agent_api/orchestrator/lyrics_orchestrator.py`.

use serde_json::{json, Value};

use crate::lyric_scenes::{apply_length_fix, is_instrumental_lyric_text, merge_lyric_text_with, next_length_fix, DEFAULT_INSTRUMENTAL_TEXT};
use crate::prompt::rx::{group, is_match, rx, sub, sub_with};
use crate::pyval::{text_or_empty, to_float, truthy, Obj};

const EPSILON: f64 = 0.03;
const DEFAULT_VARIANCE: f64 = 2.0;
const REFERENCE_UNIT_MODES: [&str; 3] = ["reference_lines", "exact_reference_lines", "reference_stanzas"];

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

/// `_number(value, default)`: a finite float or the default.
fn numd(value: Option<&Value>, default: f64) -> f64 {
    value.and_then(to_float).filter(|n| n.is_finite()).unwrap_or(default)
}

fn num(seg: &Value, key: &str) -> f64 {
    numd(seg.get(key), 0.0)
}

fn set(seg: &mut Value, key: &str, value: Value) {
    if let Some(map) = seg.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

fn sort_by_time(items: &mut [Value]) {
    items.sort_by(|a, b| (num(a, "start"), num(a, "end")).partial_cmp(&(num(b, "start"), num(b, "end"))).unwrap_or(std::cmp::Ordering::Equal));
}

fn is_non_vocal_marker_label(label: &str) -> bool {
    let clean = sub(rx!(r"[^a-z0-9]+"), &label.to_lowercase(), " ");
    let clean = sub(rx!(r"\s+"), &clean, " ");
    is_match(rx!(r"^(instrumental|break|interlude|solo|no vocal|no vocals|no lyrics|silence|b roll|music only)$"), clean.trim())
}

/// Drop `[Verse 1]` style headers and tidy spacing.
pub fn clean_timestamped_lyric_text(input: &str) -> String {
    let stripped = sub_with(rx!(r"\[([^\]]{2,80})\]"), input, |c| if is_non_vocal_marker_label(&group(c, 1)) { group(c, 0) } else { " ".to_string() });
    let stripped = sub(rx!(r"\s+"), &stripped, " ");
    let stripped = sub(rx!(r"\s+([,.;:!?])"), &stripped, "$1");
    let stripped = sub(rx!(r"([(\[{])\s+"), &stripped, "$1");
    let stripped = sub(rx!(r"\s+([)\]}])"), &stripped, "$1");
    stripped.trim().to_string()
}

// -- section labels ---------------------------------------------------------------------------------------------------------

fn section_lookup_text(value: &str) -> String {
    let lowered = value.to_lowercase();
    let lowered = sub(rx!(r"[‘’'`]"), &lowered, "");
    let lowered = sub(rx!(r"[^\w' ]|_"), &lowered, " ");
    sub(rx!(r"\s+"), &lowered, " ").trim().to_string()
}

fn is_structural_section(label: &str) -> bool {
    is_match(
        rx!(r"(?i)^(?:intro|verse|pre[\s-]?chorus|chorus|post[\s-]?chorus|bridge|outro|refrain|hook|breakdown|drop|interlude|instrumental(?:\s+break)?|solo|break|spoken(?:\s+word)?|rap)(?:\s+(?:\d+|[ivxlcdm]+))?$"),
        label,
    )
}

/// Each lyric line (normalized) with the `[Section]` header above it, in the order first seen.
pub fn lyric_section_map_from_reference_text(input: &str) -> Vec<(String, String)> {
    let mut mapping: Vec<(String, String)> = Vec::new();
    let mut current = String::new();
    let known = |m: &[(String, String)], key: &str| m.iter().any(|(k, _)| k == key);
    for raw in input.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let tags: Vec<(usize, String)> = rx!(r"\[([^\]]{1,80})\]").captures_iter(line).filter_map(Result::ok).map(|c| (c.get(0).map_or(0, |m| m.start()), group(&c, 1))).collect();
        let prefix = rx!(r"^(?:\s*\[[^\]]{1,80}\])+\s*").find(line).ok().flatten().map(|m| m.as_str().to_string());
        let tag_prefix = match (&prefix, tags.first()) {
            (Some(p), Some((0, _))) => p.clone(),
            _ => String::new(),
        };
        let collapse = |s: &str| sub(rx!(r"\s+"), s, " ").trim().to_string();
        let structural = tags.iter().map(|(_, t)| collapse(t)).find(|t| is_structural_section(t));
        let terminal = tags.iter().any(|(_, t)| is_match(rx!(r"(?i)^(?:end|end of song)$"), t.trim()));
        if let (Some(section), false) = (&structural, tag_prefix.is_empty()) {
            current = section.clone();
            let key = section_lookup_text(line[tag_prefix.len()..].trim());
            if !key.is_empty() && !known(&mapping, &key) {
                mapping.push((key, current.clone()));
            }
            continue;
        }
        if terminal && !tag_prefix.is_empty() && line[tag_prefix.len()..].trim().is_empty() {
            current.clear();
            continue;
        }
        let key = section_lookup_text(line);
        if !key.is_empty() && !current.is_empty() && !known(&mapping, &key) {
            mapping.push((key, current.clone()));
        }
    }
    mapping
}

/// Set `lyric_section` on scenes from the pasted lyrics. Returns how many were set.
pub fn apply_lyric_sections(segments: &mut [Value], reference_text: &str) -> usize {
    let map = lyric_section_map_from_reference_text(reference_text);
    if map.is_empty() {
        return 0;
    }
    let mut applied = 0;
    for segment in segments.iter_mut() {
        let existing = text(segment.get("lyric_section")).to_lowercase();
        let lyric = text(segment.get("lyric_text"));
        if !existing.is_empty() && existing != "instrumental" {
            continue;
        }
        if lyric.is_empty() || is_instrumental_lyric_text(&lyric) {
            set(segment, "lyric_section", json!("instrumental"));
            continue;
        }
        let lines: Vec<String> = lyric.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        for line in lines {
            if let Ok(Some(c)) = rx!(r"^\[([^\]]{2,80})\]$").captures(&line) {
                set(segment, "lyric_section", json!(group(&c, 1).trim()));
                applied += 1;
                break;
            }
            let key = section_lookup_text(&line);
            if let Some((_, value)) = map.iter().find(|(k, _)| *k == key) {
                set(segment, "lyric_section", json!(value));
                applied += 1;
                break;
            }
            if key.chars().count() >= 6 {
                let fuzzy = map.iter().find(|(k, _)| k.chars().count() >= 6 && (key.contains(k.as_str()) || k.contains(key.as_str()))).map(|(_, v)| v.clone());
                if let Some(value) = fuzzy {
                    set(segment, "lyric_section", json!(value));
                    applied += 1;
                    break;
                }
            }
        }
    }
    applied
}

// -- timed lines to scenes --------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct FromPayload {
    pub segment_mode: String,
    pub include_instrumental_gaps: bool,
    pub instrumental_text: String,
    pub min_gap_seconds: Option<f64>,
    pub max_scene_seconds: Option<f64>,
    pub min_scene_seconds: Option<f64>,
    pub keep_reference_units: bool,
}

/// `payload.get(a, payload.get(b))`.
fn payload_get<'a>(payload: &'a Obj, a: &str, b: &str) -> Option<&'a Value> {
    if payload.contains_key(a) { payload.get(a) } else { payload.get(b) }
}

/// The first truthy value, like Python's `a or b or c`.
fn first_truthy<'a>(values: &[Option<&'a Value>]) -> Option<&'a Value> {
    values.iter().copied().flatten().find(|v| truthy(v))
}

fn word_text(word: &Value) -> String {
    text(first_truthy(&[word.get("text"), word.get("word")]))
}

fn joined_words(words: &[Value]) -> String {
    words.iter().map(word_text).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
}

struct Source {
    item: Value,
    start: f64,
    end: f64,
}

fn new_scene(start: f64, end: f64, label: &str, new_id: &mut dyn FnMut() -> String) -> Value {
    json!({
        "id": new_id(), "track": "base", "start": start, "end": end, "label": label, "notes": "", "timeline_note": "", "lyric_text": "",
        "lyric_section": "", "story_beat": "", "no_character_present": false, "source": "timestamped_lyrics"
    })
}

/// Chunks longer than max plus variance are split near word boundaries, gaps between vocal chunks become
/// instrumental scenes and the tail of the song is filled.
pub fn segments_from_timestamped_payload(payload: &Obj, p: &FromPayload, new_id: &mut dyn FnMut() -> String) -> Vec<Value> {
    let source_segments: Vec<Value> = payload.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let mode = text(first_truthy(&[Some(&json!(p.segment_mode)), payload.get("segment_mode"), payload.get("segmentMode")]));
    let preserves_units = mode == "exact_reference_lines" || (p.keep_reference_units && REFERENCE_UNIT_MODES.contains(&mode.as_str()));
    let min_scene = numd(p.min_scene_seconds.map(|v| json!(v)).as_ref().or_else(|| payload_get(payload, "min_scene_seconds", "minSceneSeconds")), 0.0).max(0.0);

    let mut ordered_source: Vec<Source> = Vec::new();
    for item in &source_segments {
        let Some(obj) = item.as_object() else { continue };
        let start = numd(obj.get("start"), 0.0).max(0.0);
        let end = numd(obj.get("end"), 0.0).max(0.0);
        if end > start + 0.01 {
            ordered_source.push(Source { item: item.clone(), start, end });
        }
    }
    ordered_source.sort_by(|a, b| (a.start, a.end).partial_cmp(&(b.start, b.end)).unwrap_or(std::cmp::Ordering::Equal));

    let instrumental = Some(text(first_truthy(&[Some(&json!(p.instrumental_text)), payload.get("instrumental_text"), payload.get("instrumentalText")]))).filter(|t| !t.is_empty()).unwrap_or_else(|| DEFAULT_INSTRUMENTAL_TEXT.to_string());
    let not_false = |v: Option<&Value>| v != Some(&Value::Bool(false));
    let fill_gaps = p.include_instrumental_gaps && not_false(payload.get("include_instrumental_gaps")) && not_false(payload.get("includeInstrumentalGaps"));
    let min_gap = numd(p.min_gap_seconds.map(|v| json!(v)).as_ref().or_else(|| payload_get(payload, "min_gap_seconds", "minGapSeconds")), 0.25).max(0.0);
    let max_raw = numd(p.max_scene_seconds.map(|v| json!(v)).as_ref().or_else(|| payload_get(payload, "max_scene_seconds", "maxSceneSeconds")), 8.0);
    let max_scene = (if max_raw == 0.0 { 8.0 } else { max_raw }).max(0.5);
    let soft_max = max_scene + DEFAULT_VARIANCE.max(0.0);

    let mut ordered: Vec<Source> = Vec::new();
    for source in ordered_source {
        let duration = source.end - source.start;
        if preserves_units || duration <= soft_max + EPSILON {
            ordered.push(source);
            continue;
        }
        let mut words: Vec<Value> = Vec::new();
        for word in source.item.get("words").and_then(Value::as_array).cloned().unwrap_or_default() {
            if !word.is_object() {
                continue;
            }
            let start = numd(word.get("start"), f64::NAN);
            let end_value = if word.get("end").is_some() { word.get("end") } else { word.get("start") };
            let end = numd(end_value, f64::NAN);
            if start.is_finite() && end.is_finite() && end > start {
                let mut copy = word.clone();
                set(&mut copy, "start", json!(start));
                set(&mut copy, "end", json!(end));
                words.push(copy);
            }
        }
        words.sort_by(|a, b| (num(a, "start"), num(a, "end")).partial_cmp(&(num(b, "start"), num(b, "end"))).unwrap_or(std::cmp::Ordering::Equal));
        let mut part_start = source.start;
        let mut cursor = 0usize;
        while source.end - part_start > soft_max + EPSILON {
            let target = part_start + max_scene;
            let limit = part_start + soft_max;
            let all: Vec<usize> = (cursor..words.len()).filter(|i| num(&words[*i], "end") > part_start + 0.1 && num(&words[*i], "end") <= limit + EPSILON).collect();
            let full: Vec<usize> = all.iter().copied().filter(|i| num(&words[*i], "end") - part_start >= min_scene - EPSILON && source.end - num(&words[*i], "end") >= min_scene - EPSILON).collect();
            let candidates = if full.is_empty() { all } else { full };
            let mut chosen: Option<usize> = None;
            for i in candidates {
                if chosen.is_none_or(|c| (num(&words[i], "end") - target).abs() < (num(&words[c], "end") - target).abs()) {
                    chosen = Some(i);
                }
            }
            let part_end = chosen.map_or(target, |i| num(&words[i], "end"));
            let mut part_item = source.item.clone();
            if let Some(i) = chosen {
                let part_words: Vec<Value> = words[cursor..=i].to_vec();
                let joined = joined_words(&part_words);
                set(&mut part_item, "words", Value::Array(part_words));
                let fallback = source.item.get("text").cloned().unwrap_or_else(|| json!(""));
                set(&mut part_item, "text", if joined.is_empty() { fallback } else { json!(joined) });
                cursor = i + 1;
            } else {
                set(&mut part_item, "words", json!([]));
            }
            set(&mut part_item, "timing_warning", json!(format!("Long transcription chunk split near a word boundary to honor the {soft_max:.2} second maximum.")));
            ordered.push(Source { item: part_item, start: part_start, end: part_end });
            part_start = part_end;
        }
        let mut final_item = source.item.clone();
        let final_words: Vec<Value> = words[cursor.min(words.len())..].iter().filter(|w| num(w, "end") > part_start - EPSILON).cloned().collect();
        if !final_words.is_empty() {
            let joined = joined_words(&final_words);
            set(&mut final_item, "words", Value::Array(final_words));
            let fallback = source.item.get("text").cloned().unwrap_or_else(|| json!(""));
            set(&mut final_item, "text", if joined.is_empty() { fallback } else { json!(joined) });
        }
        set(&mut final_item, "timing_warning", json!("Continuation of a long transcription chunk split near word boundaries."));
        ordered.push(Source { item: final_item, start: part_start, end: source.end });
    }

    let mut created: Vec<Value> = Vec::new();
    let add_segment = |created: &mut Vec<Value>, new_id: &mut dyn FnMut() -> String, start: f64, end: f64, item: Option<&Value>, forced: &str| {
        let clean_start = start.max(0.0);
        let clean_end = (clean_start + 0.05).max(numd(Some(&json!(end)), clean_start + 4.0));
        let mut segment = new_scene(clean_start, clean_end, &format!("SCENE {}", created.len() + 1), new_id);
        set(&mut segment, "timeline_note", json!(item.map(|i| text(i.get("timing_warning"))).unwrap_or_default()));
        let item_text = item.and_then(|i| i.get("text"));
        let raw = if !forced.is_empty() { forced.to_string() } else { text_or_empty(first_truthy(&[item_text])) };
        let cleaned = clean_timestamped_lyric_text(&raw);
        let lyric = if cleaned.is_empty() { instrumental.clone() } else { cleaned };
        let no_sync = item.is_none() || item.is_some_and(|i| text(i.get("type")).to_lowercase() == "instrumental") || is_instrumental_lyric_text(&lyric);
        set(&mut segment, "lyric_text", json!(lyric));
        set(&mut segment, "lyric_no_lip_sync", json!(no_sync));
        created.push(segment);
    };
    let add_gap = |created: &mut Vec<Value>, new_id: &mut dyn FnMut() -> String, start: f64, end: f64| -> bool {
        if !fill_gaps {
            return false;
        }
        let clean_start = start.max(0.0);
        let clean_end = clean_start.max(end);
        if clean_end - clean_start < min_gap.max(EPSILON) {
            return false;
        }
        let mut gap_start = clean_start;
        let mut added = false;
        while gap_start < clean_end - EPSILON {
            let gap_end = clean_end.min(gap_start + max_scene);
            add_segment(created, new_id, gap_start, gap_end, None, &instrumental);
            added = true;
            gap_start = gap_end;
        }
        added
    };

    let mut cursor = 0.0f64;
    for entry in &ordered {
        let mut segment_start = entry.start;
        if entry.start > cursor + EPSILON && !add_gap(&mut created, new_id, cursor, entry.start) {
            if let Some(last) = created.last_mut() {
                set(last, "end", json!(entry.start));
            } else {
                segment_start = cursor;
            }
        }
        add_segment(&mut created, new_id, segment_start, entry.end, Some(&entry.item), "");
        cursor = cursor.max(entry.end);
    }
    let duration = numd(payload.get("duration"), 0.0);
    if duration > cursor + EPSILON && !add_gap(&mut created, new_id, cursor, duration) {
        if let Some(last) = created.last_mut() {
            set(last, "end", json!(duration));
        }
    }
    sort_by_time(&mut created);
    for (i, segment) in created.iter_mut().enumerate() {
        set(segment, "label", json!(format!("SCENE {}", i + 1)));
    }
    created
}

/// Merge scenes shorter than the minimum into a neighbour.
pub fn normalize_scene_durations(segments: &[Value], min_scene_seconds: f64, max_scene_seconds: f64, segment_mode: &str, instrumental_text: &str) -> Vec<Value> {
    let mut items: Vec<Value> = segments.iter().filter(|s| s.as_object().is_some_and(|o| !o.is_empty())).cloned().collect();
    sort_by_time(&mut items);
    if REFERENCE_UNIT_MODES.contains(&segment_mode) {
        return items;
    }
    let min_s = numd(Some(&json!(min_scene_seconds)), 1.0).max(1.0);
    let max_s = min_s.max(numd(Some(&json!(max_scene_seconds)), 8.0));
    let soft_max = max_s + DEFAULT_VARIANCE.max(0.0);
    if items.is_empty() {
        return items;
    }
    let mut index = 0usize;
    while index < items.len() {
        let start = num(&items[index], "start").max(0.0);
        let end = (start + 0.05).max(numd(items[index].get("end"), start + 0.05));
        set(&mut items[index], "start", json!(start));
        set(&mut items[index], "end", json!(end));
        let duration = end - start;
        if duration >= min_s || items.len() <= 1 {
            index += 1;
            continue;
        }
        let prev = index.checked_sub(1);
        let next = if index + 1 < items.len() { Some(index + 1) } else { None };
        let span = |i: Option<usize>| i.map_or(-1.0, |i| (num(&items[i], "end") - num(&items[i], "start")).max(0.0));
        let (prev_duration, next_duration) = (span(prev), span(next));
        let segment_instrumental = is_instrumental_lyric_text(&text(items[index].get("lyric_text")));
        let vocal = |i: Option<usize>| i.is_some_and(|i| !is_instrumental_lyric_text(&text(items[i].get("lyric_text"))));
        let (prev_vocal, next_vocal) = (vocal(prev), vocal(next));
        let nearest = if next.is_some() && (prev.is_none() || next_duration <= prev_duration) { next } else { prev };
        let target = if segment_instrumental {
            nearest
        } else if prev_vocal {
            prev
        } else if next_vocal {
            next
        } else {
            nearest
        };
        let Some(target) = target else {
            if !segment_instrumental {
                let next_start = next.map(|i| num(&items[i], "start"));
                let desired_end = start + min_s;
                let new_end = match next_start {
                    Some(n) if n != 0.0 && n > start => desired_end.min(n),
                    _ => desired_end,
                };
                set(&mut items[index], "end", json!(new_end));
            }
            index += 1;
            continue;
        };
        let target_start = num(&items[target], "start");
        let merged_start = target_start.min(start);
        let merged_end = numd(items[target].get("end"), target_start + min_s).max(end);
        if merged_end - merged_start > soft_max + 0.001 {
            index += 1;
            continue;
        }
        let target_first = target_start <= start;
        let (a, b) = (text(items[target].get("lyric_text")), text(items[index].get("lyric_text")));
        let lyric = if target_first { merge_lyric_text_with(&a, &b, instrumental_text) } else { merge_lyric_text_with(&b, &a, instrumental_text) };
        let note = [text(items[target].get("timeline_note")), text(items[index].get("timeline_note"))].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n");
        let no_sync = is_instrumental_lyric_text(&lyric);
        let t = &mut items[target];
        set(t, "start", json!(merged_start));
        set(t, "end", json!(merged_end));
        set(t, "lyric_text", json!(lyric));
        set(t, "lyric_no_lip_sync", json!(no_sync));
        set(t, "timeline_note", json!(note));
        items.remove(index);
        index = index.saturating_sub(2);
    }
    for i in 0..items.len() {
        if i > 0 {
            let start = num(&items[i - 1], "end").max(num(&items[i], "start"));
            set(&mut items[i], "start", json!(start));
            if num(&items[i], "end") <= start + 0.05 {
                set(&mut items[i], "end", json!(start + min_s));
            }
        }
        let end = (num(&items[i], "start") + 0.05).max(num(&items[i], "end"));
        set(&mut items[i], "end", json!(end));
    }
    items
}

/// Apply the length fix until every scene fits, on an in-memory list.
pub fn enforce_scene_lengths(segments: &mut Vec<Value>, min_s: f64, max_s: f64, locked: &[String], new_id: &mut dyn FnMut() -> String) -> Result<(), String> {
    let limit = 50.max(segments.len() * 6);
    for _ in 0..limit {
        let Some(fix) = next_length_fix(segments, min_s, max_s, locked)? else { break };
        apply_length_fix(segments, &fix, new_id)?;
    }
    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by(|a, b| num(&segments[*a], "start").total_cmp(&num(&segments[*b], "start")));
    for (n, i) in order.into_iter().enumerate() {
        if is_match(rx!(r"(?i)^SCENE \d+$"), &text(segments[i].get("label"))) {
            set(&mut segments[i], "label", json!(format!("SCENE {}", n + 1)));
        }
    }
    sort_by_time(segments);
    Ok(())
}

// -- the job's steps --------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BuildParams {
    pub segment_mode: String,
    pub instrumental_text: String,
    pub min_scene_seconds: f64,
    pub max_scene_seconds: f64,
    pub min_gap_seconds: f64,
    pub include_instrumental_gaps: bool,
    pub enforce_lengths: bool,
}

impl Default for BuildParams {
    fn default() -> Self {
        Self {
            segment_mode: "reference_lines".into(),
            instrumental_text: DEFAULT_INSTRUMENTAL_TEXT.into(),
            min_scene_seconds: 1.0,
            max_scene_seconds: 8.0,
            min_gap_seconds: 2.0,
            include_instrumental_gaps: true,
            enforce_lengths: true,
        }
    }
}

fn normalize_for_safety(value: &str) -> String {
    let cleaned = clean_timestamped_lyric_text(value).to_lowercase();
    let cleaned = sub(rx!(r"[\W_]+"), &cleaned, " ");
    sub(rx!(r"\s+"), &cleaned, " ").trim().to_string()
}

/// One scene must not receive several of the pasted lyric lines.
pub fn assert_no_bundled_reference_lyrics(lyric_values: &[String], reference_lyrics: &str) -> Result<(), String> {
    let mut reference_lines: Vec<String> = Vec::new();
    for raw in reference_lyrics.replace("\r\n", "\n").split('\n') {
        let line = normalize_for_safety(raw);
        if line.chars().count() >= 4 && !reference_lines.contains(&line) {
            reference_lines.push(line);
        }
    }
    if reference_lines.len() < 2 {
        return Ok(());
    }
    for (i, value) in lyric_values.iter().enumerate() {
        let t = normalize_for_safety(value);
        if t.is_empty() || reference_lines.contains(&t) {
            continue;
        }
        let contained = reference_lines.iter().filter(|line| t.contains(line.as_str())).count();
        if contained >= 2 {
            return Err(format!("Transcription safety check stopped the update: scene {} received {contained} pasted lyric lines in one result.", i + 1));
        }
    }
    Ok(())
}

/// Timed lines to scenes with section labels and the length rule.
pub fn build_scenes_from_payload(payload: &Obj, params: &BuildParams, reference_lyrics: &str, new_id: &mut dyn FnMut() -> String) -> Result<Vec<Value>, String> {
    let mode = {
        let m = params.segment_mode.trim().to_lowercase();
        if m.is_empty() { text(payload.get("segment_mode")).to_lowercase() } else { m }
    };
    let mode = if mode.is_empty() { "reference_lines".to_string() } else { mode };
    let instrumental = if params.instrumental_text.is_empty() { DEFAULT_INSTRUMENTAL_TEXT.to_string() } else { params.instrumental_text.clone() };
    let scenes = segments_from_timestamped_payload(
        payload,
        &FromPayload {
            segment_mode: mode.clone(),
            include_instrumental_gaps: params.include_instrumental_gaps,
            instrumental_text: instrumental.clone(),
            min_gap_seconds: Some(params.min_gap_seconds),
            max_scene_seconds: Some(params.max_scene_seconds),
            min_scene_seconds: Some(params.min_scene_seconds),
            keep_reference_units: !params.enforce_lengths,
        },
        new_id,
    );
    let mut scenes = normalize_scene_durations(&scenes, params.min_scene_seconds, params.max_scene_seconds, &mode, &instrumental);
    if scenes.is_empty() {
        return Err("Timestamped lines did not produce any usable scene segments.".into());
    }
    if matches!(mode.as_str(), "reference_lines" | "exact_reference_lines") {
        let lyrics: Vec<String> = scenes.iter().map(|s| text_or_empty(s.get("lyric_text"))).collect();
        assert_no_bundled_reference_lyrics(&lyrics, reference_lyrics)?;
    }
    if params.enforce_lengths {
        enforce_scene_lengths(&mut scenes, params.min_scene_seconds, params.max_scene_seconds, &[], new_id)?;
    }
    apply_lyric_sections(&mut scenes, reference_lyrics);
    Ok(scenes)
}

/// Find the timestamped lyrics JSON in the workflow's text output.
pub fn parse_timestamped_lyrics_output(output: &str) -> Result<Obj, String> {
    let raw = output.trim();
    let has_segments = |v: &Value| v.get("segments").is_some_and(Value::is_array);
    if let Ok(Value::Object(direct)) = serde_json::from_str::<Value>(raw) {
        if has_segments(&Value::Object(direct.clone())) {
            return Ok(direct);
        }
    }
    if let (Some(start), Some(end)) = (raw.find('{'), raw.rfind('}')) {
        if end > start {
            if let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&raw[start..=end]) {
                if has_segments(&Value::Object(parsed.clone())) {
                    return Ok(parsed);
                }
            }
        }
    }
    Err("Timestamped transcription finished, but no timestamped lyrics JSON was found.".into())
}
