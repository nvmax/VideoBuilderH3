//! Timeline rules on a list of scenes. Port of `builder/timeline.py` (the parts without files).
//!
//! Scenes are loose JSON objects so keys this app does not know stay as they are. Numbers follow Python's
//! `float(value or default)`: a missing or empty value gives the default.

use serde_json::{json, Value};

use crate::prompt::rx::{is_match, rx};
use crate::pyval::{py_float_str, py_str, round_to, to_float, truthy};

/// `float(segment.get(key, default) or default)`.
pub fn num(seg: &Value, key: &str, default: f64) -> f64 {
    seg.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(default)
}

fn set(seg: &mut Value, key: &str, value: f64) {
    if let Some(map) = seg.as_object_mut() {
        map.insert(key.to_string(), json!(value));
    }
}

/// Shift a scene's start, end and custom audio start by `delta` seconds.
pub fn shift_segment_timing(seg: &mut Value, delta: f64) {
    if delta.abs() < 1e-6 {
        return;
    }
    let start = round_to(num(seg, "start", 0.0) + delta, 4);
    let end = round_to(num(seg, "end", 0.0) + delta, 4);
    set(seg, "start", start);
    set(seg, "end", end);
    if let Some(custom) = seg.get("custom_audio_timeline_start").filter(|v| !v.is_null()).and_then(to_float) {
        set(seg, "custom_audio_timeline_start", round_to(custom + delta, 4));
    }
}

/// Sort by start time, keeping the order of equal starts.
pub fn sort_segments(segs: &mut [Value]) {
    segs.sort_by(|a, b| num(a, "start", 0.0).total_cmp(&num(b, "start", 0.0)));
}

/// Keep generic labels (`Scene N`, `N. Rooftop`) in step with the scene's position.
pub fn renumber_generic_base_scene_labels(segs: &mut [Value]) {
    sort_segments(segs);
    for (i, seg) in segs.iter_mut().enumerate() {
        let index = i + 1;
        let current = crate::pyval::text_or_empty(seg.get("label")).trim().to_string();
        let numbered = rx!(r"^\d+\.\s*(.+)$").captures(&current).ok().flatten().map(|c| c.get(1).map_or(String::new(), |m| m.as_str().to_string()));
        if let Some(desc) = numbered {
            seg["label"] = json!(format!("{index}. {desc}"));
        } else if current.is_empty() || is_match(rx!(r"(?i)^scene(?:\s+\d+(?:\.\d+)?)?$"), &current) {
            seg["label"] = json!(format!("Scene {index}"));
        }
    }
}

/// A scene with a rendered or imported video.
pub fn has_locked_video(seg: &Value) -> bool {
    let video = |key: &str| seg.get(key).filter(|v| truthy(v)).map(|v| crate::pyval::text_or_empty(Some(v))).filter(|s| !s.trim().is_empty());
    video("video_path").or_else(|| video("video_output")).is_some()
}

/// Keep scenes from overlapping after the scene at `active` changed its start or end.
pub fn normalize_segments(segs: &mut [Value], active_index: usize) {
    const MIN: f64 = 0.1;
    sort_segments(segs);
    if segs.is_empty() || active_index >= segs.len() {
        return;
    }
    let ai = active_index;
    let mut start = num(&segs[ai], "start", 0.0).max(0.0);
    let mut end = {
        let default = start + 4.0;
        (start + MIN).max(num(&segs[ai], "end", default))
    };
    let has_prev = ai > 0;
    let has_next = ai + 1 < segs.len();

    if !has_prev {
        start = 0.0;
    } else {
        let prev_locked = has_locked_video(&segs[ai - 1]);
        if prev_locked {
            start = start.max(num(&segs[ai - 1], "end", 0.0));
        } else {
            let prev_start = num(&segs[ai - 1], "start", 0.0);
            start = start.max(prev_start + MIN);
            set(&mut segs[ai - 1], "end", start);
        }
    }
    end = (start + MIN).max(end);

    if has_next {
        if has_locked_video(&segs[ai + 1]) {
            end = end.min(num(&segs[ai + 1], "start", end));
        } else {
            let next_end = num(&segs[ai + 1], "end", end + MIN);
            end = end.min(next_end - MIN);
        }
        end = (start + MIN).max(end);
        if !has_locked_video(&segs[ai + 1]) {
            set(&mut segs[ai + 1], "start", end);
            if num(&segs[ai + 1], "end", 0.0) < end + MIN {
                set(&mut segs[ai + 1], "end", end + MIN);
            }
        }
    }
    if has_prev && !has_locked_video(&segs[ai - 1]) {
        set(&mut segs[ai - 1], "end", start);
    }
    set(&mut segs[ai], "start", start);
    set(&mut segs[ai], "end", end);
    sort_segments(segs);
}

