//! Edits of the session that need no file moves: scene fields, prompts, lyrics, references and projects. Port of the
//! matching functions in `agent_api/mutations.py`. Storyboard cards are not written, because this app has none.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::lyric_scenes::is_instrumental_lyric_text;
use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_core::scene_card::{apply_to_segment, card_only_names, field_names, parse_scene_card_patch};
use h3_core::timeline::{normalize_segments, sort_segments};
use serde_json::{json, Value};

use crate::atomic::atomic_write_text;
use crate::layout::{context_folder, Stamp};
use crate::scene_ops::{open, OpError, OpResult};

const TEXT_FIELDS: [&str; 17] = [
    "t2i_prompt", "i2v_prompt", "enhance_prompt", "minimax_h3_prompt", "minimax_h3_i2v_frame_mode", "first_last_frame_end_image_path", "minimax_h3_continuation_direction",
    "flux_prompt", "nb_prompt", "flow_gpt_prompt", "ernie_t2i_prompt", "video_path", "video_output", "video_status", "approved_image_path", "audio_path", "custom_audio_path",
];
const NUMBER_FIELDS: [&str; 2] = ["start", "end"];
const OPTIONAL_NUMBER_FIELDS: [&str; 1] = ["minimax_h3_continuation_start_seconds"];
const PROMPT_KEYS: [&str; 4] = ["t2i_prompt", "i2v_prompt", "minimax_h3_prompt", "minimax_h3_pass2_prompt"];

/// The reference scene maps the Builder keeps inside `flux_reference_builder`, by API name.
pub const REFERENCE_MAP_KEYS: [(&str, &str); 4] = [("subjects", "subject_scene_map"), ("locations", "scene_map"), ("ingredients", "ingredients_scene_map"), ("extras", "extra_scene_map")];

fn supported_fields() -> Vec<String> {
    let mut all: Vec<String> = TEXT_FIELDS.iter().chain(NUMBER_FIELDS.iter()).chain(OPTIONAL_NUMBER_FIELDS.iter()).map(|s| s.to_string()).collect();
    all.extend(field_names());
    all.sort();
    all.dedup();
    all
}

/// The live scene map for `api_name`, folding in an older top level map.
pub fn reference_map<'a>(session: &'a mut Obj, api_name: &str) -> &'a mut Obj {
    let key = REFERENCE_MAP_KEYS.iter().find(|(n, _)| *n == api_name).map(|(_, k)| *k).unwrap_or("scene_map");
    let legacy = if matches!(api_name, "subjects" | "locations") { session.get(key).and_then(Value::as_object).cloned().filter(|m| !m.is_empty()) } else { None };
    if legacy.is_some() {
        session.shift_remove(key);
    }
    let refs = session.entry("flux_reference_builder").or_insert_with(|| json!({}));
    if !refs.is_object() {
        *refs = json!({});
    }
    let current = refs.as_object_mut().expect("object").entry(key).or_insert_with(|| json!({}));
    if !current.is_object() {
        *current = json!({});
    }
    let map = current.as_object_mut().expect("object");
    if let Some(legacy) = legacy {
        for (scene, value) in legacy {
            map.entry(scene).or_insert(value);
        }
    }
    map
}

