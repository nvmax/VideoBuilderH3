//! MiniMax H3 drafts on disk: the folder a draft keeps its saved first pass in, and the two graphs the Builder asks for.
//! Port of `_draft_folder`, `_build_minimax_draft` and `_build_minimax_finalize` in `runner/minimax_drafts.py`.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{text_or_empty, to_float, to_int, Obj};
use h3_graph::drafts;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::payload::path_text;

type Reply = Result<Value, String>;

/// The payload value of a key the pack reads with `payload[key]`. A missing key reads like Python's `KeyError`.
fn required<'a>(payload: &'a Obj, key: &str) -> Result<&'a Value, String> {
    payload.get(key).ok_or_else(|| format!("'{key}'"))
}

fn integer(value: &Value) -> Result<i64, String> {
    to_int(value).ok_or_else(|| format!("invalid literal for int() with base 10: {}", text_or_empty(Some(value))))
}

fn float(value: &Value) -> Result<f64, String> {
    to_float(value).ok_or_else(|| match value {
        Value::String(s) => format!("could not convert string to float: '{s}'"),
        _ => "float() argument must be a string or a real number".to_string(),
    })
}

/// 32 lowercase hex characters, unique for each call.
fn new_draft_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let digest = Sha256::digest(format!("{nanos}-{}-{count}", std::process::id()).as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// `_draft_folder`: the folder of one draft inside the project.
fn draft_folder(project_folder: &str, draft_id: &str) -> Result<PathBuf, String> {
    let project = Path::new(project_folder);
    if !project.is_dir() {
        return Err("The draft project folder does not exist.".into());
    }
    if draft_id.len() != 32 || !draft_id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err("Invalid MiniMax draft id.".into());
    }
    Ok(project.join("minimax_drafts").join(draft_id))
}

fn project_of(payload: &Obj) -> Result<String, String> {
    let raw = text_or_empty(Some(required(payload, "project_folder")?));
    Ok(raw.trim().to_string())
}

