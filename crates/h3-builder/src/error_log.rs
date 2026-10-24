//! The project's error report: `logs/error_log.txt`, written by the Builder when something fails. Port of
//! `builder/error_log.py`. Credentials are removed from every line before it is written.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fancy_regex::Regex;
use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_project::atomic::atomic_write_text;
use serde_json::{json, Value};

use crate::payload::path_text;

static LOCK: Mutex<()> = Mutex::new(());

type Reply = Result<Value, String>;

/// Keys that hold a credential: `api.?key|token|password|secret|authorization|credential`, any case.
fn secret_key(key: &str) -> bool {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"(?i)api.?key|token|password|secret|authorization|credential").expect("valid pattern"));
    pattern.is_match(key).unwrap_or(false)
}

/// Credential values in a JSON value, without unrelated project data.
pub fn secret_values(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                match item {
                    Value::String(s) if secret_key(key) && !s.is_empty() => found.push(s.clone()),
                    Value::Object(_) | Value::Array(_) => found.extend(secret_values(item)),
                    _ => {}
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| found.extend(secret_values(i))),
        _ => {}
    }
    found
}

/// Remove known credentials and common credential bearing text.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();
    let mut unique: Vec<&String> = secrets.iter().collect();
    unique.sort_by_key(|s| std::cmp::Reverse(s.len()));
    unique.dedup();
    for secret in unique.into_iter().filter(|s| !s.is_empty()) {
        result = result.replace(secret.as_str(), "[redacted]");
    }
    let steps: [(&str, &str); 3] = [
        (r#"(?i)(Bearer\s+)[^\s,'"<>]+"#, "${1}[redacted]"),
        (r#"(?i)((?:api[_-]?key|access[_-]?token|token|password|secret|authorization)["']?\s*[:=]\s*["']?)[^\s,"'&<>]+"#, "${1}[redacted]"),
        (r"(https?://)[^/\s:@]+:[^/\s@]+@", "${1}[redacted]@"),
    ];
    for (pattern, replacement) in steps {
        if let Ok(regex) = Regex::new(pattern) {
            result = regex.replace_all(&result, replacement).into_owned();
        }
    }
    result
}

/// The log path inside a saved project, and its session.
fn project_log(project_folder: &str) -> Result<(PathBuf, Obj, PathBuf), String> {
    if project_folder.trim().is_empty() {
        return Err("Save or open a project before using its error log.".into());
    }
    let folder = fs::canonicalize(project_folder).map_err(|e| e.to_string())?;
    let session_text = fs::read_to_string(folder.join("vrgdg_builder_session.json")).map_err(|e| e.to_string())?;
    let session = match h3_core::pyjson::parse(&session_text).map_err(|e| e.to_string())? {
        Value::Object(map) => map,
        _ => return Err("Builder session must be an object.".into()),
    };
    Ok((folder.join("logs").join("error_log.txt"), session, folder))
}

fn header(folder: &Path, session: &Obj) -> String {
    let title = match session.get("project_name") {
        Some(v) if truthy(v) => text_or_empty(Some(v)),
        _ => folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
    };
    format!("{}\nVIDEO BUILDER - PROJECT ERROR LOG\nProject: {title}\n{}\n", "=".repeat(50), "=".repeat(50))
}

/// `POST /vrgdg/music_builder/error_log` (and `/clear` with `clear`): create and read the report, or reset it to its header.
pub fn project_error_log(payload: &Obj, clear: bool, read: bool) -> Reply {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (path, session, folder) = project_log(&path_text(payload, "project_folder"))?;
    if clear || !path.exists() {
        atomic_write_text(&path, &redact(&header(&folder, &session), &secret_values(&Value::Object(session.clone())))).map_err(|e| e.to_string())?;
    }
    if !read {
        return Ok(json!({"path": path.display().to_string()}));
    }
    let body = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(json!({"path": path.display().to_string(), "text": body}))
}

/// `POST /vrgdg/music_builder/error_log/append`: add one record. `extra_secrets` are credentials the caller knows of, for
/// example the ones saved with the model defaults.
pub fn append_project_error(payload: &Obj, extra_secrets: &[String]) -> Reply {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let entry = payload.get("entry").and_then(Value::as_object).cloned().unwrap_or_default();
    if !entry.get("message").is_some_and(truthy) {
        return Err("An error-log entry must contain a message.".into());
    }
    let (path, session, folder) = project_log(&path_text(payload, "project_folder"))?;
    let mut credentials = secret_values(&Value::Object(session.clone()));
    credentials.extend(extra_secrets.iter().cloned());
    let environment: Obj = std::env::vars().map(|(k, v)| (k, Value::String(v))).collect();
    credentials.extend(secret_values(&Value::Object(environment)));
    let severity = match entry.get("severity") {
        Some(v) if truthy(v) => text_or_empty(Some(v)).to_uppercase(),
        _ => "ERROR".to_string(),
    };
    if !["ERROR", "WARNING", "CRITICAL"].contains(&severity.as_str()) {
        return Err("Unsupported error-log severity.".into());
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%:z");
    let mut lines = vec![String::new(), format!("[{stamp}] [{severity}]")];
    for (key, label) in [("scene", "Scene"), ("stage", "Stage"), ("mode", "Mode"), ("source", "Source"), ("component", "Component"), ("message", "Message")] {
        if let Some(value) = entry.get(key).filter(|v| !v.is_null()) {
            let shown = text_or_empty(Some(value));
            if !shown.is_empty() {
                lines.push(format!("{label}: {shown}"));
            }
        }
    }
    if entry.get("details").is_some_and(truthy) {
        lines.extend([String::new(), "Error Details:".into(), text_or_empty(entry.get("details"))]);
    }
    if entry.get("stack").is_some_and(truthy) {
        lines.extend([String::new(), "Stack Trace:".into(), text_or_empty(entry.get("stack"))]);
    }
    lines.push("-".repeat(50));
    let record = redact(&(lines.join("\n") + "\n"), &credentials);
    if !path.exists() {
        atomic_write_text(&path, &redact(&header(&folder, &session), &credentials)).map_err(|e| e.to_string())?;
    }
    let mut handle = fs::OpenOptions::new().append(true).open(&path).map_err(|e| e.to_string())?;
    handle.write_all(record.as_bytes()).map_err(|e| e.to_string())?;
    let message = redact(&text_or_empty(entry.get("message")), &credentials);
    eprintln!("[VRGDG-RS Builder] {message}");
    Ok(json!({"path": path.display().to_string(), "message": message}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn credentials_are_removed() {
        let text = "Authorization: Bearer abc123 and api_key=sk-secret-1 at https://user:pw@host/x token: \"zzz\"";
        let out = redact(text, &["sk-secret-1".to_string()]);
        assert!(!out.contains("abc123") && !out.contains("sk-secret-1") && !out.contains("user:pw") && !out.contains("zzz"));
        assert!(out.contains("https://[redacted]@host/x"));
    }

    #[test]
    fn secret_keys_are_found_in_nested_values() {
        let found = secret_values(&json!({"a": {"llm_api_key": "k1", "name": "n"}, "list": [{"password": "p"}], "apikey": "k2", "api": "no"}));
        assert_eq!(found, ["k1", "p", "k2"]);
    }

    #[test]
    fn log_is_created_appended_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("vrgdg_builder_session.json"), r#"{"project_name": "Song", "own_server_api_key": "topsecret"}"#).unwrap();
        let folder = dir.path().display().to_string();
        let created = project_error_log(&obj(json!({"project_folder": folder})), false, true).unwrap();
        assert!(created["text"].as_str().unwrap().contains("Project: Song"));
        let appended = append_project_error(&obj(json!({"project_folder": folder, "entry": {"message": "Render failed with topsecret", "scene": 3, "severity": "warning"}})), &[]).unwrap();
        assert_eq!(appended["message"], "Render failed with [redacted]");
        let text = project_error_log(&obj(json!({"project_folder": folder})), false, true).unwrap()["text"].as_str().unwrap().to_string();
        assert!(text.contains("[WARNING]") && text.contains("Scene: 3") && !text.contains("topsecret"));
        assert!(append_project_error(&obj(json!({"project_folder": folder, "entry": {}})), &[]).is_err());
        let cleared = project_error_log(&obj(json!({"project_folder": folder})), true, true).unwrap();
        assert!(!cleared["text"].as_str().unwrap().contains("Scene: 3"));
    }
}
