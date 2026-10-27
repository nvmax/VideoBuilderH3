//! Scene operations on a project folder: insert, delete, split, merge, move, resize, bulk timeline and close gaps.
//! Port of the scene functions in `agent_api/mutations.py`.
//!
//! Operations that rename scene files (insert, delete, split, merge) run inside a [`TimelineJournal`], so a failure
//! puts the files and the session back. Each operation saves through [`persist_session`].

use std::io;
use std::path::Path;

use h3_core::lyric_scenes::{carry_lyrics_over, is_instrumental_lyric_text, merge_lyric_text, split_lyric_text};
use h3_core::pyval::{py_float_str, text_or_empty, to_float, truthy, Obj};
use h3_core::timeline::{
    close_all_base_timeline_gaps, close_base_timeline_gap, has_locked_video, move_scene_timing, normalize_segments, num, parse_bulk_scenes, renumber_generic_base_scene_labels,
    resize_scene_timing, shift_segment_timing, snap_to_beat, sort_segments,
};
use serde_json::{json, Value};

use crate::assets::{renumber_after_insert, renumber_after_removal, Renamed};
use crate::journal::{recover_pending, TimelineJournal};
use crate::layout::Stamp;
use crate::session::{load_session, persist_session};

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("Scene '{0}' was not found.")]
    SceneNotFound(String),
    #[error("{0}")]
    Validation(String),
    #[error("Project revision conflict: incoming revision {incoming} < saved revision {current}.")]
    Conflict { current: i64, incoming: i64 },
    #[error("{0}")]
    Io(#[from] io::Error),
}

pub type OpResult = Result<Value, OpError>;

fn int_of(session: &Obj, key: &str) -> i64 {
    session.get(key).filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64)
}

pub(crate) fn current_revision(session: &Obj) -> i64 {
    match int_of(session, "revision") {
        0 => int_of(session, "builder_save_revision"),
        n => n,
    }
}

