//! Which scenes a stitch uses and how it is laid out. Port of `build_stitch_payload` and its helpers in
//! `agent_api/orchestrator/video_orchestrator.py` (`stitchRenderedScenes` in the Video Builder).
//!
//! * One timing item per scene and 24 fps: the stitcher cuts or pads every clip to `round(end * 24) - round(start * 24)`
//!   frames and muxes without `-shortest`, so the output has exactly the timeline's frames.
//! * A preview of some scenes shifts the timing by a whole number of frames and cuts the song to the selection.
//! * `audio`: `auto` uses the scenes' own audio when every selected scene renders with built-in audio, the project song
//!   otherwise. `embedded` and `project` force one or the other.

use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_core::settings::settings_for_scene;
use h3_media::stitch::{OverlayItem, StitchRequest, TimingItem};
use serde_json::{json, Value};

pub const STITCH_TIMELINE_FPS: u32 = 24;
const AUDIO_MODES: [&str; 3] = ["auto", "embedded", "project"];

/// What a stitch will do, for reports.
#[derive(Debug, Clone, PartialEq)]
pub struct StitchSummary {
    pub scene_ids: Vec<String>,
    pub scene_numbers: Vec<usize>,
    pub skipped_scene_ids: Vec<String>,
    pub audio: &'static str,
    pub audio_start: f64,
    pub audio_duration: f64,
    pub expected_frame_count: Option<i64>,
}

