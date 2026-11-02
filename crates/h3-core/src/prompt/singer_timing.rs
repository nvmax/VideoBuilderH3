//! Timing a multi-shot singing scene before its MiniMax prompt is written. Port of `time_scene_singer_cues` in
//! `agent_api/orchestrator/lyrics_orchestrator.py` (everything that needs no ComfyUI) and of `shot_aligned_singer_cues` in
//! `minimax/shot_prompt.py`.
//!
//! A scene that cuts between shots and has a lyric to sing needs to know which words fall in which shot. Stable-ts times the
//! scene's own audio clip, and [`shot_aligned_singer_cues`] places the words into the shots. [`plan_singer_timing`] decides whether
//! that is needed and what to time. The ComfyUI run itself is done by the caller.

use serde_json::{json, Value};

use super::plan::exact_duration;
use crate::lyric_scenes::is_instrumental_lyric_text;
use crate::pyval::{py_str, round_to, text_or_empty, to_float, truthy, Obj};

/// What Stable-ts has to time for one scene.
#[derive(Debug, Clone)]
pub struct TimingPlan {
    /// The scene's number on the timeline (1 based), as the messages name it.
    pub scene_number: usize,
    /// The subject cast as the singer: every timed word is sung by them.
    pub performer: Obj,
    /// The request for `prepare_scene_audio_clip`: the scene's own audio, cut from the song or the custom clip.
    pub clip: Obj,
    /// The words Stable-ts matches to the audio.
    pub reference_lyrics: String,
    /// The scene's length in seconds.
    pub duration: f64,
    /// The scene's shot plan, which gives the cut times.
    pub cut_plan: Obj,
}

impl TimingPlan {
    /// The request for the timestamp workflow, once the scene's audio clip exists at `audio_path`.
    pub fn graph_request(&self, audio_path: &str) -> Obj {
        let mut request = Obj::new();
        request.insert("audio_path".into(), json!(audio_path));
        request.insert("reference_lyrics".into(), json!(self.reference_lyrics));
        request.insert("language".into(), json!("english"));
        request.insert("segment_mode".into(), json!("reference_scene_words"));
        request.insert("include_instrumental_gaps".into(), json!(true));
        request.insert("instrumental_text".into(), json!("[instrumental]"));
        request.insert("min_gap_seconds".into(), json!(0.5));
        request.insert("min_scene_seconds".into(), json!(1.0));
        request.insert("max_scene_seconds".into(), json!(self.duration.max(1.0)));
        request.insert("vocal_tail_padding_seconds".into(), json!(0.0));
        request.insert("model_name".into(), json!("large-v3"));
        request
    }

    /// The scene fields to save once `payload` (the timed words) is back. An empty result means no word could be placed.
    pub fn cue_fields(&self, payload: &Obj) -> Result<Obj, String> {
        let cues = shot_aligned_singer_cues(payload, &self.cut_plan, &self.performer);
        if cues.is_empty() {
            return Err(format!("Scene {}: Whisper returned no word timings; prompt generation was stopped.", self.scene_number));
        }
        let mut fields = Obj::new();
        fields.insert("lyric_cue_map".into(), Value::Array(cues));
        fields.insert("lyric_performance_mode".into(), json!("cue_map"));
        fields.insert("lyric_shot_word_timing_enabled".into(), json!(true));
        Ok(fields)
    }
}

/// `float(value)` for a number or a numeric string.
fn float_of(value: Option<&Value>) -> Option<f64> {
    value.and_then(to_float)
}

fn finite(value: Option<&Value>) -> Option<f64> {
    float_of(value).filter(|n| n.is_finite())
}

/// `float(value or default)`.
fn float_or(value: Option<&Value>, default: f64) -> f64 {
    match value {
        Some(v) if truthy(v) => to_float(v).unwrap_or(default),
        _ => default,
    }
}

/// `str(value or "")` stripped.
fn trimmed(value: Option<&Value>) -> String {
    match value {
        Some(v) if truthy(v) => py_str(Some(v)).trim().to_string(),
        _ => String::new(),
    }
}

/// `re.split(r"[,;\n]+", text)`: runs of separators count once, and a separator at either end leaves an empty part.
fn split_names(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_separator = false;
    for ch in text.chars() {
        if matches!(ch, ',' | ';' | '\n') {
            if !in_separator {
                parts.push(std::mem::take(&mut current));
                in_separator = true;
            }
        } else {
            in_separator = false;
            current.push(ch);
        }
    }
    parts.push(current);
    parts
}

/// `session_audio_path`: the project's audio, preferring a path that exists.
pub fn session_audio_path(session: &Obj) -> String {
    let candidates: Vec<String> = ["audio_path", "audio_file"].iter().map(|key| trimmed(session.get(*key))).filter(|path| !path.is_empty()).collect();
    candidates.iter().find(|path| std::path::Path::new(path).is_file()).or(candidates.first()).cloned().unwrap_or_default()
}

