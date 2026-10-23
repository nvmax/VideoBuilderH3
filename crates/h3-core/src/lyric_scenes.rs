//! Lyrics when scenes are split, merged or replaced, and the scene length rule. Port of the matching parts of
//! `builder/lyric_scenes.py`.

use serde_json::{json, Value};

use crate::prompt::rx::{is_match, rx, sub};
use crate::pyval::{round_half_even, text_or_empty, to_float, Obj};

pub const DEFAULT_INSTRUMENTAL_TEXT: &str = "[instrumental]";

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

fn number(value: Option<&Value>) -> f64 {
    value.and_then(to_float).filter(|n| n.is_finite()).unwrap_or(0.0)
}

/// Scene text that means "no vocals".
pub fn is_instrumental_lyric_text(value: &str) -> bool {
    let lowered = sub(rx!(r"\s+"), &value.trim().to_lowercase(), " ");
    if lowered.is_empty() {
        return false;
    }
    if matches!(lowered.as_str(), "instrumental" | "[instrumental]" | "instrumental section" | "instrumental section.") {
        return true;
    }
    let labels = rx!(r"(?i)\[(?:intro|outro|bridge|verse|chorus|pre-chorus|prechorus|hook|refrain|interlude|break|section|instrumental|music|no vocals?|no singing|no lip\s*-?\s*sync|no lipsync|b-?roll|visual only|silence)\]");
    let words = rx!(r"(?i)\b(?:intro|outro|bridge|verse|chorus|pre-chorus|prechorus|hook|refrain|interlude|break|section|instrumental|music|no vocals?|no singing|no lip\s*-?\s*sync|no lipsync|b-?roll|visual only|silence)\b");
    let stripped = sub(labels, &lowered, " ");
    let stripped = sub(words, &stripped, " ");
    let stripped = sub(rx!(r"[\W_]+"), &stripped, " ").trim().to_string();
    is_match(rx!(r"(?i)\binstrumental|no vocals?|no singing|no lip\s*-?\s*sync|no lipsync|b-?roll|visual only|silence\b"), &lowered) && stripped.is_empty()
}

/// Join two scenes' lyrics, ignoring instrumental markers.
pub fn merge_lyric_text(a: &str, b: &str) -> String {
    let values: Vec<String> = [a, b].iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect();
    let vocal: Vec<&String> = values.iter().filter(|v| !is_instrumental_lyric_text(v)).collect();
    if vocal.is_empty() {
        return values.first().cloned().unwrap_or_else(|| DEFAULT_INSTRUMENTAL_TEXT.to_string());
    }
    let mut seen: Vec<&String> = Vec::new();
    for v in vocal {
        if !seen.contains(&v) {
            seen.push(v);
        }
    }
    seen.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
}

/// Divide a scene's lyrics between its two halves. `fraction` is the share of time the left half takes.
pub fn split_lyric_text(value: &str, fraction: f64) -> (String, String) {
    let value = value.trim();
    if value.is_empty() || is_instrumental_lyric_text(value) {
        let v = if value.is_empty() { DEFAULT_INSTRUMENTAL_TEXT } else { value };
        return (v.to_string(), v.to_string());
    }
    let fraction = fraction.clamp(0.05, 0.95);
    let lines: Vec<String> = value.replace("\r\n", "\n").split('\n').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if lines.len() >= 2 {
        let left = (round_half_even(lines.len() as f64 * fraction).max(1) as usize).min(lines.len() - 1);
        return (lines[..left].join("\n"), lines[left..].join("\n"));
    }
    let words: Vec<&str> = value.split_whitespace().collect();
    if words.len() < 2 {
        return (value.to_string(), DEFAULT_INSTRUMENTAL_TEXT.to_string());
    }
    let left = (round_half_even(words.len() as f64 * fraction).max(1) as usize).min(words.len() - 1);
    (words[..left].join(" "), words[left..].join(" "))
}

fn overlap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0)
}