fn time_of(segment: &Obj, key: &str) -> f64 {
    segment.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

fn round_half_up(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// Python's `repr` of a loose value, for messages that echo what the caller sent.
fn py_repr(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".into(),
        Some(Value::String(s)) => {
            if s.contains('\'') && !s.contains('"') { format!("\"{s}\"") } else { format!("'{}'", s.replace('\'', "\\'")) }
        }
        other => h3_core::pyval::py_str(other),
    }
}

fn bad_selection() -> String {
    "scene_ids must be a list of scene numbers (1 = first scene) or scene ids.".to_string()
}

/// Indexes into `segments` for `scene_ids` (scene numbers, 1 = first, or scene ids), in timeline order. No `scene_ids`
/// selects every scene.
fn select_segments(segments: &[Obj], scene_ids: Option<&Value>) -> Result<Vec<usize>, String> {
    let mut indexes: Vec<usize> = match scene_ids {
        None | Some(Value::Null) => (0..segments.len()).collect(),
        Some(Value::Array(list)) if list.is_empty() => (0..segments.len()).collect(),
        Some(Value::Array(list)) => {
            let mut by_key: Vec<(String, usize)> = Vec::new();
            for index in 0..segments.len() {
                by_key.push(((index + 1).to_string(), index));
            }
            for (index, segment) in segments.iter().enumerate() {
                let id = text_or_empty(segment.get("id")).trim().to_string();
                if !id.is_empty() {
                    match by_key.iter_mut().find(|(k, _)| *k == id) {
                        Some(entry) => entry.1 = index,
                        None => by_key.push((id, index)),
                    }
                }
            }
            let mut chosen: Vec<usize> = Vec::new();
            let mut unknown: Vec<String> = Vec::new();
            for raw in list {
                let key = match raw {
                    Value::String(s) => s.trim().to_string(),
                    Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
                    _ => return Err(bad_selection()),
                };
                match by_key.iter().find(|(k, _)| *k == key) {
                    Some((_, index)) => {
                        if !chosen.contains(index) {
                            chosen.push(*index);
                        }
                    }
                    None => unknown.push(key),
                }
            }
            if !unknown.is_empty() {
                return Err(format!(
                    "Unknown scene(s) for stitch: {}. Use a scene number (1 = first scene, {} scenes) or a scene id.",
                    unknown.join(", "),
                    segments.len()
                ));
            }
            chosen
        }
        Some(_) => return Err(bad_selection()),
    };
    // Like the Builder's preview stitch, play the scenes in timeline order whatever order they were named in.
    indexes.sort_by(|a, b| time_of(&segments[*a], "start").total_cmp(&time_of(&segments[*b], "start")).then(a.cmp(b)));
    Ok(indexes)
}

fn session_audio_path(session: &Obj) -> String {
    let candidates: Vec<String> =
        ["audio_path", "audio_file"].iter().map(|k| text_or_empty(session.get(*k)).trim().to_string()).filter(|p| !p.is_empty()).collect();
    candidates.iter().find(|p| Path::new(p).is_file()).or(candidates.first()).cloned().unwrap_or_default()
}

/// Build the stitch request the Video Builder would send, and a summary. `strict` (the stitch command) makes a selected
/// scene without a video file an error. The stitch at the end of a batch passes `false` and leaves such scenes out.
pub fn build_stitch_request(folder: &Path, session: &Obj, params: &Obj, strict: bool) -> Result<(StitchRequest, StitchSummary), String> {
    let segments: Vec<Obj> = session.get("segments").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_object).cloned().collect()).unwrap_or_default();
    let audio = match params.get("audio") {
        Some(v) if truthy(v) => text_or_empty(Some(v)).trim().to_lowercase(),
        _ => "auto".to_string(),
    };
    if !AUDIO_MODES.contains(&audio.as_str()) {
        return Err(format!("audio must be one of {} (got {}).", AUDIO_MODES.join(", "), py_repr(params.get("audio"))));
    }
    let overlays_value = params.get("overlays").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([]));
    let Some(overlay_list) = overlays_value.as_array() else {
        return Err("overlays must be a list of {path, start, end, source_start} items.".to_string());
    };
    let output_prefix = match params.get("output_prefix").filter(|v| truthy(v)) {
        None => "FINAL_VIDEO".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err("output_prefix must be a string.".to_string()),
    };
    let explicit_selection = params.get("scene_ids").is_some_and(truthy);
    let indexes = select_segments(&segments, params.get("scene_ids"))?;

    let mut selected: Vec<(usize, &Obj, PathBuf)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for index in indexes {
        let segment = &segments[index];
        let path = ["video_path", "rendered_video_path"].iter().filter_map(|k| segment.get(*k)).find(|v| truthy(v)).map(|v| text_or_empty(Some(v)).trim().to_string()).unwrap_or_default();
        if path.is_empty() || !Path::new(&path).is_file() {
            let id = text_or_empty(segment.get("id").filter(|v| truthy(v)));
            missing.push(if id.is_empty() { (index + 1).to_string() } else { id });
            continue;
        }
        selected.push((index, segment, PathBuf::from(path)));
    }
    if !missing.is_empty() && strict {
        return Err(format!("Scene(s) without a rendered video: {}. Render them or leave them out of scene_ids.", missing.join(", ")));
    }
    if selected.is_empty() {
        return Err("No scene video files available to stitch.".to_string());
    }

    let project_session = Value::Object(session.clone());
    let embedded = if audio == "auto" {
        selected.iter().all(|(_, segment, _)| {
            settings_for_scene(&project_session, Some(&Value::Object((*segment).clone()))).get("audio_mode").and_then(Value::as_str) == Some("built_in_audio")
        })
    } else {
        audio == "embedded"
    };

    let override_audio = text_or_empty(params.get("audio_path")).trim().to_string();
    let chosen_audio = if override_audio.is_empty() { session_audio_path(session) } else { override_audio };
    let (mut audio_path, mut song_path): (Option<PathBuf>, Option<PathBuf>) = (None, None);
    if embedded {
        // The scenes play their own audio. The song only fills audio that runs short, or plays continuously when the
        // clips' audio is the song.
        if Path::new(&chosen_audio).is_file() {
            song_path = Some(PathBuf::from(&chosen_audio));
        }
    } else {
        if !Path::new(&chosen_audio).is_file() {
            return Err("The project has no audio file to stitch with. Attach the song, pass audio_path, or use audio: embedded.".to_string());
        }
        audio_path = Some(PathBuf::from(&chosen_audio));
    }

    let start_time = selected.iter().map(|(_, s, _)| time_of(s, "start")).fold(f64::INFINITY, f64::min);
    let end_time = selected.iter().map(|(_, s, _)| time_of(s, "end")).fold(f64::NEG_INFINITY, f64::max);
    let (mut audio_start, mut audio_duration, mut timeline_offset) = (0.0, 0.0, 0.0);
    if explicit_selection {
        let contiguous = selected.windows(2).all(|w| w[1].0 == w[0].0 + 1);
        if !embedded && !contiguous {
            return Err("A stitch with the project song needs contiguous scenes, so one window of the song fits them. Pick one continuous scene range, or use audio: embedded.".to_string());
        }
        timeline_offset = start_time;
        audio_start = start_time;
        audio_duration = (end_time - start_time).max(0.1);
    }

    // A whole number of frames, so each clip rounds to the same frame count as on the full timeline.
    let fps = f64::from(STITCH_TIMELINE_FPS);
    let aligned_offset = round_half_up(timeline_offset * fps) as f64 / fps;
    let mut timing: Vec<TimingItem> = Vec::new();
    let mut expected_frames = 0i64;
    for (_, segment, _) in &selected {
        let start = (time_of(segment, "start") - aligned_offset).max(0.0);
        let end = (time_of(segment, "end") - aligned_offset).max(0.0);
        timing.push(TimingItem { start, end });
        // Same rounding as the stitcher.
        expected_frames += ((start.max(end) * fps + 0.5) as i64 - (start * fps + 0.5) as i64).max(1);
    }

    let mut overlays: Vec<OverlayItem> = Vec::new();
    for item in overlay_list {
        let Some(map) = item.as_object() else { continue };
        let number = |key: &str| map.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0);
        overlays.push(OverlayItem {
            path: PathBuf::from(text_or_empty(map.get("path")).trim().to_string()),
            start: number("start"),
            end: number("end"),
            source_start: number("source_start"),
        });
    }
    let request = StitchRequest {
        project: folder.to_path_buf(),
        scene_paths: selected.iter().map(|(_, _, p)| p.clone()).collect(),
        audio_path,
        song_path,
        scene_audio_items: vec![],
        scene_timing: timing,
        timeline_fps: STITCH_TIMELINE_FPS,
        use_embedded_scene_audio: embedded,
        overlays,
        width: 0,
        height: 0,
        audio_start,
        audio_duration,
        output_prefix,
    };
    let summary = StitchSummary {
        scene_ids: selected
            .iter()
            .map(|(index, segment, _)| {
                let id = text_or_empty(segment.get("id").filter(|v| truthy(v)));
                if id.is_empty() { (index + 1).to_string() } else { id }
            })
            .collect(),
        scene_numbers: selected.iter().map(|(index, _, _)| index + 1).collect(),
        skipped_scene_ids: missing,
        audio: if embedded { "embedded" } else { "project" },
        audio_start,
        audio_duration,
        expected_frame_count: Some(expected_frames),
    };
    Ok((request, summary))
}