/// A saved cue map whose every row has a start and a later end (a row without an end ends where the next one starts) needs no new timing.
fn cue_map_is_complete(existing: &[Value], duration: f64) -> bool {
    for (position, cue) in existing.iter().enumerate() {
        let Some(cue) = cue.as_object() else { return false };
        let start = finite(cue.get("start").filter(|v| !v.is_null()));
        let end = match cue.get("end").filter(|v| !v.is_null()) {
            Some(end) => finite(Some(end)),
            None if position + 1 < existing.len() => existing[position + 1].as_object().and_then(|next| finite(next.get("start").filter(|v| !v.is_null()))),
            None => Some(duration),
        };
        match (start, end) {
            (Some(start), Some(end)) if end > start => {}
            _ => return false,
        }
    }
    true
}

/// The subject the scene's singers map to: the Reference Builder's performer map, else the scene's own `lyric_singers`.
fn performer_of(session: &Obj, segment: &Obj, index: usize) -> Option<Obj> {
    let refs = session.get("flux_reference_builder").filter(|v| truthy(v)).and_then(Value::as_object);
    let map = refs.and_then(|refs| refs.get("performer_scene_map")).and_then(Value::as_object);
    let by_id = map.and_then(|map| map.get(&py_str(segment.get("id")))).filter(|v| truthy(v));
    let by_number = map.and_then(|map| map.get(&(index + 1).to_string())).filter(|v| truthy(v));
    let own = segment.get("lyric_singers").filter(|v| truthy(v));
    let selected: Vec<String> = match by_id.or(by_number).or(own) {
        Some(Value::String(text)) => split_names(text).iter().map(|name| name.trim().to_lowercase()).collect(),
        Some(Value::Array(items)) => items.iter().map(|item| py_str(Some(item)).trim().to_lowercase()).collect(),
        _ => Vec::new(),
    };
    refs?
        .get("subjects")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .find(|subject| selected.contains(&text_or_empty(subject.get("id")).to_lowercase()) || selected.contains(&text_or_empty(subject.get("name")).to_lowercase()))
        .cloned()
}

/// Whether the scene needs its sung words timed before its prompt is written, and what to time.
///
/// `Ok(None)` means it does not: the scene has no cuts, nobody sings in it, or its cue map is already timed. An error says what to
/// set up first (a singer, the audio).
pub fn plan_singer_timing(session: &Obj, folder: &str, segment: &Obj, index: usize, cut_plan: &Obj) -> Result<Option<TimingPlan>, String> {
    let lyric = trimmed(segment.get("lyric_text"));
    let has_cuts = cut_plan.get("cut_times_seconds").is_some_and(truthy);
    let performance = Some(text_or_empty(segment.get("performance_mode"))).filter(|s| !s.is_empty()).or_else(|| Some(text_or_empty(session.get("video_type"))).filter(|s| !s.is_empty())).unwrap_or_else(|| "singing".into());
    if !has_cuts
        || segment.get("no_character_present").is_some_and(truthy)
        || segment.get("lyric_no_lip_sync").is_some_and(truthy)
        || lyric.is_empty()
        || is_instrumental_lyric_text(&lyric)
        || performance != "singing"
    {
        return Ok(None);
    }
    let duration = exact_duration(cut_plan);
    let existing: Vec<Value> = segment.get("lyric_cue_map").and_then(Value::as_array).cloned().unwrap_or_default();
    if segment.get("lyric_performance_mode").and_then(Value::as_str) == Some("cue_map") && !existing.is_empty() && cue_map_is_complete(&existing, duration) {
        return Ok(None);
    }
    let number = index + 1;
    let performer = performer_of(session, segment, index).ok_or_else(|| format!("Scene {number}: auto-time-before-prompt needs a mapped singer."))?;
    let custom_audio = trimmed(segment.get("custom_audio_path"));
    let source = if custom_audio.is_empty() { session_audio_path(session) } else { custom_audio.clone() };
    if source.is_empty() {
        return Err(format!("Scene {number}: load project audio or add custom scene audio before auto-timing cues."));
    }
    let (start, length) = if custom_audio.is_empty() {
        (float_or(segment.get("start"), 0.0), duration)
    } else {
        (float_or(segment.get("custom_audio_source_start"), 0.0), float_or(segment.get("custom_audio_duration"), duration))
    };
    let mut clip = Obj::new();
    clip.insert("audio_path".into(), json!(source));
    clip.insert("project_folder".into(), json!(folder));
    clip.insert("scene_number".into(), json!(number));
    clip.insert("start_seconds".into(), json!(start));
    clip.insert("duration_seconds".into(), json!(length));
    let sung: Vec<String> = existing
        .iter()
        .filter_map(Value::as_object)
        .filter(|cue| cue.get("type").and_then(Value::as_str) != Some("instrumental"))
        .map(|cue| trimmed(cue.get("text")))
        .filter(|text| !text.is_empty())
        .collect();
    let reference_lyrics = if sung.is_empty() { lyric } else { sung.join("\n") };
    Ok(Some(TimingPlan { scene_number: number, performer, clip, reference_lyrics, duration, cut_plan: cut_plan.clone() }))
}

