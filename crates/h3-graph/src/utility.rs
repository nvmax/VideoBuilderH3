//! Small utility graphs: clear memory, and the beat based scene timing used by Beat Mode of the lyric transcription. Ports of
//! `_build_clear_memory_prompt` in `runner/utility_workflows.py` and `_build_whisper_workflow_prompt` in
//! `prompt_creator/nodes.py`.

use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use serde_json::{json, Value};

use crate::api::{inputs, set_input};
use crate::env::{GraphError, Result};
use crate::inputs::template;

/// The graph that frees VRAM, RAM and the Gemma/GGUF models. Returns the template name and the graph.
pub fn clear_memory_graph() -> Result<(String, Obj)> {
    template("clear_memory_api.json")
}

/// `_payload_bool`: bools as they are, numbers by truth, text by its meaning, anything else the default.
fn payload_bool(value: Option<&Value>, default: bool) -> bool {
    match value {
        None | Some(Value::Null) => default,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(other) => {
            let text = text_or_empty(Some(other)).trim().to_lowercase();
            match text.as_str() {
                "true" | "1" | "yes" | "on" | "y" => true,
                "false" | "0" | "no" | "off" | "n" | "" => false,
                _ => default,
            }
        }
    }
}

/// `float(payload.get(key, default) or default)`.
fn number(payload: &Obj, key: &str, default: f64) -> Result<f64> {
    match payload.get(key).filter(|v| truthy(v)) {
        None => Ok(default),
        Some(value) => to_float(value).ok_or_else(|| match value {
            Value::String(s) => GraphError::new(format!("could not convert string to float: '{s}'")),
            _ => GraphError::new("float() argument must be a string or a real number"),
        }),
    }
}

/// `str(payload.get(key, default) or default)`, trimmed, with the default for blank text.
fn text_or(payload: &Obj, key: &str, default: &str) -> String {
    let text = text_or_empty(payload.get(key));
    let text = text.trim();
    if text.is_empty() { default.to_string() } else { text.to_string() }
}

fn patch(prompt: &mut Obj, id: &str, pairs: &[(&str, Value)]) -> Result<()> {
    for (name, value) in pairs {
        set_input(prompt, id, name, value.clone())?;
    }
    Ok(())
}

/// What a Beat Mode graph is built for.
pub struct BeatSrtRequest<'a> {
    /// The song's name in ComfyUI's input folder, for the audio upload node.
    pub audio_upload_name: &'a str,
    /// The song's full path in ComfyUI's input folder, for the stem splitter.
    pub staged_audio_path: &'a str,
    /// The time stamp the SRT name carries (`%Y%m%d_%H%M%S`).
    pub stamp: &'a str,
}

/// The name of the SRT file the Beat Mode graph writes.
pub fn beat_srt_filename(stamp: &str) -> String {
    format!("builder_segments_{stamp}.srt")
}

/// The Beat Mode graph: scene lengths from the beats of the song, matched to the pasted lyrics. Returns the graph.
pub fn beat_srt_graph(payload: &Obj, request: &BeatSrtRequest) -> Result<Obj> {
    let (_, mut prompt) = template("beat_srt_api.json")?;
    let min_duration = number(payload, "min_duration", 4.0)?;
    let max_duration = number(payload, "max_duration", 10.0)?;
    let bias = number(payload, "bias", 0.7)?;
    let duration_preset = Some(text_or_empty(payload.get("duration_preset"))).filter(|t| !t.is_empty()).unwrap_or_else(|| "varied_no_repeat".into());
    let use_srt_durations = payload_bool(payload.get("use_srt_durations"), true);
    let fixed_scene_duration = number(payload, "fixed_scene_duration", 4.0)?;
    let empty_segment_text = text_or(payload, "empty_segment_text", "Instrumental section.");
    let whisper_language = text_or(payload, "whisper_language", "english");
    let full_lyrics = text_or_empty(payload.get("full_lyrics"));
    let output_filename = beat_srt_filename(request.stamp);
    let has = |prompt: &Obj, id: &str| prompt.contains_key(id);

    if has(&prompt, "954") {
        patch(&mut prompt, "954", &[("audio", json!(request.audio_upload_name))])?;
    } else if has(&prompt, "964") {
        patch(&mut prompt, "964", &[("audio", json!(request.audio_upload_name))])?;
    }
    if has(&prompt, "28:114") {
        patch(&mut prompt, "28:114", &[("audio_file_path", json!(request.staged_audio_path))])?;
    } else if has(&prompt, "955") && inputs(&prompt, "955").is_some_and(|i| i.contains_key("audio_file_path")) {
        patch(&mut prompt, "955", &[("audio_file_path", json!(request.staged_audio_path))])?;
    }
    if crate::api::class_of(&prompt, "955") == "VRGDG_TextBox" {
        patch(&mut prompt, "955", &[("text", json!(full_lyrics))])?;
    }
    if has(&prompt, "960") {
        patch(
            &mut prompt,
            "960",
            &[
                ("scene_duration_seconds", json!(fixed_scene_duration)),
                ("reference_lyrics", json!(full_lyrics)),
                ("language", json!(whisper_language)),
                ("strict_reference_text", json!(true)),
                ("preserve_nonvocal_segments", json!(true)),
                ("alignment_min_words", json!(1)),
            ],
        )?;
    }
    if has(&prompt, "28:933") {
        patch(&mut prompt, "28:933", &[("switch", json!(use_srt_durations))])?;
    }
    if has(&prompt, "28:887") {
        patch(&mut prompt, "28:887", &[("use_srt_durations", json!(use_srt_durations))])?;
    }
    if has(&prompt, "28:920") {
        patch(&mut prompt, "28:920", &[("use_srt_file", json!(use_srt_durations))])?;
    }
    if has(&prompt, "28:949") {
        patch(&mut prompt, "28:949", &[("empty_segment_text", json!(empty_segment_text))])?;
    }
    let duration_node = if has(&prompt, "28:80") { "28:80" } else { "963" };
    if !has(&prompt, duration_node) {
        return Err(GraphError::new(format!("'Hidden Whisper workflow node {duration_node} was not found.'")));
    }
    patch(
        &mut prompt,
        duration_node,
        &[
            ("min_duration", json!(min_duration)),
            ("max_duration", json!(max_duration)),
            ("bias", json!(bias)),
            ("duration_preset", json!(duration_preset)),
            ("output_filename", json!(output_filename)),
        ],
    )?;
    Ok(prompt)
}
