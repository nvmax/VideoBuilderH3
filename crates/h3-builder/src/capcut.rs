//! Beat markers from the user's latest CapCut project. Port of `_extract_capcut_project_beats` and
//! `_find_latest_capcut_beats` in `builder/audio.py`.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{round_to, text_or_empty, to_float, truthy, Obj};
use serde_json::{json, Value};

type Reply = Result<Value, String>;

fn load_json(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    h3_core::pyjson::parse(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

fn list(value: Option<&Value>) -> Vec<&Value> {
    value.and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}

fn dicts(value: Option<&Value>) -> Vec<&Obj> {
    list(value).into_iter().filter_map(Value::as_object).collect()
}

fn id_of(item: &Obj) -> String {
    text_or_empty(item.get("id"))
}

fn number(value: Option<&Value>) -> f64 {
    value.filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

/// `os.path.normpath` for a path that is not made absolute. An empty path is ".".
fn normpath(text: &str) -> String {
    let path: PathBuf = Path::new(text).components().collect();
    if path.as_os_str().is_empty() { ".".into() } else { path.display().to_string() }
}

fn sorted_unique(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values.dedup();
    values
}

/// The beats a CapCut draft holds, or `None` when it has fewer than two.
pub fn extract_project_beats(draft: &Value, draft_path: &str) -> Option<Value> {
    let draft = draft.as_object()?;
    let materials = draft.get("materials").and_then(Value::as_object).cloned().unwrap_or_default();
    let audio_materials: Vec<&Obj> = dicts(materials.get("audios")).into_iter().filter(|m| !id_of(m).is_empty()).collect();
    let mut audio_segments: Vec<&Obj> = Vec::new();
    for track in dicts(draft.get("tracks")) {
        if text_or_empty(track.get("type")).to_lowercase() == "audio" {
            audio_segments.extend(dicts(track.get("segments")));
        }
    }
    let empty = Obj::new();
    let audio_segment = audio_segments.first().copied().unwrap_or(&empty);
    let audio_material = audio_materials.iter().find(|m| id_of(m) == text_or_empty(audio_segment.get("material_id"))).copied().unwrap_or(&empty);
    let referenced: Vec<String> = list(audio_segment.get("extra_material_refs")).iter().map(|v| text_or_empty(Some(v))).filter(|v| !v.is_empty()).collect();

    let time_marks = dicts(materials.get("time_marks"));
    let linked: Vec<&Obj> = time_marks.iter().filter(|m| referenced.contains(&id_of(m))).copied().collect();
    let mut marker_times = Vec::new();
    for collection in if linked.is_empty() { &time_marks } else { &linked } {
        for marker in dicts(collection.get("mark_items")) {
            let range = marker.get("time_range").and_then(Value::as_object);
            let start = range.and_then(|r| r.get("start"));
            let parsed = match start {
                None => Some(0.0),
                Some(v) if !truthy(v) => Some(0.0),
                Some(v) => to_float(v),
            };
            if let Some(micro) = parsed {
                let time = micro / 1_000_000.0;
                if time >= 0.0 {
                    marker_times.push(round_to(time, 6));
                }
            }
        }
    }
    let marker_times = sorted_unique(marker_times);

    let beat_materials = dicts(materials.get("beats"));
    let linked_beats: Vec<&Obj> = beat_materials.iter().filter(|m| referenced.contains(&id_of(m))).copied().collect();
    let beat_material = linked_beats.first().or_else(|| beat_materials.first()).copied().unwrap_or(&empty);
    let ai_beats = beat_material.get("ai_beats").and_then(Value::as_object).cloned().unwrap_or_default();
    let cache_path = normpath(text_or_empty(ai_beats.get("beats_path")).trim());
    let mut cache_times = Vec::new();
    let mut beat_values: Vec<Value> = Vec::new();
    if cache_path != "." && Path::new(&cache_path).is_file() {
        if let Ok(Value::Object(cache)) = load_json(Path::new(&cache_path)) {
            let mut times = Vec::new();
            for value in list(cache.get("time")) {
                if let Some(ms) = to_float(value) {
                    if ms / 1000.0 >= 0.0 {
                        times.push(round_to(ms / 1000.0, 6));
                    }
                }
            }
            cache_times = times;
            beat_values = list(cache.get("value")).into_iter().cloned().collect();
        }
    }

    // CapCut's visible project markers are frame-aligned. Prefer them when they correspond one-for-one with the AI cache,
    // otherwise use the raw AI times.
    let (beats, source) = if !marker_times.is_empty() && (cache_times.is_empty() || marker_times.len().abs_diff(cache_times.len()) <= 1) {
        (marker_times, "timeline_markers")
    } else {
        (sorted_unique(cache_times.clone()), "ai_beat_cache")
    };
    if beats.len() < 2 {
        return None;
    }
    let project_name = Some(text_or_empty(draft.get("name")).trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
        Path::new(draft_path).parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    });
    let absolute = if draft_path.is_empty() { String::new() } else { h3_core::abspath::absolute(draft_path).map(|p| p.components().collect::<PathBuf>().display().to_string()).unwrap_or_else(|_| draft_path.to_string()) };
    Some(json!({
        "project_name": project_name,
        "draft_path": absolute,
        "project_fps": number(draft.get("fps")),
        "project_duration": number(draft.get("duration")) / 1_000_000.0,
        "audio_name": text_or_empty(audio_material.get("name")).trim(),
        "audio_path": text_or_empty(audio_material.get("path")).trim(),
        "beat_cache_path": cache_path,
        "beat_source": source,
        "beats": beats,
        "raw_ai_beats": cache_times,
        "beat_values": beat_values,
    }))
}

/// `POST /vrgdg/music_builder/import_capcut_beats`: the beats of the newest CapCut project whose length matches the loaded
/// audio (within 0.75 seconds). With no audio length the newest project that has beats is used.
pub fn find_latest_capcut_beats(audio_duration: &Value) -> Reply {
    let local = std::env::var("LOCALAPPDATA").ok().filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| std::env::var("USERPROFILE").ok().map(|h| PathBuf::from(h).join("AppData").join("Local"))).unwrap_or_default();
    find_in(&local.join("CapCut").join("User Data").join("Projects").join("com.lveditor.draft").join("root_meta_info.json"), audio_duration)
}

fn find_in(index_path: &Path, audio_duration: &Value) -> Reply {
    if !index_path.is_file() {
        return Err(format!("CapCut project index was not found: {}", index_path.display()));
    }
    let index = load_json(index_path)?;
    let mut entries: Vec<&Obj> = dicts(index.get("all_draft_store")).into_iter().filter(|e| !e.get("tm_draft_removed").is_some_and(truthy)).collect();
    entries.sort_by(|a, b| number(b.get("tm_draft_modified")).partial_cmp(&number(a.get("tm_draft_modified"))).unwrap_or(std::cmp::Ordering::Equal));
    let requested = number(Some(audio_duration)).max(0.0);
    let mut latest: Option<Value> = None;
    for entry in entries.into_iter().take(150) {
        let draft_path = normpath(text_or_empty(entry.get("draft_json_file")).trim());
        if draft_path == "." || !Path::new(&draft_path).is_file() {
            continue;
        }
        let Some(mut result) = load_json(Path::new(&draft_path)).ok().and_then(|d| extract_project_beats(&d, &draft_path)) else { continue };
        let name = Some(text_or_empty(entry.get("draft_name")).trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| text_or_empty(result.get("project_name")).trim().to_string());
        if let Some(map) = result.as_object_mut() {
            map.insert("project_name".into(), json!(name));
            map.insert("project_modified".into(), json!(number(entry.get("tm_draft_modified"))));
        }
        if latest.is_none() {
            latest = Some(result.clone());
        }
        if requested <= 0.0 || (number(result.get("project_duration")) - requested).abs() <= 0.75 {
            return Ok(result);
        }
    }
    match latest {
        Some(found) if requested <= 0.0 => Ok(found),
        Some(_) => Err("CapCut projects with beat data were found, but none matched the loaded audio duration within 0.75 seconds.".into()),
        None => Err("No CapCut project containing beat data was found.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(markers: &[i64], cache: Option<&Path>) -> Value {
        json!({
            "name": "My Edit", "fps": 30, "duration": 12_000_000,
            "tracks": [{"type": "audio", "segments": [{"material_id": "a1", "extra_material_refs": ["t1", "b1"]}]}],
            "materials": {
                "audios": [{"id": "a1", "name": "song.mp3", "path": "C:/music/song.mp3"}],
                "time_marks": [{"id": "t1", "mark_items": markers.iter().map(|m| json!({"time_range": {"start": m}})).collect::<Vec<_>>()}],
                "beats": [{"id": "b1", "ai_beats": {"beats_path": cache.map(|p| p.display().to_string()).unwrap_or_default()}}]
            }
        })
    }

    #[test]
    fn markers_become_beats_in_seconds() {
        let result = extract_project_beats(&draft(&[2_000_000, 500_000, 500_000, 1_250_000], None), "C:\\drafts\\one\\draft_content.json").unwrap();
        assert_eq!(result["beats"], json!([0.5, 1.25, 2.0]));
        assert_eq!(result["beat_source"], "timeline_markers");
        assert_eq!(result["project_name"], "My Edit");
        assert_eq!(result["project_duration"], 12.0);
        assert_eq!(result["audio_name"], "song.mp3");
        assert_eq!(result["beat_cache_path"], ".");
        assert!(extract_project_beats(&draft(&[500_000], None), "x").is_none(), "one beat is not enough");
        assert!(extract_project_beats(&json!([]), "x").is_none());
    }

    #[test]
    fn the_ai_cache_is_used_when_the_markers_do_not_match_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("beats.json");
        fs::write(&cache, r#"{"time": [100, 600, 1100, 1600, 2100], "value": [1, 0, 1, 0, 1]}"#).unwrap();
        let result = extract_project_beats(&draft(&[100_000, 600_000], Some(&cache)), "x").unwrap();
        assert_eq!(result["beat_source"], "ai_beat_cache");
        assert_eq!(result["beats"], json!([0.1, 0.6, 1.1, 1.6, 2.1]));
        assert_eq!(result["beat_values"], json!([1, 0, 1, 0, 1]));
        let close = extract_project_beats(&draft(&[100_000, 600_000, 1_100_000, 1_600_000], Some(&cache)), "x").unwrap();
        assert_eq!(close["beat_source"], "timeline_markers");
        assert_eq!(close["raw_ai_beats"], json!([0.1, 0.6, 1.1, 1.6, 2.1]));
    }

    #[test]
    fn the_newest_project_matching_the_audio_length_wins() {
        let dir = tempfile::tempdir().unwrap();
        let write_draft = |name: &str, seconds: i64| {
            let path = dir.path().join(name);
            let mut value = draft(&[500_000, 1_000_000], None);
            value["duration"] = json!(seconds * 1_000_000);
            fs::write(&path, value.to_string()).unwrap();
            path.display().to_string()
        };
        let index = json!({"all_draft_store": [
            {"draft_name": "old short", "draft_json_file": write_draft("a.json", 12), "tm_draft_modified": 100},
            {"draft_name": "new long", "draft_json_file": write_draft("b.json", 200), "tm_draft_modified": 300},
            {"draft_name": "removed", "draft_json_file": write_draft("c.json", 12), "tm_draft_modified": 400, "tm_draft_removed": true},
            {"draft_name": "missing", "draft_json_file": "nowhere.json", "tm_draft_modified": 500}
        ]});
        let index_path = dir.path().join("root_meta_info.json");
        fs::write(&index_path, index.to_string()).unwrap();
        assert_eq!(find_in(&index_path, &json!(12)).unwrap()["project_name"], "old short");
        assert_eq!(find_in(&index_path, &json!(0)).unwrap()["project_name"], "new long");
        assert_eq!(find_in(&index_path, &json!(12)).unwrap()["project_modified"], 100.0);
        assert!(find_in(&index_path, &json!(90)).unwrap_err().contains("none matched the loaded audio duration"));
        assert!(find_in(&dir.path().join("none.json"), &json!(0)).unwrap_err().starts_with("CapCut project index was not found: "));
    }
}