fn known_ids(session: &Obj, list_key: &str) -> Vec<String> {
    session.get("flux_reference_builder").and_then(|r| r.get(list_key)).and_then(Value::as_array).map(|a| a.iter().filter_map(|i| i.get("id")).map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default()
}

/// Map the scene's characters and location like the Reference Builder. Returns the maps that changed.
fn apply_scene_references(session: &mut Obj, scene_id: &str, values: &Obj) -> Result<Vec<&'static str>, OpError> {
    session.entry("flux_reference_builder").or_insert_with(|| json!({}));
    let mut changed = Vec::new();
    for (name, api_name, list_key, switch) in [("subject_ids", "subjects", "subjects", "use_subject_reference"), ("location_id", "locations", "locations", "use_location_references")] {
        let Some(value) = values.get(name) else { continue };
        let known = known_ids(session, list_key);
        let wanted: Vec<String> = if name == "subject_ids" {
            value.as_array().map(|a| a.iter().map(|v| text_or_empty(Some(v))).collect()).unwrap_or_default()
        } else {
            Some(text_or_empty(Some(value))).filter(|s| !s.is_empty()).into_iter().collect()
        };
        let unknown: Vec<&String> = wanted.iter().filter(|w| !known.contains(w)).collect();
        if !unknown.is_empty() {
            let mut sorted = known.clone();
            sorted.sort();
            return Err(OpError::Validation(format!(
                "Unknown {} id{}: {}. Known: {}.",
                &list_key[..list_key.len() - 1],
                if unknown.len() > 1 { "s" } else { "" },
                unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                if sorted.is_empty() { "none".to_string() } else { sorted.join(", ") }
            )));
        }
        let new_value = value.clone();
        let scene_map = reference_map(session, api_name);
        if scene_map.get(scene_id) != Some(&new_value) {
            scene_map.insert(scene_id.to_string(), new_value);
            changed.push(api_name);
        }
        let any_mapped = reference_map(session, api_name).values().any(truthy);
        let has_cards = session.get("flux_reference_builder").and_then(|r| r.get(list_key)).is_some_and(truthy);
        session["flux_reference_builder"][switch] = json!(has_cards || any_mapped);
    }
    Ok(changed)
}

fn validate_scene_h3_patch(legacy: &Obj) -> Result<(), OpError> {
    if let Some(flag) = legacy.get("use_scene_minimax_h3_settings") {
        if !flag.is_boolean() {
            return Err(OpError::Validation("use_scene_minimax_h3_settings must be true or false.".into()));
        }
    }
    match legacy.get("minimax_h3_settings") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(settings)) => {
            let problems = h3_core::settings::validate_patch(settings);
            if problems.is_empty() {
                return Ok(());
            }
            let detail: Vec<String> = problems.iter().map(|(k, v)| format!("minimax_h3_settings.{k}: {}", text_or_empty(Some(v)))).collect();
            Err(OpError::Validation(format!("Invalid scene MiniMax H3 settings: {}. See GET /settings/minimax-h3/schema.", detail.join("; "))))
        }
        Some(_) => Err(OpError::Validation("minimax_h3_settings must be an object of MiniMax H3 settings, or null.".into())),
    }
}

fn find_scene(segs: &[Value], scene_id: &str) -> Option<usize> {
    segs.iter().enumerate().position(|(i, s)| s.get("id").and_then(Value::as_str) == Some(scene_id) || (i + 1).to_string() == scene_id)
}

