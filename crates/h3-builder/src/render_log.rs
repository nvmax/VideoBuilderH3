//! Render logs: the timing report the Builder saves for Render All, as JSON and as text. Port of
//! `_save_builder_render_log` and `_render_log_text` in `builder/project.py`.

use std::fs;
use std::path::Path;

use h3_core::pyval::{py_str, round_half_even, text_or_empty, to_float, to_int, truthy, Obj};
use h3_project::atomic::{atomic_write_json, atomic_write_text};
use h3_project::layout::{session_path, Stamp};
use serde_json::{json, Value};

use crate::payload::{object, path_text};

type Reply = Result<Value, String>;

/// `_render_log_duration_text`: `1h 02m 03s`, `2m 03s` or `3s`.
pub fn duration_text(milliseconds: Option<&Value>) -> String {
    let ms = milliseconds.filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0);
    let total = round_half_even(ms / 1000.0).max(0);
    let (hours, rest) = (total / 3600, total % 3600);
    let (minutes, seconds) = (rest / 60, rest % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// `str(map.get(key, ""))`.
fn shown(map: &Obj, key: &str) -> String {
    map.get(key).map_or_else(String::new, |v| py_str(Some(v)))
}

fn count(map: &Obj, key: &str, absent: i64) -> i64 {
    match map.get(key) {
        None => absent,
        Some(v) => Some(v).filter(|v| truthy(v)).and_then(to_int).unwrap_or(0),
    }
}

/// The text report of a render log.
pub fn render_text(log: &Obj) -> String {
    let summary = object(log.get("summary"));
    let scenes: Vec<Value> = log.get("scenes").and_then(Value::as_array).cloned().unwrap_or_default();
    let first_text = |keys: &[&str], fallback: &str| keys.iter().map(|k| text_or_empty(log.get(*k))).find(|t| !t.is_empty()).unwrap_or_else(|| fallback.into());
    let total_ms = summary.get("total_ms").or_else(|| Some(log.get("total_ms").unwrap_or(&Value::Null)));
    let status = Some(text_or_empty(log.get("status"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "unknown".into()).to_uppercase();
    let mut lines: Vec<String> = vec![
        "VRGDG Video Builder Render Log".into(),
        "=".repeat(32),
        format!("Session: {}", shown(log, "id")),
        format!("Status: {status}"),
        format!("Project: {}", shown(log, "project_folder")),
        format!("Mode: {}", first_text(&["mode_label", "scene_scope"], "Render All")),
        format!("Started: {}", shown(log, "started_at")),
        format!("Finished: {}", shown(log, "ended_at")),
        String::new(),
        "Summary".into(),
        "-".repeat(32),
        format!("Total wall time: {}", duration_text(total_ms)),
        format!("Active scene rendering: {}", duration_text(summary.get("render_ms"))),
        format!("Between-render time: {}", duration_text(summary.get("between_render_ms"))),
        format!("Setup time: {}", duration_text(summary.get("setup_ms"))),
        format!("Final stitching: {}", duration_text(summary.get("stitch_ms"))),
        format!("Other overhead: {}", duration_text(summary.get("overhead_ms"))),
        format!("Scenes completed: {}/{}", count(&summary, "completed_scenes", 0), count(&summary, "target_scenes", scenes.len() as i64)),
        format!("Existing scenes skipped: {}", count(&summary, "skipped_existing_scenes", 0)),
        format!("Average render per completed scene: {}", duration_text(summary.get("average_render_ms"))),
    ];
    if log.get("final_video_path").is_some_and(truthy) {
        lines.push(format!("Final video: {}", shown(log, "final_video_path")));
    }
    if log.get("error").is_some_and(truthy) {
        lines.extend([String::new(), format!("Error: {}", shown(log, "error"))]);
    }
    lines.extend([String::new(), "Scene Details".into(), "-".repeat(32)]);
    if scenes.is_empty() {
        lines.push("No scene render timing has been recorded yet.".into());
    }
    for scene in scenes.iter().filter_map(Value::as_object) {
        let label = Some(text_or_empty(scene.get("label"))).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("Scene {}", scene.get("scene_number").map_or_else(|| "?".to_string(), |v| py_str(Some(v)))));
        let status = Some(text_or_empty(scene.get("status"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "pending".into()).to_uppercase();
        lines.extend([
            format!("{label} [{status}]"),
            format!("  Total scene step: {}", duration_text(scene.get("total_ms"))),
            format!("  Preparation: {}", duration_text(scene.get("preparation_ms"))),
            format!("  Video render: {}", duration_text(scene.get("render_ms"))),
            format!("  Post-processing/cleanup: {}", duration_text(scene.get("post_ms"))),
            format!("  Time since previous render: {}", duration_text(scene.get("gap_before_render_ms"))),
        ]);
        if scene.get("video_path").is_some_and(truthy) {
            lines.push(format!("  Video: {}", shown(scene, "video_path")));
        }
        if scene.get("error").is_some_and(truthy) {
            lines.push(format!("  Error: {}", shown(scene, "error")));
        }
    }
    format!("{}\n", lines.join("\n").trim_end())
}

/// `re.sub(r"[^A-Za-z0-9._-]+", "_", text).strip("._")`.
fn safe_id(text: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            pending = false;
        } else if !pending {
            out.push('_');
            pending = true;
        }
    }
    out.trim_matches(['.', '_']).to_string()
}

/// `POST /vrgdg/music_builder/save_render_log`: write the log's JSON and text report into the project's `render_logs`
/// folder and keep the last 20 logs in the session.
pub fn save_render_log(payload: &Obj, now: &Stamp) -> Reply {
    let folder = path_text(payload, "project_folder");
    if folder.is_empty() {
        return Err("Project folder is required before saving a render log.".into());
    }
    let project = h3_core::abspath::absolute(Path::new(&folder)).map_err(|e| e.to_string())?;
    fs::create_dir_all(&project).map_err(|e| e.to_string())?;
    let mut log = object(payload.get("log"));
    if log.is_empty() {
        return Err("Render log data is empty.".into());
    }
    let id = safe_id(text_or_empty(log.get("id")).trim());
    let id = if id.is_empty() { format!("render_{}", now.compact) } else { id };
    log.insert("id".into(), json!(id));
    log.insert("project_folder".into(), json!(project.display().to_string()));
    let logs_folder = project.join("render_logs");
    fs::create_dir_all(&logs_folder).map_err(|e| e.to_string())?;
    let json_path = logs_folder.join(format!("{id}.json"));
    let text_path = logs_folder.join(format!("{id}.txt"));
    log.insert("report_json_path".into(), json!(json_path.display().to_string()));
    log.insert("report_text_path".into(), json!(text_path.display().to_string()));
    atomic_write_json(&json_path, &Value::Object(log.clone())).map_err(|e| e.to_string())?;
    atomic_write_text(&text_path, &render_text(&log)).map_err(|e| e.to_string())?;

    let file = session_path(&project);
    // A session that cannot be read is left as it is: the pack would replace it with an empty one.
    if let Some(Value::Object(mut session)) = fs::read(&file).ok().and_then(|b| h3_core::pyjson::parse(String::from_utf8_lossy(&b).trim_start_matches('\u{feff}')).ok()) {
        let mut logs: Vec<Value> = session.get("render_logs").and_then(Value::as_array).map(|l| l.iter().filter(|i| i.as_object().is_some_and(|o| o.get("id") != Some(&json!(id)))).cloned().collect()).unwrap_or_default();
        logs.push(Value::Object(log.clone()));
        let keep_from = logs.len().saturating_sub(20);
        session.insert("render_logs".into(), Value::Array(logs.split_off(keep_from)));
        session.insert("active_render_log_id".into(), json!(if log.get("status") == Some(&json!("running")) { id.clone() } else { String::new() }));
        session.insert("updated".into(), json!(now.epoch));
        atomic_write_json(&file, &Value::Object(session)).map_err(|e| e.to_string())?;
    }
    Ok(json!({"log": Value::Object(log), "report_json_path": json_path.display().to_string(), "report_text_path": text_path.display().to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp { compact: "20260101_000000".into(), iso: "2026-01-01T00:00:00".into(), epoch: 1_767_225_600.0 }
    }

    #[test]
    fn durations_read_like_the_pack() {
        for (ms, expected) in [(json!(0), "0s"), (json!(59_400), "59s"), (json!(61_000), "1m 01s"), (json!(3_723_000), "1h 02m 03s"), (json!("junk"), "0s"), (json!(-5), "0s"), (json!(2500), "2s")] {
            assert_eq!(duration_text(Some(&ms)), expected, "{ms}");
        }
        assert_eq!(duration_text(None), "0s");
    }

    #[test]
    fn the_report_lists_scenes_and_errors() {
        let log = json!({
            "id": "r1", "status": "failed", "project_folder": "P", "mode_label": "Render All", "started_at": "a", "ended_at": "b",
            "summary": {"total_ms": 125_000, "render_ms": 90_000, "completed_scenes": 1, "target_scenes": 2},
            "error": "boom", "final_video_path": "F.mp4",
            "scenes": [{"label": "Scene 1", "status": "done", "render_ms": 3000, "video_path": "v.mp4"}, {"scene_number": 2, "error": "bad"}]
        });
        let text = render_text(log.as_object().unwrap());
        assert!(text.starts_with("VRGDG Video Builder Render Log\n================================\nSession: r1\nStatus: FAILED\n"));
        assert!(text.contains("Total wall time: 2m 05s\nActive scene rendering: 1m 30s\n"));
        assert!(text.contains("Scenes completed: 1/2\n"));
        assert!(text.contains("Final video: F.mp4\n"));
        assert!(text.contains("\nError: boom\n"));
        assert!(text.contains("Scene 1 [DONE]\n  Total scene step: 0s\n  Preparation: 0s\n  Video render: 3s\n"));
        assert!(text.contains("  Video: v.mp4\n"));
        assert!(text.contains("Scene 2 [PENDING]\n") && text.contains("  Error: bad\n"));
        assert!(text.ends_with("\n") && !text.ends_with("\n\n"));
        assert!(render_text(&Obj::new()).contains("No scene render timing has been recorded yet."));
    }

    #[test]
    fn saving_keeps_twenty_logs_and_the_running_one_active() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("p");
        fs::create_dir_all(&project).unwrap();
        fs::write(session_path(&project), r#"{"keep": 1}"#).unwrap();
        let body = |id: &str, status: &str| json!({"project_folder": project.display().to_string(), "log": {"id": id, "status": status}}).as_object().cloned().unwrap();
        for n in 0..22 {
            save_render_log(&body(&format!("run {n}"), "done"), &stamp()).unwrap();
        }
        let reply = save_render_log(&body("live", "running"), &stamp()).unwrap();
        assert_eq!(reply["log"]["id"], "live");
        assert!(project.join("render_logs").join("live.json").is_file() && project.join("render_logs").join("live.txt").is_file());
        assert!(project.join("render_logs").join("run_3.json").is_file(), "ids are made safe");
        let session: Value = serde_json::from_str(&fs::read_to_string(session_path(&project)).unwrap()).unwrap();
        assert_eq!(session["keep"], 1);
        assert_eq!(session["render_logs"].as_array().unwrap().len(), 20);
        assert_eq!(session["active_render_log_id"], "live");
        assert_eq!(save_render_log(&Obj::new(), &stamp()).unwrap_err(), "Project folder is required before saving a render log.");
        let empty = json!({"project_folder": project.display().to_string()}).as_object().cloned().unwrap();
        assert_eq!(save_render_log(&empty, &stamp()).unwrap_err(), "Render log data is empty.");
        let unnamed = json!({"project_folder": project.display().to_string(), "log": {"status": "done"}}).as_object().cloned().unwrap();
        assert_eq!(save_render_log(&unnamed, &stamp()).unwrap()["log"]["id"], "render_20260101_000000");
    }
}
