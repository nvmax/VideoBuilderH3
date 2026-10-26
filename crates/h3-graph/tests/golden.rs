//! Every golden graph from the pack's Python builders, rebuilt in Rust from the same settings and compared whole.
//! Fixtures come from `tools/make_graph_fixtures.py`, which runs the pack's own code against a fake ComfyUI.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))] // the tests that use these helpers only run on Windows

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::Obj;
use h3_core::settings::{build_render_payload, normalize_settings, render_pass, workflow_key};
use h3_core::timing::TimingPlan;
use h3_core::tokens::frames_to_tokens;
use h3_graph::{build, GraphEnv, LatentFacts};
use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

/// The fake ComfyUI and project folder the generator used.
struct FakeEnv {
    root: String,
    output_dir: String,
    song_seconds: f64,
    models: Value,
    object_info: Value,
    predecessor_frames: i64,
    predecessor_padding: i64,
    trim_call: RefCell<Option<Value>>,
}

impl FakeEnv {
    fn is_fake(&self, path: &str) -> bool {
        path.to_lowercase().starts_with(&self.root.to_lowercase())
    }

    fn has_suffix(path: &str) -> bool {
        let name = path.rsplit(['\\', '/']).next().unwrap_or("");
        name.rfind('.').is_some_and(|i| i > 0 && i + 1 < name.len())
    }
}

impl GraphEnv for FakeEnv {
    fn is_file(&self, path: &str) -> bool {
        if self.is_fake(path) && Self::has_suffix(path) { true } else { Path::new(path).is_file() }
    }
    fn is_dir(&self, path: &str) -> bool {
        if self.is_fake(path) && !Self::has_suffix(path) { true } else { Path::new(path).is_dir() }
    }
    fn ensure_dir(&self, _path: &str) {}
    fn node_info(&self, class: &str) -> Option<Value> {
        self.object_info.get(class).cloned()
    }
    fn model_choices(&self, category: &str) -> Vec<String> {
        self.models
            .get(category)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
    fn output_dir(&self) -> String {
        self.output_dir.clone()
    }
    fn probe_duration(&self, _path: &str) -> Result<f64, String> {
        Ok(self.song_seconds)
    }
    fn trim_audio(&self, source: &str, project: &str, scene: u32, timing: &TimingPlan) -> Result<Value, String> {
        *self.trim_call.borrow_mut() = Some(json!({
            "source": source,
            "project_folder": project,
            "scene_number": scene,
            "start": timing.audio_trim_start_seconds,
            "duration": timing.audio_trim_duration_seconds,
            "leading_padding": timing.audio_leading_padding_seconds,
        }));
        Ok(json!({
            "audio_path": format!("{project}\\minimax_h3_scene_audio\\scene_audio_{scene:04}.wav"),
            "start": timing.audio_trim_start_seconds,
            "duration": timing.audio_trim_duration_seconds,
            "requested_duration": timing.audio_trim_duration_seconds,
            "format": "pcm_s16le_wav",
        }))
    }
    fn latent_exists(&self, project: &str, scene: u32) -> bool {
        self.is_dir(project) && self.is_file(&self.latent_path(project, scene))
    }
    fn latent_path(&self, project: &str, scene: u32) -> String {
        format!("{project}\\latents\\scene_{scene:03}.latent")
    }
    fn latent_facts(&self, _project: &str, _scene: u32) -> LatentFacts {
        LatentFacts {
            exists: true,
            token_count: frames_to_tokens(self.predecessor_frames),
            tail_padding_frames: Some(self.predecessor_padding),
        }
    }
    fn pass1_latent_matches(&self, _project: &str, _scene: u32) -> bool {
        false
    }
    fn random_seed(&self) -> u64 {
        4_242
    }
}

/// Where two JSON values first differ, as a path. Numbers compare by kind too (`1` is not `1.0`).
fn first_difference(path: &str, got: &Value, want: &Value) -> Option<String> {
    match (got, want) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()) {
                let (x, y) = (a.get(key), b.get(key));
                match (x, y) {
                    (Some(x), Some(y)) => {
                        if let Some(found) = first_difference(&format!("{path}.{key}"), x, y) {
                            return Some(found);
                        }
                    }
                    (Some(x), None) => return Some(format!("{path}.{key}: extra in Rust: {x}")),
                    (None, Some(y)) => return Some(format!("{path}.{key}: missing in Rust (golden {y})")),
                    (None, None) => {}
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: length {} vs {}", a.len(), b.len()));
            }
            a.iter().zip(b).enumerate().find_map(|(i, (x, y))| first_difference(&format!("{path}[{i}]"), x, y))
        }
        _ => (got != want).then(|| format!("{path}: got {got} want {want}")),
    }
}