fn path_key(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Point stored paths at renamed files. An exact match wins over a match on a parent folder.
pub fn rewrite_renamed_scene_paths(node: &mut Value, renamed: &Renamed) {
    if renamed.is_empty() {
        return;
    }
    let pairs: Vec<(String, String)> = renamed.iter().map(|(a, b)| (a.display().to_string(), b.display().to_string())).collect();
    fn rename(value: &str, pairs: &[(String, String)]) -> String {
        let key = path_key(value);
        // Python builds a dict, so the last pair with an equal key wins.
        if let Some((_, repl)) = pairs.iter().rev().find(|(orig, _)| path_key(orig) == key) {
            return repl.clone();
        }
        for (orig, repl) in pairs {
            let parent = format!("{}/", path_key(orig));
            if key.starts_with(&parent) {
                let suffix: String = value.chars().skip(orig.chars().count()).collect();
                return format!("{repl}{suffix}");
            }
        }
        value.to_string()
    }
    fn walk(node: &mut Value, pairs: &[(String, String)]) {
        match node {
            Value::Object(map) => {
                for (_, v) in map.iter_mut() {
                    match v {
                        Value::String(s) => *s = rename(s, pairs),
                        Value::Object(_) | Value::Array(_) => walk(v, pairs),
                        _ => {}
                    }
                }
            }
            Value::Array(items) => {
                for v in items.iter_mut() {
                    match v {
                        Value::String(s) => *s = rename(s, pairs),
                        Value::Object(_) | Value::Array(_) => walk(v, pairs),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    walk(node, &pairs);
}

fn renamed_json(renamed: &Renamed) -> Value {
    Value::Array(renamed.iter().map(|(a, b)| json!([a.display().to_string(), b.display().to_string()])).collect())
}

fn rewrite_session(session: &mut Obj, renamed: &Renamed) {
    let mut value = Value::Object(std::mem::take(session));
    rewrite_renamed_scene_paths(&mut value, renamed);
    if let Value::Object(map) = value {
        *session = map;
    }
}

/// Rewrite renamed paths in the session and in the scenes, which an edit holds apart from the session.
fn rewrite_all(session: &mut Obj, segs: &mut Vec<Value>, renamed: &Renamed) {
    rewrite_session(session, renamed);
    let mut value = Value::Array(std::mem::take(segs));
    rewrite_renamed_scene_paths(&mut value, renamed);
    if let Value::Array(items) = value {
        *segs = items;
    }
}

/// An open edit of a project: the loaded session and the revision check.
pub(crate) struct Edit {
    pub(crate) project: std::path::PathBuf,
    pub(crate) session: Obj,
}

pub(crate) fn open(project: &Path, if_match: Option<i64>) -> Result<Edit, OpError> {
    recover_pending(project);
    let loaded = load_session(project)?;
    let current = current_revision(&loaded.session);
    if let Some(incoming) = if_match.filter(|i| *i != current) {
        return Err(OpError::Conflict { current, incoming });
    }
    Ok(Edit { project: loaded.project_folder, session: loaded.session })
}

impl Edit {
    pub(crate) fn segments(&mut self) -> Vec<Value> {
        let value = self.session.entry("segments").or_insert_with(|| json!([])).take();
        match value {
            Value::Array(items) => items,
            _ => vec![],
        }
    }

    pub(crate) fn put_segments(&mut self, segs: Vec<Value>) {
        self.session.insert("segments".into(), Value::Array(segs));
    }

    pub(crate) fn save(self, now: &Stamp) -> Result<i64, OpError> {
        Ok(persist_session(&self.project, self.session, now.clone())?.revision)
    }
}

/// `segment.get("id") == scene_id or str(i + 1) == str(scene_id)`, whichever scene matches first.
fn find_first(segs: &[Value], scene_id: &str) -> Option<usize> {
    segs.iter().enumerate().position(|(i, s)| s.get("id").and_then(Value::as_str) == Some(scene_id) || (i + 1).to_string() == scene_id)
}

fn set(seg: &mut Value, key: &str, value: Value) {
    if let Some(map) = seg.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

fn str_of(seg: &Value, key: &str) -> String {
    text_or_empty(seg.get(key)).trim().to_string()
}

pub type NewId<'a> = &'a mut dyn FnMut() -> String;

/// Run the part of an operation that renames files and saves, rolling everything back on failure.
fn journaled(
    project: &Path,
    operation: &str,
    now: &Stamp,
    body: impl FnOnce(&mut TimelineJournal) -> Result<Value, OpError>,
) -> OpResult {
    let mut journal = TimelineJournal::new(project, operation, now.millis());
    journal.start(now.epoch)?;
    match body(&mut journal) {
        Ok(value) => {
            journal.commit();
            Ok(value)
        }
        Err(e) => {
            journal.rollback();
            Err(e)
        }
    }
}

/// Insert a scene at the end, or before or after another scene. Later scene files shift up one number.
#[allow(clippy::too_many_arguments)]
pub fn create_scene(
    project: &Path,
    position: &str,
    ref_scene: Option<&str>,
    duration: f64,
    label: &str,
    t2i_prompt: &str,
    i2v_prompt: &str,
    notes: &str,
    if_match: Option<i64>,
    now: &Stamp,
    new_id: NewId,
) -> OpResult {
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    sort_segments(&mut segs);
    let pos = position.to_lowercase();
    let dur = (if duration == 0.0 { 4.0 } else { duration }).max(0.1);
    let mut insert_idx = segs.len();
    let mut start_time = segs.last().map_or(0.0, |s| num(s, "end", 0.0));
    if matches!(pos.as_str(), "before" | "after") {
        if let Some(reference) = ref_scene.filter(|r| !r.is_empty()) {
            let ref_idx = find_first(&segs, reference).ok_or_else(|| OpError::SceneNotFound(reference.to_string()))?;
            if pos == "before" {
                insert_idx = ref_idx;
                start_time = num(&segs[ref_idx], "start", 0.0);
            } else {
                insert_idx = ref_idx + 1;
                start_time = num(&segs[ref_idx], "end", 0.0);
            }
        }
    }
    let slot = insert_idx as u32 + 1;
    let scene = json!({
        "id": new_id(),
        "label": if label.is_empty() { format!("Scene {slot}") } else { label.to_string() },
        "start": h3_core::pyval::round_to(start_time, 4),
        "end": h3_core::pyval::round_to(start_time + dur, 4),
        "t2i_prompt": t2i_prompt,
        "i2v_prompt": i2v_prompt,
        "notes": notes,
        "source": "inserted",
    });
    journaled(project, "create_scene", now, move |journal| {
        let mut renamed: Renamed = vec![];
        if insert_idx < segs.len() {
            renamed = renumber_after_insert(&edit.project, slot)?;
            journal.record_renames(&renamed, now.epoch)?;
            rewrite_all(&mut edit.session, &mut segs, &renamed);
            for s in segs[insert_idx..].iter_mut() {
                shift_segment_timing(s, dur);
            }
        }
        segs.insert(insert_idx, scene.clone());
        renumber_generic_base_scene_labels(&mut segs);
        edit.put_segments(segs);
        let revision = edit.save(now)?;
        Ok(json!({"scene": scene, "renamed": renamed_json(&renamed), "revision": revision}))
    })
}

/// Delete a scene. Its files are archived and later scene files shift down one number. `ripple` closes the gap.
pub fn delete_scene(project: &Path, scene_id: &str, ripple: bool, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    sort_segments(&mut segs);
    let idx = find_first(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    let slot = idx as u32 + 1;
    let (start, end) = (num(&segs[idx], "start", 0.0), num(&segs[idx], "end", 0.0));
    let deleted_id = segs[idx].get("id").cloned().unwrap_or(Value::Null);
    journaled(project, "delete_scene", now, move |journal| {
        let renamed = renumber_after_removal(&edit.project, slot, &now.compact)?;
        journal.record_renames(&renamed, now.epoch)?;
        rewrite_all(&mut edit.session, &mut segs, &renamed);
        segs.remove(idx);
        if ripple {
            let mut overlay = edit.session.get_mut("overlay_segments").map(Value::take).and_then(|v| if let Value::Array(a) = v { Some(a) } else { None });
            close_base_timeline_gap(&mut segs, overlay.as_deref_mut(), start, end);
            if let Some(o) = overlay {
                edit.session.insert("overlay_segments".into(), Value::Array(o));
            }
        }
        renumber_generic_base_scene_labels(&mut segs);
        edit.put_segments(segs);
        let revision = edit.save(now)?;
        Ok(json!({"deleted_scene_id": deleted_id, "renamed": renamed_json(&renamed), "revision": revision}))
    })
}

const MEDIA_KEYS: [&str; 6] = ["approved_image_path", "video_path", "video_output", "video_status", "image_history", "video_history"];

/// Split a scene at `at_time`. The lyric text is divided by time. `clear_right_media` drops the media on the new half.
pub fn split_scene(project: &Path, scene_id: &str, at_time: f64, clear_right_media: bool, if_match: Option<i64>, now: &Stamp, new_id: NewId) -> OpResult {
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    sort_segments(&mut segs);
    let idx = find_first(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    if has_locked_video(&segs[idx]) {
        return Err(OpError::Validation("Scene has rendered video. Clear or trim video before splitting.".into()));
    }
    let start = num(&segs[idx], "start", 0.0);
    let end = segs[idx].get("end").filter(|v| truthy(v)).and_then(to_float).unwrap_or(start);
    let t = at_time;
    if t < start + 0.05 || t > end - 0.05 {
        return Err(OpError::Validation(format!("Split time {}s must be between {}s and {}s.", py_float_str(t), py_float_str(start + 0.05), py_float_str(end - 0.05))));
    }
    let slot = idx as u32 + 2;
    let right_id = new_id();
    journaled(project, "split_scene", now, move |journal| {
        let mut renamed: Renamed = vec![];
        if slot as usize <= segs.len() {
            renamed = renumber_after_insert(&edit.project, slot)?;
            journal.record_renames(&renamed, now.epoch)?;
            rewrite_all(&mut edit.session, &mut segs, &renamed);
        }
        set(&mut segs[idx], "end", json!(h3_core::pyval::round_to(t, 4)));
        let mut right = segs[idx].clone();
        set(&mut right, "id", json!(right_id));
        set(&mut right, "start", json!(h3_core::pyval::round_to(t, 4)));
        set(&mut right, "end", json!(h3_core::pyval::round_to(end, 4)));
        set(&mut right, "source", json!("split"));
        if !str_of(&segs[idx], "lyric_text").is_empty() {
            let (l, r) = split_lyric_text(&text_or_empty(segs[idx].get("lyric_text")), (t - start) / (end - start).max(0.01));
            set(&mut segs[idx], "lyric_text", json!(l));
            set(&mut right, "lyric_text", json!(r));
            set(&mut segs[idx], "lyric_no_lip_sync", json!(is_instrumental_lyric_text(&l)));
            set(&mut right, "lyric_no_lip_sync", json!(is_instrumental_lyric_text(&r)));
        }
        if clear_right_media {
            if let Some(map) = right.as_object_mut() {
                for key in MEDIA_KEYS {
                    map.shift_remove(key);
                }
            }
            set(&mut right, "video_status", json!("none"));
        }
        let left = segs[idx].clone();
        segs.insert(idx + 1, right.clone());
        renumber_generic_base_scene_labels(&mut segs);
        edit.put_segments(segs);
        let revision = edit.save(now)?;
        Ok(json!({"left_scene": left, "right_scene": right, "renamed": renamed_json(&renamed), "revision": revision}))
    })
}

/// Merge a scene with its next or previous neighbour. Neither may have rendered video.
pub fn merge_scenes(project: &Path, scene_id: &str, with_direction: &str, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    sort_segments(&mut segs);
    let idx = find_first(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    let direction = with_direction.to_lowercase();
    let (left_idx, right_idx): (i64, i64) = match direction.as_str() {
        "next" => (idx as i64, idx as i64 + 1),
        "previous" => (idx as i64 - 1, idx as i64),
        _ => return Err(OpError::Validation(format!("Invalid merge direction '{with_direction}'. Must be 'next' or 'previous'."))),
    };
    if left_idx < 0 || right_idx as usize >= segs.len() {
        return Err(OpError::Validation(format!("Cannot merge scene {scene_id} with {direction}: no adjacent neighbor.")));
    }
    let (left_idx, right_idx) = (left_idx as usize, right_idx as usize);
    if has_locked_video(&segs[left_idx]) || has_locked_video(&segs[right_idx]) {
        return Err(OpError::Validation("Cannot merge scenes: one or both scenes already have rendered video.".into()));
    }
    let slot = right_idx as u32 + 1;
    journaled(project, "merge_scenes", now, move |journal| {
        let renamed = renumber_after_removal(&edit.project, slot, &now.compact)?;
        journal.record_renames(&renamed, now.epoch)?;
        rewrite_all(&mut edit.session, &mut segs, &renamed);
        let right = segs[right_idx].clone();
        let left = &mut segs[left_idx];
        let start = num(left, "start", 0.0).min(num(&right, "start", 0.0));
        let end = num(left, "end", 0.0).max(num(&right, "end", 0.0));
        set(left, "start", json!(start));
        set(left, "end", json!(end));
        let join = |a: &Value, b: &Value, key: &str, sep: &str| [str_of(a, key), str_of(b, key)].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(sep);
        let notes = join(left, &right, "notes", "\n\n");
        set(left, "notes", json!(notes));
        if !str_of(left, "lyric_text").is_empty() || !str_of(&right, "lyric_text").is_empty() {
            let merged = merge_lyric_text(&text_or_empty(left.get("lyric_text")), &text_or_empty(right.get("lyric_text")));
            set(left, "lyric_no_lip_sync", json!(is_instrumental_lyric_text(&merged)));
            set(left, "lyric_text", json!(merged));
        }
        let note = join(left, &right, "timeline_note", "\n");
        set(left, "timeline_note", json!(note));
        let merged_scene = left.clone();
        segs.remove(right_idx);
        renumber_generic_base_scene_labels(&mut segs);
        edit.put_segments(segs);
        let revision = edit.save(now)?;
        Ok(json!({"merged_scene": merged_scene, "renamed": renamed_json(&renamed), "revision": revision}))
    })
}

fn timing_result(edit: Edit, segs: Vec<Value>, scene_id: &str, delta: f64, now: &Stamp) -> OpResult {
    let scene = segs.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(scene_id)).or_else(|| segs.iter().enumerate().find(|(i, _)| (i + 1).to_string() == scene_id).map(|(_, s)| s)).cloned().unwrap_or(Value::Null);
    let mut edit = edit;
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"scene": scene, "delta": delta, "revision": revision}))
}

/// Move a scene to a new start time.
pub fn move_scene(project: &Path, scene_id: &str, start_time: f64, ripple: bool, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    let delta = move_scene_timing(&mut segs, scene_id, start_time, ripple).map_err(|_| OpError::SceneNotFound(scene_id.to_string()))?;
    timing_result(edit, segs, scene_id, delta, now)
}

/// Change a scene's length by duration or end time.
pub fn resize_scene(project: &Path, scene_id: &str, duration: Option<f64>, end_time: Option<f64>, ripple: bool, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    let delta = resize_scene_timing(&mut segs, scene_id, duration, end_time, ripple).map_err(|_| OpError::SceneNotFound(scene_id.to_string()))?;
    timing_result(edit, segs, scene_id, delta, now)
}

/// Close every gap between scenes.
pub fn close_gaps(project: &Path, now: &Stamp) -> OpResult {
    let mut edit = open(project, None)?;
    let mut segs = edit.segments();
    let mut overlay = match edit.session.entry("overlay_segments").or_insert_with(|| json!([])).take() {
        Value::Array(a) => a,
        _ => vec![],
    };
    let removed = close_all_base_timeline_gaps(&mut segs, Some(&mut overlay));
    edit.session.insert("overlay_segments".into(), Value::Array(overlay));
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"removed_duration": removed, "revision": revision}))
}

/// Snap a scene edge, or every scene start, to the nearest beat.
pub fn snap_to_beats(project: &Path, scope: &str, scene_id: Option<&str>, edge: &str, now: &Stamp) -> OpResult {
    let mut edit = open(project, None)?;
    let beats: Vec<Value> = crate::session_ops::session_beats(&edit.session);
    if beats.is_empty() {
        return Err(OpError::Validation("No beats loaded in project. Analyze audio first.".into()));
    }
    let mut segs = edit.segments();
    if scope == "edge" {
        let idx = scene_id.and_then(|id| find_first(&segs, id)).unwrap_or(0);
        if idx < segs.len() {
            let key = if edge == "start" { "start" } else { "end" };
            let snapped = snap_to_beat(num(&segs[idx], key, 0.0), &beats, 0.14);
            set(&mut segs[idx], key, json!(snapped));
            normalize_segments(&mut segs, idx);
        }
    } else if scope == "all_starts" {
        for (i, seg) in segs.iter_mut().enumerate() {
            if i > 0 && !has_locked_video(seg) {
                let snapped = snap_to_beat(num(seg, "start", 0.0), &beats, 0.14);
                set(seg, "start", json!(snapped));
            }
        }
        for i in 0..segs.len() {
            normalize_segments(&mut segs, i);
        }
    }
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"snapped": true, "revision": revision}))
}