/// Lyrics for new scene ranges, taken from the scenes they replace. One object per range with `lyric_text`,
/// `lyric_no_lip_sync` and `lyric_singers` (empty when nothing landed there).
pub fn carry_lyrics_over(old_segments: &[Value], ranges: &[(f64, f64)]) -> Vec<Obj> {
    let mut pieces: Vec<Vec<String>> = vec![vec![]; ranges.len()];
    let mut singers: Vec<Vec<String>> = vec![vec![]; ranges.len()];
    let mut ordered: Vec<&Value> = old_segments.iter().filter(|s| s.is_object()).collect();
    ordered.sort_by(|a, b| number(a.get("start")).total_cmp(&number(b.get("start"))));
    for old in ordered {
        let lyric = text(old.get("lyric_text"));
        if lyric.is_empty() || ranges.is_empty() {
            continue;
        }
        let old_start = number(old.get("start"));
        let old_end = old_start.max(number(old.get("end")));
        let mut weights: Vec<(usize, f64)> = ranges.iter().enumerate().map(|(i, (s, e))| (i, overlap(old_start, old_end, *s, *e))).filter(|(_, w)| *w > 0.0).collect();
        if weights.is_empty() {
            let middle = (old_start + old_end) / 2.0;
            let key = |i: usize| (middle - ranges[i].0).abs().min((middle - ranges[i].1).abs());
            let mut nearest = 0;
            for i in 1..ranges.len() {
                if key(i) < key(nearest) {
                    nearest = i;
                }
            }
            weights = vec![(nearest, 1.0)];
        }
        let mut remaining = lyric;
        let mut remaining_weight: f64 = weights.iter().map(|(_, w)| w).sum();
        for (position, (index, weight)) in weights.iter().enumerate() {
            let piece = if position == weights.len() - 1 {
                remaining.clone()
            } else {
                let (left, right) = split_lyric_text(&remaining, weight / remaining_weight);
                remaining = right;
                remaining_weight -= weight;
                left
            };
            if !piece.trim().is_empty() {
                pieces[*index].push(piece);
            }
            for name in old.get("lyric_singers").and_then(Value::as_array).into_iter().flatten() {
                let name = text(Some(name));
                if !name.is_empty() && !singers[*index].contains(&name) {
                    singers[*index].push(name);
                }
            }
        }
    }
    (0..ranges.len())
        .map(|i| {
            let mut merged = String::new();
            for piece in &pieces[i] {
                merged = if merged.is_empty() { piece.trim().to_string() } else { merge_lyric_text(&merged, piece) };
            }
            let mut entry = Obj::new();
            if !merged.is_empty() {
                entry.insert("lyric_no_lip_sync".into(), json!(is_instrumental_lyric_text(&merged)));
                entry.insert("lyric_text".into(), json!(merged));
                if !singers[i].is_empty() {
                    entry.insert("lyric_singers".into(), json!(singers[i]));
                }
            }
            entry
        })
        .collect()
}

// -- scene length rule ------------------------------------------------------------------------------------------------------

/// One edit toward `min <= length <= max`.
#[derive(Debug, Clone, PartialEq)]
pub enum LengthFix {
    Split { scene_id: String, at_time: f64 },
    Merge { scene_id: String, with: &'static str },
}

fn duration(seg: &Value) -> f64 {
    number(seg.get("end")) - number(seg.get("start"))
}

fn sorted_by_time(segments: &[Value]) -> Vec<&Value> {
    let mut items: Vec<&Value> = segments.iter().collect();
    items.sort_by(|a, b| (number(a.get("start")), number(a.get("end"))).partial_cmp(&(number(b.get("start")), number(b.get("end")))).unwrap_or(std::cmp::Ordering::Equal));
    items
}

fn scene_id(seg: &Value) -> String {
    text_or_empty(seg.get("id"))
}

/// The next edit that brings the scenes into range, or `None`. Scenes in `locked` are never touched.
pub fn next_length_fix(segments: &[Value], min_s: f64, max_s: f64, locked: &[String]) -> Result<Option<LengthFix>, String> {
    const TOL: f64 = 0.01;
    if min_s <= 0.0 || max_s < min_s {
        return Err("min_scene_seconds must be positive and not larger than max_scene_seconds.".into());
    }
    let items = sorted_by_time(segments);
    let is_locked = |s: &Value| locked.contains(&scene_id(s));
    for seg in &items {
        if is_locked(seg) {
            continue;
        }
        let d = duration(seg);
        if d > max_s + TOL {
            let parts = (d / max_s - 1e-9).ceil();
            return Ok(Some(LengthFix::Split { scene_id: scene_id(seg), at_time: crate::pyval::round_to(number(seg.get("start")) + d / parts, 4) }));
        }
    }
    if items.len() < 2 {
        return Ok(None);
    }
    for (index, seg) in items.iter().enumerate() {
        if is_locked(seg) || duration(seg) >= min_s - TOL {
            continue;
        }
        let mut options: Vec<(bool, i32, f64, &'static str)> = Vec::new();
        for (ni, direction) in [(index as i64 - 1, "previous"), (index as i64 + 1, "next")] {
            if ni >= 0 && (ni as usize) < items.len() && !is_locked(items[ni as usize]) {
                let n = items[ni as usize];
                let merged = number(n.get("end")).max(number(seg.get("end"))) - number(n.get("start")).min(number(seg.get("start")));
                let same = is_instrumental_lyric_text(&text(n.get("lyric_text"))) == is_instrumental_lyric_text(&text(seg.get("lyric_text")));
                options.push((merged > max_s + TOL, if same { 0 } else { 1 }, merged, direction));
            }
        }
        if options.is_empty() {
            continue;
        }
        options.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)).then(a.3.cmp(b.3)));
        return Ok(Some(LengthFix::Merge { scene_id: scene_id(seg), with: options[0].3 }));
    }
    Ok(None)
}