/// Ripple later scenes (and overlays) left by the length of a removed span. Returns the span length.
pub fn close_base_timeline_gap(segs: &mut [Value], overlay: Option<&mut [Value]>, start_time: f64, end_time: f64) -> f64 {
    let duration = (end_time - start_time).max(0.0);
    if duration <= 0.0001 {
        return 0.0;
    }
    let threshold = end_time - 0.001;
    for item in segs.iter_mut() {
        if num(item, "start", 0.0) >= threshold {
            shift_segment_timing(item, -duration);
        }
    }
    sort_segments(segs);
    if let Some(overlay) = overlay {
        for item in overlay.iter_mut() {
            if num(item, "start", 0.0) >= threshold {
                shift_segment_timing(item, -duration);
            }
        }
        sort_segments(overlay);
    }
    duration
}

/// Walk the scenes in order and close every gap. Returns the seconds removed.
pub fn close_all_base_timeline_gaps(segs: &mut [Value], mut overlay: Option<&mut [Value]>) -> f64 {
    sort_segments(segs);
    let mut removed = 0.0;
    let mut cursor = 0.0;
    for seg in segs.iter_mut() {
        let start = num(seg, "start", 0.0);
        if start > cursor + 0.001 {
            let gap = start - cursor;
            shift_segment_timing(seg, -gap);
            removed += gap;
            if let Some(overlay) = overlay.as_deref_mut() {
                for o in overlay.iter_mut() {
                    if num(o, "start", 0.0) >= start - 0.001 {
                        shift_segment_timing(o, -gap);
                    }
                }
            }
        } else if (start - cursor).abs() <= 0.001 && start != cursor {
            shift_segment_timing(seg, cursor - start);
        }
        cursor = f64::max(cursor, num(seg, "end", 0.0));
    }
    sort_segments(segs);
    if let Some(overlay) = overlay {
        sort_segments(overlay);
    }
    round_to(removed, 4)
}

/// The beat nearest to `t` within `tolerance` seconds, else `t` itself.
pub fn snap_to_beat(t: f64, beats: &[Value], tolerance: f64) -> f64 {
    if beats.is_empty() {
        return t;
    }
    let mut best = t;
    let mut min_dist = tolerance;
    for b in beats {
        let time = if b.is_object() { b.get("time").and_then(to_float) } else { to_float(b) }.unwrap_or(0.0);
        let dist = (time - t).abs();
        if dist <= min_dist {
            min_dist = dist;
            best = time;
        }
    }
    round_to(best, 4)
}

fn find_scene(segs: &[Value], scene_id: &str) -> Result<usize, String> {
    segs.iter()
        .enumerate()
        .position(|(i, s)| s.get("id").and_then(Value::as_str) == Some(scene_id) || (i + 1).to_string() == scene_id)
        .ok_or_else(|| format!("Scene {scene_id} not found"))
}

/// Move a scene to a new start. Returns the shift. `ripple` moves the later scenes with it.
pub fn move_scene_timing(segs: &mut [Value], scene_id: &str, new_start: f64, ripple: bool) -> Result<f64, String> {
    sort_segments(segs);
    let idx = find_scene(segs, scene_id)?;
    let old_start = num(&segs[idx], "start", 0.0);
    let old_end = num(&segs[idx], "end", old_start + 4.0);
    let dur = (old_end - old_start).max(0.1);
    let target = round_to(new_start, 4).max(0.0);
    let delta = target - old_start;
    set(&mut segs[idx], "start", target);
    set(&mut segs[idx], "end", round_to(target + dur, 4));
    if ripple {
        for s in segs[idx + 1..].iter_mut() {
            shift_segment_timing(s, delta);
        }
    } else {
        normalize_segments(segs, idx);
    }
    sort_segments(segs);
    renumber_generic_base_scene_labels(segs);
    Ok(delta)
}