fn set(seg: &mut Value, key: &str, value: Value) {
    if let Some(map) = seg.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

fn text_of(seg: &Value, key: &str) -> String {
    text_or_empty(seg.get(key))
}

/// Merge-patch scene fields (prompts, card fields, timing, scene settings). Marks the scene's latent dirty when a prompt or
/// the length changed. Unsupported fields are an error and nothing is saved.
pub fn patch_scene(project: &Path, scene_id: &str, patch: &Obj, if_match: Option<i64>, now: &Stamp) -> OpResult {
    if let Some(mode) = patch.get("minimax_h3_i2v_frame_mode") {
        if !matches!(mode, Value::Null) && !matches!(mode.as_str(), Some("normal" | "flf" | "")) {
            return Err(OpError::Validation("minimax_h3_i2v_frame_mode must be normal or flf.".into()));
        }
    }
    let (values, legacy) = parse_scene_card_patch(patch).map_err(OpError::Validation)?;
    let supported = supported_fields();
    let mut unsupported: Vec<&String> = legacy.keys().filter(|k| !supported.contains(k) && k.as_str() != "id" && !(k.starts_with("use_scene_") || k.ends_with("_settings"))).collect();
    unsupported.sort();
    if !unsupported.is_empty() {
        return Err(OpError::Validation(format!(
            "Unsupported scene field{}: {}. Supported fields: {}, plus use_scene_* and *_settings. POST /scenes/bulk with op=patch can write other fields.",
            if unsupported.len() > 1 { "s" } else { "" },
            unsupported.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            supported.join(", ")
        )));
    }
    validate_scene_h3_patch(&legacy)?;
    let card_only = card_only_names(&values);
    if !card_only.is_empty() {
        return Err(OpError::Validation(format!("{} belong to a Storyboard card, which this app does not have.", card_only.join(", "))));
    }
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    sort_segments(&mut segs);
    let idx = find_scene(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    let resolved_id = text_or_empty(segs[idx].get("id")).trim().to_string();
    let resolved_id = if resolved_id.is_empty() { scene_id.to_string() } else { resolved_id };
    let before = segs[idx].clone();

    for key in TEXT_FIELDS {
        if let Some(v) = legacy.get(key) {
            set(&mut segs[idx], key, json!(if truthy(v) { text_or_empty(Some(v)) } else { String::new() }));
        }
    }
    for key in NUMBER_FIELDS {
        if let Some(v) = legacy.get(key) {
            let n = to_float(v).ok_or_else(|| OpError::Validation(format!("{key} must be a number.")))?;
            set(&mut segs[idx], key, json!(n));
        }
    }
    for key in OPTIONAL_NUMBER_FIELDS {
        if let Some(v) = legacy.get(key) {
            if v.is_null() || v.as_str() == Some("") {
                set(&mut segs[idx], key, Value::Null);
                continue;
            }
            let n = to_float(v).filter(|n| n.is_finite() && *n >= 0.0).ok_or_else(|| OpError::Validation(format!("{key} must be a number of seconds that is 0 or more, or null for the default.")))?;
            set(&mut segs[idx], key, json!(h3_core::pyval::round_to(n, 2)));
        }
    }
    let engine = text_or_empty(edit.session.get("video_engine"));
    let image_mode = Some(text_or_empty(edit.session.get("image_model_mode"))).filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "zimage".into());
    let references_changed = apply_scene_references(&mut edit.session, &resolved_id, &values)?;
    let plain: Obj = values.iter().filter(|(k, _)| !matches!(k.as_str(), "subject_ids" | "location_id")).map(|(k, v)| (k.clone(), v.clone())).collect();
    apply_to_segment(&mut segs[idx], &plain, &engine, &image_mode);
    if values.contains_key("lyric_text") && !values.contains_key("lyric_no_lip_sync") {
        let instrumental = is_instrumental_lyric_text(&text_of(&segs[idx], "lyric_text"));
        set(&mut segs[idx], "lyric_no_lip_sync", json!(instrumental));
    }
    for (key, val) in &legacy {
        if key == "minimax_h3_settings" && val.is_object() && segs[idx].get(key).is_some_and(Value::is_object) {
            let mut merged = segs[idx][key].as_object().cloned().unwrap_or_default();
            merged.extend(val.as_object().cloned().unwrap_or_default());
            set(&mut segs[idx], key, Value::Object(merged));
        } else if key.starts_with("use_scene_") || key.ends_with("_settings") {
            set(&mut segs[idx], key, val.clone());
        }
    }
    let prompt_changed = PROMPT_KEYS.iter().any(|k| text_of(&before, k) != text_of(&segs[idx], k));
    let dur = |s: &Value| h3_core::timeline::num(s, "end", 0.0) - h3_core::timeline::num(s, "start", 0.0);
    let duration_changed = (dur(&before) - dur(&segs[idx])).abs() > 0.05;
    if prompt_changed || duration_changed {
        crate::latents::mark_dirty(&edit.project, idx as u32 + 1, "Predecessor scene re-rendered or modified", now);
    }
    if legacy.contains_key("start") || legacy.contains_key("end") {
        normalize_segments(&mut segs, idx);
    }
    let scene = segs[idx].clone();
    let _ = references_changed;
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"scene": scene, "revision": revision}))
}

const PROMPT_FIELDS: [&str; 9] = ["t2i_prompt", "i2v_prompt", "minimax_h3_prompt", "minimax_h3_pass2_prompt", "flux_prompt", "nb_prompt", "flow_gpt_prompt", "ernie_t2i_prompt", "enhance_prompt"];