fn set(seg: &mut Value, key: &str, value: Value) {
    if let Some(map) = seg.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

/// Apply one fix to an in-memory scene list. `new_id` makes the id of the scene a split creates.
pub fn apply_length_fix(segments: &mut Vec<Value>, fix: &LengthFix, new_id: &mut dyn FnMut() -> String) -> Result<(), String> {
    segments.sort_by(|a, b| (number(a.get("start")), number(a.get("end"))).partial_cmp(&(number(b.get("start")), number(b.get("end")))).unwrap_or(std::cmp::Ordering::Equal));
    let target = match fix {
        LengthFix::Split { scene_id: id, .. } | LengthFix::Merge { scene_id: id, .. } => id.clone(),
    };
    let index = segments.iter().position(|s| scene_id(s) == target).ok_or_else(|| format!("Scene {target} not found"))?;
    match fix {
        LengthFix::Split { at_time, .. } => {
            let scene = segments[index].clone();
            let (start, end) = (number(scene.get("start")), number(scene.get("end")));
            let (left, right_text) = split_lyric_text(&text(scene.get("lyric_text")), (at_time - start) / (end - start).max(0.01));
            let mut right = scene.clone();
            set(&mut right, "id", json!(new_id()));
            set(&mut right, "start", json!(crate::pyval::round_to(*at_time, 4)));
            set(&mut right, "end", json!(crate::pyval::round_to(end, 4)));
            set(&mut right, "lyric_text", json!(right_text));
            set(&mut right, "source", json!("split"));
            set(&mut right, "lyric_no_lip_sync", json!(is_instrumental_lyric_text(&right_text)));
            let left_seg = &mut segments[index];
            set(left_seg, "end", json!(crate::pyval::round_to(*at_time, 4)));
            set(left_seg, "lyric_text", json!(left));
            set(left_seg, "lyric_no_lip_sync", json!(is_instrumental_lyric_text(&left)));
            segments.insert(index + 1, right);
        }
        LengthFix::Merge { with, .. } => {
            let other = if *with == "previous" { index.checked_sub(1).ok_or("no neighbour")? } else { index + 1 };
            if other >= segments.len() {
                return Err("no neighbour".into());
            }
            let (first_i, second_i) = if other < index { (other, index) } else { (index, other) };
            let second = segments[second_i].clone();
            let first = &mut segments[first_i];
            let start = number(first.get("start")).min(number(second.get("start")));
            let end = number(first.get("end")).max(number(second.get("end")));
            set(first, "start", json!(start));
            set(first, "end", json!(end));
            let lyric = merge_lyric_text(&text(first.get("lyric_text")), &text(second.get("lyric_text")));
            set(first, "lyric_no_lip_sync", json!(is_instrumental_lyric_text(&lyric)));
            set(first, "lyric_text", json!(lyric));
            let join = |a: &Value, b: &Value, key: &str, sep: &str| [text(a.get(key)), text(b.get(key))].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(sep);
            let note = join(first, &second, "timeline_note", "\n");
            let notes = join(first, &second, "notes", "\n\n");
            set(first, "timeline_note", json!(note));
            set(first, "notes", json!(notes));
            segments.remove(second_i);
        }
    }
    Ok(())
}