struct Word {
    text: String,
    start: f64,
    end: f64,
}

/// Assign the timed words to the scene's shots, moving a cut out of a word when a gap lies within a second of it
/// (`shot_aligned_singer_cues`). Rows are scene relative; a shot with no word is an instrumental row.
pub fn shot_aligned_singer_cues(payload: &Obj, cut_plan: &Obj, performer: &Obj) -> Vec<Value> {
    let duration = exact_duration(cut_plan);
    let mut words: Vec<Word> = Vec::new();
    for segment in payload.get("segments").and_then(Value::as_array).into_iter().flatten() {
        let Some(segment) = segment.as_object() else { continue };
        if segment.get("type").and_then(Value::as_str) == Some("instrumental") {
            continue;
        }
        for word in segment.get("words").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object) {
            let text = Some(text_or_empty(word.get("text"))).filter(|t| !t.is_empty()).unwrap_or_else(|| text_or_empty(word.get("word")));
            let text = text.trim().to_string();
            let (start, end) = (word.get("start").filter(|v| !v.is_null()), word.get("end").filter(|v| !v.is_null()));
            if let (false, Some(start), Some(end)) = (text.is_empty(), finite(start), finite(end)) {
                if end > start {
                    words.push(Word { text, start: start.max(0.0), end: end.max(0.0) });
                }
            }
        }
    }
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    if words.is_empty() {
        return Vec::new();
    }
    let splits_word = |time: f64| words.iter().any(|word| word.start - 0.08 < time && time < word.end - 0.02);

    let mut gaps = vec![(words[0].start - 0.1).max(0.0), words[words.len() - 1].end + 0.1];
    gaps.extend(words.windows(2).filter(|pair| pair[1].start > pair[0].end).map(|pair| (pair[0].end + pair[1].start) / 2.0));
    let mut cuts: Vec<f64> = cut_plan.get("cut_times_seconds").and_then(Value::as_array).into_iter().flatten().filter_map(|v| float_of(Some(v))).filter(|time| 0.001 < *time && *time < duration - 0.001).collect();
    cuts.sort_by(f64::total_cmp);
    let mut placed: Vec<f64> = Vec::new();
    for (position, original) in cuts.iter().enumerate() {
        let mut cut = *original;
        let previous = placed.last().copied().unwrap_or(0.0);
        let following = cuts.get(position + 1).copied().unwrap_or(duration);
        if splits_word(cut) {
            let mut best: Option<f64> = None;
            for time in gaps.iter().copied().filter(|time| (time - cut).abs() <= 1.0 && time - previous >= 1.0 && following - time >= 1.0 && !splits_word(*time)) {
                if best.is_none_or(|current| (time - cut).abs() < (current - cut).abs()) {
                    best = Some(time);
                }
            }
            if let Some(time) = best {
                cut = time;
            }
        }
        if cut > previous + 0.001 {
            placed.push(round_to(cut, 3));
        }
    }
    let mut boundaries = vec![0.0];
    boundaries.extend(placed);
    boundaries.push(duration);
    let id = text_or_empty(performer.get("id"));
    let name = text_or_empty(performer.get("name"));
    let last = boundaries.len() - 2;
    let mut cues = Vec::new();
    for (index, pair) in boundaries.windows(2).enumerate() {
        let (start, end) = (pair[0], pair[1]);
        let selected: Vec<&Word> = words
            .iter()
            .filter(|word| {
                let middle = (word.start + word.end) / 2.0;
                start <= middle && if index == last { middle <= end } else { middle < end }
            })
            .collect();
        let sung = !selected.is_empty();
        cues.push(json!({
            "type": if sung { "vocal" } else { "instrumental" },
            "text": selected.iter().map(|word| word.text.as_str()).collect::<Vec<_>>().join(" "),
            "action_note": "",
            "singer_id": if sung { id.clone() } else { String::new() },
            "singer_name": if sung { name.clone() } else { String::new() },
            "start": start,
            "end": end,
            "vocal_start": if sung { json!(start.max(selected[0].start)) } else { Value::Null },
            "vocal_end": if sung { json!(end.min(selected[selected.len() - 1].end)) } else { Value::Null },
        }));
    }
    cues
}