/// Set one prompt field of a scene directly, with where it came from.
pub fn set_scene_prompt(project: &Path, scene_id: &str, field: &str, prompt: &str, origin: &str, if_match: Option<i64>, now: &Stamp) -> OpResult {
    if !PROMPT_FIELDS.contains(&field) {
        return Err(OpError::Validation(format!("Field '{field}' is not an editable prompt field.")));
    }
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    let idx = find_scene(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    set(&mut segs[idx], field, json!(prompt));
    set(&mut segs[idx], &format!("{field}_origin"), json!(origin));
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"scene_id": scene_id, "field": field, "prompt": prompt, "origin": origin, "revision": revision}))
}

/// Replace the project lyrics (`full_lyrics.txt` and `canonical_lyrics`).
pub fn set_lyrics(project: &Path, lyrics_text: Option<&str>, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    if let Some(text) = lyrics_text {
        let dir = context_folder(&edit.project);
        fs::create_dir_all(&dir)?;
        atomic_write_text(&dir.join("full_lyrics.txt"), text)?;
        edit.session.insert("canonical_lyrics".into(), json!(text));
    }
    let revision = edit.save(now)?;
    Ok(json!({"lyrics_saved": lyrics_text.is_some(), "revision": revision}))
}

fn card_list<'a>(session: &'a mut Obj, kind: &str) -> Result<&'a mut Vec<Value>, OpError> {
    let key = match kind {
        "subjects" | "subject" => "subjects",
        "locations" | "location" => "locations",
        _ => return Err(OpError::Validation("kind must be 'subjects' or 'locations'.".into())),
    };
    let refs = session.entry("flux_reference_builder").or_insert_with(|| json!({}));
    let list = refs.as_object_mut().ok_or_else(|| OpError::Validation("The reference builder is not an object.".into()))?.entry(key).or_insert_with(|| json!([]));
    list.as_array_mut().ok_or_else(|| OpError::Validation(format!("{key} is not a list.")))
}

/// Create or update a character or location card. Only the fields sent change. Existing cards keep their other keys.
pub fn upsert_reference(project: &Path, kind: &str, card_id: &str, payload: &Obj, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let is_subject = matches!(kind, "subjects" | "subject");
    let list = card_list(&mut edit.session, kind)?;
    let existing = list.iter().position(|c| c.get("id").and_then(Value::as_str) == Some(card_id));
    let old = existing.map(|i| list[i].clone()).unwrap_or(Value::Null);
    let pick = |key: &str, default: Value| -> Value { payload.get(key).cloned().or_else(|| old.get(key).cloned()).unwrap_or(default) };
    let card = if is_subject {
        json!({
            "id": card_id,
            "name": pick("name", json!("Character")),
            "description": pick("description", json!("")),
            "face_description": pick("face_description", json!("")),
            "reference_type": pick("reference_type", json!("character")),
            "minimax_voice": pick("minimax_voice", json!("none")),
            "trigger_phrase": pick("trigger_phrase", json!("")),
            "trigger_position": pick("trigger_position", json!("start")),
            "extra_reference_for": pick("extra_reference_for", json!("")),
            "extra_reference_note": pick("extra_reference_note", json!("")),
            "image": pick("image", json!({})),
        })
    } else {
        json!({
            "id": card_id,
            "name": payload.get("name").cloned().unwrap_or_else(|| json!("Location")),
            "description": payload.get("description").cloned().unwrap_or_else(|| json!("")),
            "trigger_phrase": payload.get("trigger_phrase").cloned().unwrap_or_else(|| json!("")),
            "trigger_position": payload.get("trigger_position").cloned().unwrap_or_else(|| json!("start")),
            "image": payload.get("image").cloned().unwrap_or_else(|| json!({})),
        })
    };
    let saved = match existing {
        Some(i) => {
            if let (Some(target), Some(new)) = (list[i].as_object_mut(), card.as_object()) {
                for (k, v) in new {
                    target.insert(k.clone(), v.clone());
                }
            }
            list[i].clone()
        }
        None => {
            list.push(card.clone());
            card
        }
    };
    let revision = edit.save(now)?;
    Ok(json!({if is_subject { "subject" } else { "location" }: saved, "revision": revision}))
}