/// Change a scene's length by duration or end time. Returns the change of the end.
pub fn resize_scene_timing(segs: &mut [Value], scene_id: &str, new_duration: Option<f64>, new_end: Option<f64>, ripple: bool) -> Result<f64, String> {
    sort_segments(segs);
    let idx = find_scene(segs, scene_id)?;
    let start = num(&segs[idx], "start", 0.0);
    let old_end = num(&segs[idx], "end", start + 4.0);
    let calc_end = match (new_duration, new_end) {
        (Some(d), _) => (start + 0.1).max(start + d),
        (None, Some(e)) => (start + 0.1).max(e),
        _ => old_end,
    };
    let delta = calc_end - old_end;
    set(&mut segs[idx], "end", round_to(calc_end, 4));
    if ripple {
        for s in segs[idx + 1..].iter_mut() {
            shift_segment_timing(s, delta);
        }
    } else {
        normalize_segments(segs, idx);
    }
    sort_segments(segs);
    renumber_generic_base_scene_labels(segs);
    Ok(delta)
}

/// Check the timeline for gaps, overlaps and a length past the audio.
pub fn validate_timeline_consistency(segs: &mut [Value], audio_duration: Option<f64>) -> Vec<Value> {
    let mut issues = Vec::new();
    if segs.is_empty() {
        return issues;
    }
    sort_segments(segs);
    if num(&segs[0], "start", 0.0) > 0.001 {
        issues.push(json!({
            "type": "timeline_gap",
            "message": format!("First scene does not start at 0.0 (starts at {}).", py_str(segs[0].get("start"))),
            "fix": "Run close-gaps or set first scene start to 0.",
        }));
    }
    for i in 0..segs.len() - 1 {
        let c_end = num(&segs[i], "end", 0.0);
        let n_start = num(&segs[i + 1], "start", 0.0);
        let scenes = json!([segs[i].get("id").cloned().unwrap_or(Value::Null), segs[i + 1].get("id").cloned().unwrap_or(Value::Null)]);
        if n_start < c_end - 0.001 {
            issues.push(json!({"type": "overlap", "message": format!("Scene {} and Scene {} overlap ({} > {}).", i + 1, i + 2, py_float_str(c_end), py_float_str(n_start)), "scenes": scenes}));
        } else if n_start > c_end + 0.001 {
            issues.push(json!({"type": "gap", "message": format!("Gap of {}s between Scene {} and Scene {}.", py_float_str(round_to(n_start - c_end, 3)), i + 1, i + 2), "scenes": scenes}));
        }
    }
    if let Some(audio) = audio_duration.filter(|a| *a > 0.0) {
        let last_end = num(&segs[segs.len() - 1], "end", 0.0);
        if last_end > audio + 0.05 {
            issues.push(json!({"type": "exceeds_audio", "message": format!("Timeline duration ({}s) exceeds audio duration ({}s).", py_float_str(last_end), py_float_str(audio))}));
        }
    }
    issues
}

// -- bulk text --------------------------------------------------------------------------------------------------------------

/// Python's `str.splitlines()`.
pub fn py_splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines.push(std::mem::take(&mut current));
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}' => lines.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Seconds from `ss.mmm`, `mm:ss` or `hh:mm:ss`.
pub fn parse_bulk_time_value(raw: &str) -> Result<f64, String> {
    let text = raw.trim().replace(',', ".");
    if text.is_empty() {
        return Err("Empty time value".into());
    }
    let parts: Vec<&str> = text.split(':').collect();
    let value = |p: &str| p.parse::<f64>().map_err(|_| format!("could not convert string to float: '{p}'"));
    match parts.len() {
        1 => value(parts[0]),
        2 => Ok(value(parts[0])? * 60.0 + value(parts[1])?),
        3 => Ok(value(parts[0])? * 3600.0 + value(parts[1])? * 60.0 + value(parts[2])?),
        _ => Err(format!("Invalid time format: '{raw}'")),
    }
}