/// `POST /vrgdg/workflow_runner/build_minimax_h3_draft_prompt`, after the two pass graph is built: save the full graph in a
/// new draft folder and return the graph that renders pass 1 only.
pub fn build_draft(result: &Obj, payload: &Obj) -> Reply {
    if !drafts::is_two_pass(result) {
        return Err("Draft rendering requires a MiniMax two-pass workflow.".into());
    }
    let project = project_of(payload)?;
    let draft_id = new_draft_id();
    let folder = draft_folder(&project, &draft_id)?;
    let scene = integer(required(payload, "scene_number")?)?;
    let start = float(required(payload, "timeline_start_seconds")?)?;
    let end = float(required(payload, "timeline_end_seconds")?)?;
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut saved = result.clone();
    saved.insert("draft_id".into(), json!(draft_id));
    saved.insert("draft_scene_number".into(), json!(scene));
    saved.insert("draft_start".into(), json!(start));
    saved.insert("draft_end".into(), json!(end));
    let restore = payload.get("draft_restore_audio").filter(|v| h3_core::pyval::truthy(v)).cloned().unwrap_or_else(|| json!({}));
    saved.insert("draft_restore_audio".into(), restore);
    saved.insert("draft_steps".into(), drafts::draft_steps(result).map_err(|e| e.to_string())?);
    fs::write(folder.join("workflow.json"), serde_json::to_string(&Value::Object(saved.clone())).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let checkpoint = folder.join("pass1.pt").display().to_string();
    drafts::draft_graph(&saved, &checkpoint).map(Value::Object).map_err(|e| e.to_string())
}

/// `POST /vrgdg/workflow_runner/build_minimax_h3_finalize_prompt`: the graph that finishes a saved draft. When the draft was
/// made with another diffusion model the reply says so instead.
pub fn build_finalize(payload: &Obj) -> Reply {
    let project = project_of(payload)?;
    let draft_id = text_or_empty(Some(required(payload, "draft_id")?));
    let folder = draft_folder(&project, &draft_id)?;
    let checkpoint = folder.join("pass1.pt");
    if !checkpoint.is_file() {
        return Err("The saved pass-1 latent is missing. Render a new draft first.".into());
    }
    let text = fs::read(folder.join("workflow.json")).map_err(|e| format!("{}: {e}", folder.join("workflow.json").display()))?;
    let saved = match h3_core::pyjson::parse(String::from_utf8_lossy(&text).as_ref()).map_err(|e| e.to_string())? {
        Value::Object(map) => map,
        _ => return Err("The saved draft is not a workflow.".into()),
    };
    let prompt = saved.get("prompt").and_then(Value::as_object).ok_or("The saved draft is not a workflow.")?;
    // The draft replays its own saved graph, so one made with another diffusion model would ignore the current model.
    let current = drafts::base_name(path_text(payload, "diffusion_model_name").trim());
    let used = drafts::draft_diffusion_models(prompt);
    if !current.is_empty() && !used.is_empty() && !used.contains(&current) {
        return Ok(json!({"stale_draft": true, "draft_models": used, "current_model": current}));
    }
    let scene = integer(required(payload, "scene_number")?)?;
    if Some(scene) != saved.get("draft_scene_number").and_then(to_int) {
        return Err("This draft belongs to a different scene.".into());
    }
    let start = float(required(payload, "timeline_start_seconds")?)?;
    let end = float(required(payload, "timeline_end_seconds")?)?;
    let saved_start = saved.get("draft_start").and_then(to_float).unwrap_or(0.0);
    let saved_end = saved.get("draft_end").and_then(to_float).unwrap_or(0.0);
    if (start - saved_start).abs() > 0.001 || (end - saved_end).abs() > 0.001 {
        return Err("The scene timing changed. Render a new draft before finalizing.".into());
    }
    drafts::finalize_graph(&saved, &checkpoint.display().to_string()).map(Value::Object).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn two_pass_result() -> Obj {
        obj(json!({"two_pass": true, "prompt": {
            "119": {"class_type": "VAELoader", "inputs": {}},
            "124": {"class_type": "KSampler", "inputs": {"steps": 4}},
            "125": {"class_type": "Sampler", "inputs": {"latent": ["124", 0]}},
            "190": {"class_type": "Primitive", "inputs": {"value": 3}},
            "142": {"class_type": "VHS_VideoCombine", "inputs": {"images": ["125", 0], "filename_prefix": "scene"}},
            "10": {"class_type": "UNETLoader", "inputs": {"unet_name": "big.gguf"}}
        }}))
    }

    fn request(project: &Path, extra: Value) -> Obj {
        let mut base = obj(json!({"project_folder": project.display().to_string(), "scene_number": 3, "timeline_start_seconds": 10.0, "timeline_end_seconds": 14.5}));
        base.extend(obj(extra));
        base
    }

    #[test]
    fn a_draft_is_saved_and_finished_from_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let draft = build_draft(&two_pass_result(), &request(dir.path(), json!({"draft_restore_audio": {"keep": 1}}))).unwrap();
        let id = draft["draft_id"].as_str().unwrap().to_string();
        assert_eq!(id.len(), 32);
        assert_eq!(draft["draft_steps"], json!({"pass1": 4, "pass2": 3}));
        assert_eq!(draft["draft_restore_audio"], json!({"keep": 1}));
        let folder = dir.path().join("minimax_drafts").join(&id);
        assert!(folder.join("workflow.json").is_file());
        assert_eq!(draft["prompt"]["draft_save"]["inputs"]["checkpoint_path"], folder.join("pass1.pt").display().to_string());

        let finish = request(dir.path(), json!({"draft_id": id}));
        assert_eq!(build_finalize(&finish).unwrap_err(), "The saved pass-1 latent is missing. Render a new draft first.");
        fs::write(folder.join("pass1.pt"), b"latent").unwrap();
        let done = build_finalize(&finish).unwrap();
        assert_eq!(done["prompt"]["125"]["class_type"], "VRGDG_MiniMaxH3LoadDraft");
        assert_eq!(done["prompt"]["142"]["inputs"]["filename_prefix"], "scene_finalized");

        let other_scene = request(dir.path(), json!({"draft_id": id, "scene_number": 4}));
        assert_eq!(build_finalize(&other_scene).unwrap_err(), "This draft belongs to a different scene.");
        let moved = request(dir.path(), json!({"draft_id": id, "timeline_end_seconds": 15.0}));
        assert_eq!(build_finalize(&moved).unwrap_err(), "The scene timing changed. Render a new draft before finalizing.");
        let model = request(dir.path(), json!({"draft_id": id, "diffusion_model_name": "C:\\models\\other.gguf"}));
        assert_eq!(build_finalize(&model).unwrap(), json!({"stale_draft": true, "draft_models": ["big.gguf"], "current_model": "other.gguf"}));
        let same_model = request(dir.path(), json!({"draft_id": id, "diffusion_model_name": "models/big.gguf"}));
        assert!(build_finalize(&same_model).unwrap().get("prompt").is_some());
    }

    #[test]
    fn bad_requests_get_the_packs_messages() {
        let dir = tempfile::tempdir().unwrap();
        let mut single = two_pass_result();
        single.remove("two_pass");
        assert_eq!(build_draft(&single, &request(dir.path(), json!({}))).unwrap_err(), "Draft rendering requires a MiniMax two-pass workflow.");
        assert_eq!(build_draft(&two_pass_result(), &request(&dir.path().join("none"), json!({}))).unwrap_err(), "The draft project folder does not exist.");
        let mut no_scene = request(dir.path(), json!({}));
        no_scene.remove("scene_number");
        assert_eq!(build_draft(&two_pass_result(), &no_scene).unwrap_err(), "'scene_number'");
        assert_eq!(build_finalize(&request(dir.path(), json!({"draft_id": "../../etc"}))).unwrap_err(), "Invalid MiniMax draft id.");
        assert_eq!(build_finalize(&request(dir.path(), json!({"draft_id": "A".repeat(32)}))).unwrap_err(), "Invalid MiniMax draft id.");
    }
}