/// Add the locations the scout found. A name already on a card (any letter case) is not added again, and fills in that
/// card's description when it has none. `new_id` makes the id of each new card.
pub fn merge_scouted_locations(project: &Path, found: &[(String, String)], style_notes: &str, new_id: &mut dyn FnMut(&[String]) -> String, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let list = card_list(&mut edit.session, "locations")?;
    let mut used: Vec<String> = list.iter().map(|c| text_or_empty(c.get("id"))).collect();
    let (mut added, mut updated) = (0, 0);
    for (name, description) in found {
        if name.is_empty() {
            continue;
        }
        let key = name.to_lowercase();
        let position = list.iter().position(|c| text_or_empty(c.get("name")).trim().to_lowercase() == key);
        match position {
            None => {
                let id = new_id(&used);
                used.push(id.clone());
                list.push(json!({"id": id, "name": name, "description": description, "image": {"path": "", "data": "", "name": ""}}));
                added += 1;
            }
            Some(i) => {
                let empty = text_or_empty(list[i].get("description")).trim().is_empty();
                if empty && !description.is_empty() {
                    list[i]["description"] = json!(description);
                    updated += 1;
                }
            }
        }
    }
    let cards: Vec<Value> = list.clone();
    let refs = edit.session.get_mut("flux_reference_builder").and_then(Value::as_object_mut).expect("card_list made it");
    refs.insert("locations_cleared".into(), json!(false));
    refs.insert("location_style_theme".into(), json!(style_notes));
    refs.insert("use_location_references".into(), json!(!cards.is_empty()));
    let revision = edit.save(now)?;
    let summary: Vec<Value> = cards.iter().map(|c| json!({"id": c["id"], "name": c["name"]})).collect();
    Ok(json!({"added": added, "updated": updated, "total_locations": cards.len(), "locations": summary, "revision": revision}))
}

/// Delete a card and take it out of the scene maps.
pub fn delete_reference(project: &Path, kind: &str, card_id: &str, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let is_subject = matches!(kind, "subjects" | "subject");
    let list = card_list(&mut edit.session, kind)?;
    let before = list.len();
    list.retain(|c| c.get("id").and_then(Value::as_str) != Some(card_id));
    let deleted = list.len() < before;
    if is_subject {
        let map = reference_map(&mut edit.session, "subjects");
        for (_, v) in map.iter_mut() {
            if let Value::Array(items) = v {
                items.retain(|i| i.as_str() != Some(card_id));
            }
        }
    } else {
        let map = reference_map(&mut edit.session, "locations");
        let stale: Vec<String> = map.iter().filter(|(_, v)| v.as_str() == Some(card_id)).map(|(k, _)| k.clone()).collect();
        for k in stale {
            map.shift_remove(&k);
        }
    }
    let revision = edit.save(now)?;
    Ok(json!({"deleted": deleted, "kind": if is_subject { "subjects" } else { "locations" }, "id": card_id, "revision": revision}))
}

/// The scene maps as the API shows them.
pub fn scene_mapping_view(session: &mut Obj) -> Value {
    let mut out = serde_json::Map::new();
    for (name, _) in REFERENCE_MAP_KEYS {
        out.insert(name.to_string(), Value::Object(reference_map(session, name).clone()));
    }
    Value::Object(out)
}

/// Set which characters and locations each scene uses. Sending `subjects` or `locations` also turns on the Builder's
/// "use reference" switch.
pub fn update_scene_mapping(project: &Path, mapping: &Obj, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    for (name, _) in REFERENCE_MAP_KEYS {
        let current = reference_map(&mut edit.session, name);
        if let Some(Value::Object(update)) = mapping.get(name) {
            for (k, v) in update {
                current.insert(k.clone(), v.clone());
            }
        }
    }
    for (name, list_key, switch) in [("subjects", "subjects", "use_subject_reference"), ("locations", "locations", "use_location_references")] {
        if mapping.get(name).is_some_and(Value::is_object) {
            let any = reference_map(&mut edit.session, name).values().any(truthy);
            let cards = edit.session.get("flux_reference_builder").and_then(|r| r.get(list_key)).is_some_and(truthy);
            edit.session["flux_reference_builder"][switch] = json!(cards || any);
        }
    }
    let view = scene_mapping_view(&mut edit.session);
    let revision = edit.save(now)?;
    Ok(json!({"scene_mapping": view, "revision": revision}))
}