const TIME_TOKEN: &str = r"\d+(?::\d+){0,2}(?:[.,]\d+)?";

fn scene(start: f64, end: f64, lyric: &str) -> Value {
    let mut item = json!({"start": start, "end": end});
    if !lyric.trim().is_empty() {
        item["lyric_text"] = json!(lyric.trim());
    }
    item
}

/// Parse multiline timings into `{start, end}` scenes, with `lyric_text` when a line carries words.
/// Modes: `durations`, `ranges`, `markers`.
pub fn parse_bulk_scenes(text: &str, mode: &str) -> Result<Vec<Value>, String> {
    let lines: Vec<String> = py_splitlines(text)
        .into_iter()
        .filter(|l| !is_match(rx!(r"^\s*(?:#|//|$).*$"), l))
        .map(|l| rx!(r"^\s*(?:[-*•]\s*|\d+[.)]\s+)").replace(l.trim(), "").into_owned())
        .collect();
    if lines.is_empty() {
        return Ok(vec![]);
    }
    let range_re = rx!(r"(?i)^(\d+(?::\d+){0,2}(?:[.,]\d+)?)\s*(?:-->|->|–|—|-|to)\s*(\d+(?::\d+){0,2}(?:[.,]\d+)?)(?:\s*[|:]\s*|\s+|$)(.*)$");
    let then_text = rx!(r"^(\d+(?::\d+){0,2}(?:[.,]\d+)?)(?:\s*[|:]\s*|\s+|$)(.*)$");
    let _ = TIME_TOKEN;
    let grp = |c: &fancy_regex::Captures, n: usize| c.get(n).map_or(String::new(), |m| m.as_str().to_string());
    let mut results = Vec::new();
    match mode {
        "durations" => {
            let mut cursor = 0.0;
            for (i, line) in lines.iter().enumerate() {
                let fail = |why: String| format!("Line {} '{line}': invalid duration ({why})", i + 1);
                let caps = then_text.captures(line).ok().flatten().ok_or_else(|| fail(if line.is_empty() { "Empty time value".into() } else { format!("Invalid time format: '{line}'") }))?;
                let dur = parse_bulk_time_value(&grp(&caps, 1)).map_err(fail)?;
                if dur <= 0.05 {
                    return Err(fail("Duration too short".into()));
                }
                let end = round_to(cursor + dur, 4);
                results.push(scene(cursor, end, &grp(&caps, 2)));
                cursor = end;
            }
        }
        "ranges" => {
            for (i, line) in lines.iter().enumerate() {
                let caps = range_re.captures(line).ok().flatten().ok_or_else(|| format!("Line {} '{line}': expected 'start - end'", i + 1))?;
                let fail = |why: String| format!("Line {} '{line}': invalid range ({why})", i + 1);
                let start = parse_bulk_time_value(&grp(&caps, 1)).map_err(fail)?;
                let end = parse_bulk_time_value(&grp(&caps, 2)).map_err(fail)?;
                if end <= start + 0.05 {
                    return Err(fail("End time must be after start time".into()));
                }
                results.push(scene(round_to(start, 4), round_to(end, 4), &grp(&caps, 3)));
            }
        }
        "markers" => {
            let mut markers: Vec<(f64, String)> = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                let fail = |why: String| format!("Line {} '{line}': invalid timestamp ({why})", i + 1);
                let caps = then_text.captures(line).ok().flatten().ok_or_else(|| fail(format!("Invalid time format: '{line}'")))?;
                markers.push((parse_bulk_time_value(&grp(&caps, 1)).map_err(fail)?, grp(&caps, 2)));
            }
            markers.sort_by(|a, b| a.0.total_cmp(&b.0));
            for pair in markers.windows(2) {
                let (s, lyric) = (&pair[0].0, &pair[0].1);
                let e = pair[1].0;
                if e > s + 0.05 {
                    results.push(scene(round_to(*s, 4), round_to(e, 4), lyric));
                }
            }
        }
        _ => {}
    }
    Ok(results)
}
