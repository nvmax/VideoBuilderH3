//! The graphs behind Line Mapping and the lyric transcription: the song put where ComfyUI can read it, and the reply with the
//! graph and where its SRT will land. Port of `_build_whisper_workflow_prompt` and `_stage_audio_for_upload_node` in
//! `prompt_creator/nodes.py`.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_graph::utility::{beat_srt_filename, beat_srt_graph, BeatSrtRequest};
use h3_project::layout::{context_folder, prompts_folder, srt_path, Stamp};
use h3_project::project_copy::copy2;
use serde_json::{json, Value};

use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

/// `_safe_file_name`: the last part of the name with `<>:"/\|?*` made `_`.
fn safe_file_name(name: &str, fallback: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out = String::new();
    let mut pending = false;
    for c in base.chars() {
        if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            if !pending {
                out.push('_');
            }
            pending = true;
        } else {
            out.push(c);
            pending = false;
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() { fallback.to_string() } else { out }
}

fn modified_seconds(path: &Path) -> i64 {
    fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// `_stage_audio_for_upload_node`: copy the song into ComfyUI's input folder. Returns its name there and its full path.
pub fn stage_audio_for_upload(input_dir: &Path, audio_path: &str) -> Result<(String, String), String> {
    let raw = audio_path.trim().trim_matches('"');
    if raw.is_empty() {
        return Err("Choose an audio file before running Prompt Creator.".into());
    }
    let mut source = absolute(Path::new(raw));
    if !source.is_file() {
        let candidate = input_dir.join(raw);
        if candidate.is_file() {
            source = absolute(&candidate);
        } else {
            return Err(format!("Audio file was not found: {raw}"));
        }
    }
    fs::create_dir_all(input_dir).map_err(|e| e.to_string())?;
    let extension = source.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_else(|| ".wav".into());
    let name = safe_file_name(&source.display().to_string(), &format!("vrgdg_prompt_creator_audio{extension}"));
    let staged = absolute(&input_dir.join(&name));
    if source != staged {
        let source_size = fs::metadata(&source).map_or(0, |m| m.len());
        let current = staged.is_file() && fs::metadata(&staged).map_or(0, |m| m.len()) == source_size && modified_seconds(&staged) == modified_seconds(&source);
        if !current {
            copy2(&source, &staged).map_err(|e| format!("{}: {e}", staged.display()))?;
        }
    }
    let shown = staged.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
    Ok((shown, staged.display().to_string()))
}

/// `POST /vrgdg/music_prompt_creator/build_whisper_prompt`: the Beat Mode graph, with the song staged for it.
pub fn build_whisper_prompt(env: &Env, input_dir: &Path, payload: &Obj, now: &Stamp) -> Reply {
    let project = {
        let raw = path_text(payload, "project_folder");
        if raw.is_empty() {
            let name = text_or_empty(payload.get("project_name")).trim().to_string();
            let name = if name.is_empty() { format!("VRGDG_Project_{}", now.compact) } else { name };
            env.output.join(h3_project::derived::safe_project_name(&name))
        } else {
            absolute(Path::new(&raw))
        }
    };
    for folder in [project.clone(), context_folder(&project), prompts_folder(&project)] {
        fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    let audio = payload.get("audio_path").filter(|v| truthy(v)).or_else(|| payload.get("audio_file")).map(|v| text_or_empty(Some(v))).unwrap_or_default();
    let (upload_name, staged) = stage_audio_for_upload(input_dir, &audio)?;
    let request = BeatSrtRequest { audio_upload_name: &upload_name, staged_audio_path: &staged, stamp: &now.compact };
    let prompt = beat_srt_graph(payload, &request).map_err(|e| e.to_string())?;
    Ok(json!({
        "workflow_template_path": "beat_srt_api.json",
        "prompt": Value::Object(prompt),
        "project_folder": project.display().to_string(),
        "expected_srt_path": srt_path(&project).display().to_string(),
        "source_srt_filename": beat_srt_filename(&now.compact),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp { compact: "20260101_000000".into(), iso: "2026-01-01T00:00:00".into(), epoch: 1_767_225_600.0 }
    }

    fn env(dir: &Path) -> Env {
        Env { output: dir.join("output"), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: dir.join("luts") }
    }

    #[test]
    fn the_song_is_copied_once_and_named_safely() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        let song = dir.path().join("My Song.mp3");
        fs::write(&song, b"audio").unwrap();
        let (name, staged) = stage_audio_for_upload(&input, &song.display().to_string()).unwrap();
        assert_eq!(name, "My Song.mp3");
        assert_eq!(safe_file_name("C:/a/b<c>:d?.wav", "x.wav"), "b_c_d_.wav");
        assert_eq!(safe_file_name("folder/", "x.wav"), "x.wav");
        assert!(Path::new(&staged).is_file());
        let first = fs::metadata(&staged).unwrap().modified().unwrap();
        let (_, again) = stage_audio_for_upload(&input, &song.display().to_string()).unwrap();
        assert_eq!(again, staged);
        assert_eq!(fs::metadata(&staged).unwrap().modified().unwrap(), first);
        assert_eq!(stage_audio_for_upload(&input, "").unwrap_err(), "Choose an audio file before running Prompt Creator.");
        assert_eq!(stage_audio_for_upload(&input, "nowhere.wav").unwrap_err(), "Audio file was not found: nowhere.wav");
        fs::write(input.join("only_here.wav"), b"x").unwrap();
        assert_eq!(stage_audio_for_upload(&input, "only_here.wav").unwrap().0, "only_here.wav");
    }

    #[test]
    fn the_beat_graph_is_built_for_the_staged_song() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.wav");
        fs::write(&song, b"audio").unwrap();
        let payload = json!({
            "project_folder": dir.path().join("proj").display().to_string(), "audio_path": song.display().to_string(), "min_duration": 3,
            "full_lyrics": "la la", "use_srt_durations": "false", "whisper_language": "german"
        })
        .as_object()
        .cloned()
        .unwrap();
        let reply = build_whisper_prompt(&env(dir.path()), &dir.path().join("input"), &payload, &stamp()).unwrap();
        let prompt = &reply["prompt"];
        assert_eq!(prompt["954"]["inputs"]["audio"], "song.wav");
        assert_eq!(prompt["28:114"]["inputs"]["audio_file_path"], dir.path().join("input").join("song.wav").display().to_string());
        assert_eq!(prompt["955"]["inputs"]["text"], "la la");
        assert_eq!(prompt["960"]["inputs"]["language"], "german");
        assert_eq!(prompt["28:933"]["inputs"]["switch"], false);
        assert_eq!(prompt["28:80"]["inputs"]["min_duration"], 3.0);
        assert_eq!(prompt["28:80"]["inputs"]["max_duration"], 10.0);
        assert_eq!(prompt["28:80"]["inputs"]["output_filename"], "builder_segments_20260101_000000.srt");
        assert_eq!(reply["source_srt_filename"], "builder_segments_20260101_000000.srt");
        assert!(dir.path().join("proj").join("project_context").is_dir() && dir.path().join("proj").join("prompts").is_dir());
        assert!(reply["expected_srt_path"].as_str().unwrap().ends_with("builder_segments.srt"));
        let mut bad = payload.clone();
        bad.insert("min_duration".into(), json!("long"));
        assert_eq!(build_whisper_prompt(&env(dir.path()), &dir.path().join("input"), &bad, &stamp()).unwrap_err(), "could not convert string to float: 'long'");
    }
}