// -- projects ---------------------------------------------------------------------------------------------------------------

/// The folder name for a project name: letters, digits, `_`, `.`, `-` and spaces.
pub fn safe_project_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_bad = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ' ' | '-') {
            out.push(c);
            last_bad = false;
        } else if !last_bad {
            out.push('_');
            last_bad = true;
        }
    }
    let out = out.trim_matches([' ', '.', '_']).to_string();
    if out.is_empty() { "VRGDG_MusicVideoBuilder".into() } else { out }
}

/// Create a project folder under `root` with an empty session for MiniMax H3. Returns the folder and the saved revision.
pub fn create_project(root: &Path, name: &str, now: &Stamp) -> Result<(PathBuf, i64), OpError> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err(OpError::Validation("Project name cannot be empty.".into()));
    }
    fs::create_dir_all(root)?;
    let folder = crate::scaffold::new_project(&root.join(safe_project_name(clean)))?;
    let mut session = Obj::new();
    session.insert("project_name".into(), json!(clean));
    session.insert("project_folder".into(), json!(folder.display().to_string()));
    session.insert("segments".into(), json!([]));
    session.insert("video_engine".into(), json!("minimax_h3"));
    session.insert("image_model_mode".into(), json!("zimage"));
    let saved = crate::session::save_session(&folder, crate::session::SaveInput { session, audio_path: String::new(), now: now.clone() })?;
    Ok((folder, saved.revision))
}

/// Delete a project folder. The caller has checked the confirmation.
pub fn delete_project(folder: &Path) -> Result<(), OpError> {
    if !crate::layout::session_path(folder).is_file() {
        return Err(OpError::Validation("That folder is not a project.".into()));
    }
    fs::remove_dir_all(folder)?;
    Ok(())
}

// -- audio ------------------------------------------------------------------------------------------------------------------

/// What an analysed song gives the session.
pub struct AudioInfo {
    pub saved_path: String,
    pub duration: f64,
    pub peaks: Vec<f64>,
    pub beats: Vec<f64>,
    pub tempo_bpm: f64,
}

/// Record the project's song in the session with the keys the Builder saves.
pub fn attach_audio(project: &Path, info: &AudioInfo, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    edit.session.insert("audio_path".into(), json!(info.saved_path));
    edit.session.insert("audio_duration".into(), json!(info.duration));
    edit.session.insert("audio_peaks".into(), json!(info.peaks));
    edit.session.insert("beat_markers".into(), json!(info.beats));
    if info.tempo_bpm > 0.0 {
        edit.session.insert("detected_tempo_bpm".into(), json!(info.tempo_bpm));
    }
    let tempo = edit.session.get("detected_tempo_bpm").cloned().unwrap_or(Value::Null);
    let revision = edit.save(now)?;
    Ok(json!({"audio_path": info.saved_path, "duration": info.duration, "peaks_count": info.peaks.len(), "beat_count": info.beats.len(), "tempo_bpm": tempo, "revision": revision}))
}

/// Beat markers as the Builder saves them (`beat_markers`), else the older `beats` key.
pub fn session_beats(session: &Obj) -> Vec<Value> {
    let list = |key: &str| session.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let markers = list("beat_markers");
    if markers.is_empty() { list("beats") } else { markers }
}

/// Replace the beat markers and optionally the tempo.
pub fn set_beats(project: &Path, beats: Vec<Value>, tempo_bpm: Option<f64>, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let count = beats.len();
    edit.session.insert("beat_markers".into(), Value::Array(beats));
    edit.session.shift_remove("beats");
    if let Some(t) = tempo_bpm {
        edit.session.insert("detected_tempo_bpm".into(), json!(t));
    }
    let tempo = edit.session.get("detected_tempo_bpm").cloned().unwrap_or(Value::Null);
    let revision = edit.save(now)?;
    Ok(json!({"beat_count": count, "tempo_bpm": tempo, "revision": revision}))
}