/// Build or extend the timeline from text. A line may end with the words of its scene. `action` is `replace` or `append`.
pub fn timeline_bulk(project: &Path, text: &str, mode: &str, action: &str, append_start: f64, now: &Stamp, new_id: NewId) -> OpResult {
    let parsed = parse_bulk_scenes(text, mode).map_err(OpError::Validation)?;
    if parsed.is_empty() {
        return Err(OpError::Validation("No valid timing lines found in input.".into()));
    }
    if action != "replace" && action != "append" {
        return Err(OpError::Validation("action must be 'replace' or 'append'.".into()));
    }
    let mut edit = open(project, None)?;
    let mut segs = edit.segments();
    let explicit = parsed.iter().any(|p| p.get("lyric_text").is_some_and(truthy));
    let mut lyric_source = if explicit { "from_text" } else { "none" };
    let mut lyric_fields: Vec<Obj> = vec![Obj::new(); parsed.len()];
    if explicit {
        for (i, item) in parsed.iter().enumerate() {
            if let Some(lyric) = item.get("lyric_text").filter(|v| truthy(v)) {
                let t = text_or_empty(Some(lyric));
                lyric_fields[i].insert("lyric_text".into(), json!(t));
                lyric_fields[i].insert("lyric_no_lip_sync".into(), json!(is_instrumental_lyric_text(&t)));
            }
        }
    }
    let bulk_scene = |start: f64, end: f64, lyric: &Obj, id: String| -> Value {
        let mut scene = json!({"id": id, "start": start, "end": end, "source": "manual"});
        for key in ["lyric_text", "lyric_no_lip_sync", "lyric_singers"] {
            if let Some(v) = lyric.get(key) {
                scene[key] = v.clone();
            }
        }
        scene
    };
    let mut new_segments: Vec<Value> = Vec::new();
    if action == "replace" {
        if !explicit && segs.iter().any(|s| !str_of(s, "lyric_text").is_empty()) {
            let ranges: Vec<(f64, f64)> = parsed.iter().map(|p| (num(p, "start", 0.0), num(p, "end", 0.0))).collect();
            lyric_fields = carry_lyrics_over(&segs, &ranges);
            lyric_source = if lyric_fields.iter().any(|f| !f.is_empty()) { "carried_over" } else { "none" };
        }
        for (item, lyric) in parsed.iter().zip(&lyric_fields) {
            new_segments.push(bulk_scene(num(item, "start", 0.0), num(item, "end", 0.0), lyric, new_id()));
        }
        segs = new_segments.clone();
        edit.session.insert("timing_frozen".into(), json!(false));
    } else {
        let mut cursor = if append_start > 0.0 { append_start } else { segs.last().map_or(0.0, |s| num(s, "end", 0.0)) };
        for (item, lyric) in parsed.iter().zip(&lyric_fields) {
            let dur = num(item, "end", 0.0) - num(item, "start", 0.0);
            new_segments.push(bulk_scene(h3_core::pyval::round_to(cursor, 4), h3_core::pyval::round_to(cursor + dur, 4), lyric, new_id()));
            cursor += dur;
        }
        segs.extend(new_segments.clone());
    }
    renumber_generic_base_scene_labels(&mut segs);
    let scene_count = segs.len();
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    let with_lyrics = new_segments.iter().filter(|s| !str_of(s, "lyric_text").is_empty()).count();
    Ok(json!({"scene_count": scene_count, "scenes_with_lyrics": with_lyrics, "lyrics": lyric_source, "revision": revision}))
}
