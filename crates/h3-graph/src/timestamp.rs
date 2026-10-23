//! The lyric timing workflow: Stable-ts on the song, matched to the pasted lyrics. Port of
//! `_patch_timestamped_transcribe_api_prompt` in `runner/utility_workflows.py`.

use std::path::Path;

use h3_core::pyval::{bool_payload, float_payload, text_or_empty, Obj};
use serde_json::json;

use crate::api::{node_id_by_class, set_input};
use crate::env::{GraphError, Result};
use crate::inputs::{abspath, clean_path_text, template};

const SEGMENT_MODES: [&str; 5] = ["whisper_chunks", "reference_lines", "exact_reference_lines", "reference_stanzas", "reference_scene_words"];

fn text(request: &Obj, key: &str) -> String {
    text_or_empty(request.get(key))
}

/// The ComfyUI graph that times `reference_lyrics` against the song at `audio_path`.
pub fn timestamp_graph(request: &Obj) -> Result<Obj> {
    let (_, mut prompt) = template("lyrics_timestamp_api.json")?;
    let audio = clean_path_text(request.get("audio_path"));
    if audio.is_empty() {
        return Err(GraphError::new("Audio file path is empty."));
    }
    let audio = abspath(&audio);
    if !Path::new(&audio).is_file() {
        return Err(GraphError::new(format!("Audio file was not found: {audio}")));
    }
    let extractor = node_id_by_class(&prompt, "VRGDG_TimestampedLyricsExtractor", Some("962"))?;
    let stems = node_id_by_class(&prompt, "VRGDG_GetStems", Some("28:114"))?;
    set_input(&mut prompt, &stems, "audio_file_path", json!(audio))?;
    set_input(&mut prompt, &extractor, "reference_lyrics", json!(text(request, "reference_lyrics")))?;
    let language = text(request, "language");
    set_input(&mut prompt, &extractor, "language", json!(if language.is_empty() { "english".to_string() } else { language }))?;
    let mode = text(request, "segment_mode").trim().to_string();
    let mode = if SEGMENT_MODES.contains(&mode.as_str()) { mode } else { "reference_lines".to_string() };
    set_input(&mut prompt, &extractor, "segment_mode", json!(mode))?;
    set_input(&mut prompt, &extractor, "include_instrumental_gaps", json!(bool_payload(request, "include_instrumental_gaps", true)))?;
    let instrumental = text(request, "instrumental_text");
    set_input(&mut prompt, &extractor, "instrumental_text", json!(if instrumental.is_empty() { "[instrumental]".to_string() } else { instrumental }))?;
    set_input(&mut prompt, &extractor, "min_gap_seconds", json!(float_payload(request, "min_gap_seconds", 1.0, 0.0, 30.0)))?;
    set_input(&mut prompt, &extractor, "min_scene_seconds", json!(float_payload(request, "min_scene_seconds", 1.0, 1.0, 30.0)))?;
    set_input(&mut prompt, &extractor, "max_scene_seconds", json!(float_payload(request, "max_scene_seconds", 8.0, 1.0, 60.0)))?;
    set_input(&mut prompt, &extractor, "vocal_tail_padding_seconds", json!(float_payload(request, "vocal_tail_padding_seconds", 0.6, 0.0, 3.0)))?;
    let model = text(request, "model_name");
    let model = model.trim();
    set_input(&mut prompt, &extractor, "model_name", json!(if model.is_empty() { "large-v3" } else { model }))?;
    Ok(prompt)
}