/// Shift every beat by `offset` seconds.
pub fn calibrate_beats(project: &Path, offset: f64, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let shifted: Vec<Value> = session_beats(&edit.session)
        .into_iter()
        .map(|b| match b {
            Value::Object(mut map) => {
                let t = map.get("time").and_then(to_float).unwrap_or(0.0) + offset;
                map.insert("time".into(), json!(h3_core::pyval::round_to(t, 4)));
                Value::Object(map)
            }
            other => json!(h3_core::pyval::round_to(to_float(&other).unwrap_or(0.0) + offset, 4)),
        })
        .collect();
    let count = shifted.len();
    edit.session.insert("beat_markers".into(), Value::Array(shifted));
    edit.session.shift_remove("beats");
    edit.session.insert("beat_calibration".into(), json!({"offset_seconds": offset, "calibrated": true}));
    let revision = edit.save(now)?;
    Ok(json!({"calibrated": true, "offset_seconds": offset, "beat_count": count, "revision": revision}))
}

/// Merge fields into one scene and save. Used by writers (prompts, descriptions) that produce several keys at once.
pub fn set_scene_fields(project: &Path, scene_id: &str, fields: &Obj, now: &Stamp) -> OpResult {
    let mut edit = open(project, None)?;
    let mut segs = edit.segments();
    let idx = find_scene(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    for (key, value) in fields {
        set(&mut segs[idx], key, value.clone());
    }
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"scene_id": scene_id, "revision": revision}))
}

/// Choose the ordered MiniMax references of one scene (`keys`), or hand it back to the scene mappings (`None`).
pub fn set_scene_reference_keys(project: &Path, scene_id: &str, keys: Option<&Value>, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let segs = edit.segments();
    let idx = find_scene(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    let value = match keys {
        None => Value::Null,
        Some(keys) => {
            let choices = scene_reference_choices(&Value::Object(edit.session.clone()), &segs, idx);
            json!(h3_core::scene::validate_reference_keys(&choices, keys).map_err(OpError::Validation)?)
        }
    };
    let mut segs = segs;
    set(&mut segs[idx], "minimax_h3_reference_keys", value);
    edit.put_segments(segs);
    let session = Value::Object(edit.session.clone());
    let segments = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut answer = scene_reference_choices(&session, &segments, idx);
    let revision = edit.save(now)?;
    if let Some(o) = answer.as_object_mut() {
        o.insert("pipeline".into(), json!("standard"));
        o.insert("revision".into(), json!(revision));
    }
    Ok(answer)
}

/// What the "Choose MiniMax References" picker shows for a scene of these segments.
pub fn scene_reference_choices(session: &Value, segments: &[Value], index: usize) -> Value {
    let segment = segments[index].as_object().cloned().unwrap_or_default();
    let settings = h3_core::settings::settings_for_scene(session, segments.get(index));
    let mode = settings.get("video_mode").and_then(Value::as_str).unwrap_or("");
    h3_core::scene::reference_choices(session, &segment, index, mode)
}

/// Several scene edits (`move`, `resize`, `patch`) in one write. Nothing is saved when one of them fails. Port of
/// `bulk_scene_operations`.
pub fn bulk_scene_ops(project: &Path, operations: &[Value], if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let mut segs = edit.segments();
    let mut applied = 0;
    let missing = |op: &Value, key: &str| OpError::Validation(format!("Operation {} needs `{key}`.", op));
    for op in operations {
        let kind = ["op", "action"].iter().filter_map(|k| op.get(*k)).find(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_default();
        let scene_id = || op.get("scene_id").and_then(Value::as_str).map(str::to_string);
        match kind.as_str() {
            "move" => {
                let id = scene_id().ok_or_else(|| missing(op, "scene_id"))?;
                let start = op.get("start").and_then(to_float).ok_or_else(|| missing(op, "start"))?;
                let ripple = op.get("ripple").is_some_and(truthy);
                h3_core::timeline::move_scene_timing(&mut segs, &id, start, ripple).map_err(|_| OpError::SceneNotFound(id))?;
                applied += 1;
            }
            "resize" => {
                let id = scene_id().ok_or_else(|| missing(op, "scene_id"))?;
                let ripple = op.get("ripple").is_some_and(truthy);
                let duration = op.get("duration").and_then(to_float);
                let end = op.get("end").and_then(to_float);
                h3_core::timeline::resize_scene_timing(&mut segs, &id, duration, end, ripple).map_err(|_| OpError::SceneNotFound(id))?;
                applied += 1;
            }
            "patch" => {
                let id = scene_id().ok_or_else(|| missing(op, "scene_id"))?;
                let Some(idx) = segs.iter().position(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str())) else { continue };
                let fields = op.get("fields").and_then(Value::as_object).cloned().unwrap_or_default();
                for (k, v) in &fields {
                    set(&mut segs[idx], k, v.clone());
                }
                if fields.contains_key("lyric_text") && !fields.contains_key("lyric_no_lip_sync") {
                    let instrumental = is_instrumental_lyric_text(&text_or_empty(segs[idx].get("lyric_text")));
                    set(&mut segs[idx], "lyric_no_lip_sync", json!(instrumental));
                }
                applied += 1;
            }
            _ => {}
        }
    }
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"applied_operations": applied, "revision": revision}))
}