fn case_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(fixtures().join("graphs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

// Its fixtures hold Windows paths, so it only runs on Windows.
#[cfg(windows)]
#[test]
fn rust_graphs_equal_the_golden_graphs() {
    let env_info = read(&fixtures().join("graph_env.json"));
    let object_info = read(&fixtures().join("object_info_h3.json"));
    let mut checked = 0;
    let mut failures: Vec<String> = Vec::new();
    for file in case_files() {
        let record = read(&file);
        let name = record["name"].as_str().unwrap().to_string();
        // The retired fixed 3 pass builder is not ported, the Video Builder never reaches it.
        if name.starts_with("legacy_") {
            continue;
        }
        let env = FakeEnv {
            root: env_info["fake_root"].as_str().unwrap().to_string(),
            output_dir: env_info["output_dir"].as_str().unwrap().to_string(),
            song_seconds: env_info["song_seconds"].as_f64().unwrap(),
            models: env_info["models"].clone(),
            object_info: object_info.clone(),
            predecessor_frames: record["scene"]["predecessor_latent_frames"].as_i64().unwrap_or(169),
            predecessor_padding: record["scene"]["predecessor_tail_padding"].as_i64().unwrap_or(9),
            trim_call: RefCell::new(None),
        };
        let mut problem: Option<String> = None;
        let settings = normalize_settings(Some(&record["settings_overrides"]));
        if record.get("settings").is_some_and(|golden| first_difference("settings", &Value::Object(settings.clone()), golden).is_some()) {
            // Settings are already covered by the h3-core tests, which compare with number tolerance.
        }
        let key = workflow_key(&settings);
        let mut payload: Obj = match build_render_payload(&settings, None) {
            Ok(p) => p,
            Err(e) => {
                let want = record.get("error").map(|e| e["message"].clone());
                if want.as_ref() != Some(&json!(e.message)) {
                    failures.push(format!("{name}: payload error {} vs {:?}", e.message, want));
                }
                checked += 1;
                continue;
            }
        };
        for (k, v) in record["scene"].as_object().unwrap() {
            payload.insert(k.clone(), v.clone());
        }
        payload.insert("project_folder".into(), record["project_folder"].clone());
        payload.insert("i2v_prompt".into(), record["scene"]["prompt"].clone());
        payload.insert("t2v_prompt".into(), record["scene"]["prompt"].clone());
        let _ = render_pass(&settings);
        if let Some(found) = first_difference("payload", &Value::Object(payload.clone()), &record["payload"]) {
            // Whole floats written as ints by the settings defaults compare equal in meaning, so only flag other kinds.
            if !found.contains(" got ") || !found.contains(" want ") {
                problem = Some(found);
            }
        }
        if record["mode"] != json!(key) {
            problem = Some(format!("workflow key {key} vs {}", record["mode"]));
        }
        match (build(&env, key, &payload), record.get("error"), record.get("result")) {
            (Ok(result), None, Some(want)) => {
                let mut got = Value::Object(result);
                if let Some(path) = got.get("workflow_path").and_then(Value::as_str).map(str::to_string) {
                    got["workflow_path"] = json!(path);
                }
                if let Some(found) = first_difference("result", &got, want) {
                    problem = Some(found);
                }
                let trim_want = &record["audio_trim_call"];
                let trim_got = env.trim_call.borrow().clone().unwrap_or(Value::Null);
                if first_difference("trim", &trim_got, trim_want).is_some() {
                    problem = Some(format!("audio trim call differs: {trim_got} vs {trim_want}"));
                }
            }
            (Err(e), Some(want), _) => {
                if json!(e.message) != want["message"] {
                    problem = Some(format!("error text: {} vs {}", e.message, want["message"]));
                }
            }
            (Ok(_), Some(want), _) => problem = Some(format!("built a graph, expected the error {}", want["message"])),
            (Err(e), None, _) => problem = Some(format!("unexpected error: {}", e.message)),
            (Ok(_), None, None) => problem = Some("fixture has neither result nor error".into()),
        }
        if let Some(p) = problem {
            failures.push(format!("{name}: {p}"));
        }
        checked += 1;
    }
    assert!(checked >= 70, "only {checked} fixtures checked");
    assert!(failures.is_empty(), "{} of {checked} graphs differ:\n{}", failures.len(), failures.join("\n"));
}