/// Assemble the official MiniMax prompt from shot descriptions and optionally save it on the scene. Port of
/// `assemble_minimax_prompt_endpoint`.
pub fn assemble_scene_prompt(project: &Path, scene_id: &str, shots: &[String], mode: Option<&str>, save: bool, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let current = ["revision", "builder_save_revision"].iter().filter_map(|k| edit.session.get(*k)).find(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64);
    let session = edit.session.clone();
    let mut segs = edit.segments();
    let idx = find_scene(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    let full = Value::Array(segs.clone());
    let mut session_with = session;
    session_with.insert("segments".into(), full);
    let segment = segs[idx].as_object().cloned().unwrap_or_default();
    let assembled = h3_core::prompt::assembly::assemble_minimax_h3_prompt(&segment, &session_with, shots, mode).map_err(OpError::Validation)?;
    let prompt = text_or_empty(assembled.get("prompt"));
    let used_mode = text_or_empty(assembled.get("mode"));
    let validation = h3_core::prompt::assembly::validate_minimax_h3_prompt(&prompt, Some(&segment), Some(&used_mode), false);
    let mut revision = current;
    if save {
        set(&mut segs[idx], "minimax_h3_prompt", json!(prompt));
        set(&mut segs[idx], "minimax_h3_prompt_origin", json!("agent"));
        edit.put_segments(segs);
        revision = edit.save(now)?;
    }
    Ok(json!({
        "prompt": prompt, "characters": assembled["characters"], "shots_used": assembled["shots_used"],
        "valid": validation["valid"], "errors": validation["errors"], "warnings": validation["warnings"],
        "saved": save, "revision": revision,
    }))
}

/// Make `saved_path` the scene's start image and the newest entry of its image history.
pub fn set_scene_image_from_frame(project: &Path, scene_id: &str, saved_path: &str, now: &Stamp) -> OpResult {
    let mut edit = open(project, None)?;
    let mut segs = edit.segments();
    let idx = find_scene(&segs, scene_id).ok_or_else(|| OpError::SceneNotFound(scene_id.to_string()))?;
    set(&mut segs[idx], "custom_image_path", json!(saved_path));
    let mut history: Vec<Value> = segs[idx].get("image_history").and_then(Value::as_array).cloned().unwrap_or_default();
    if !history.iter().any(|h| h.as_str() == Some(saved_path)) {
        history.push(json!(saved_path));
    }
    let last = history.len() - 1;
    set(&mut segs[idx], "image_history", Value::Array(history));
    set(&mut segs[idx], "image_history_index", json!(last));
    set(&mut segs[idx], "preview_mode", json!("image"));
    edit.put_segments(segs);
    let revision = edit.save(now)?;
    Ok(json!({"scene_id": scene_id, "saved_path": saved_path, "revision": revision}))
}
