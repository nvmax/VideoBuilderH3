//! What a save changed, for the windows that show the project. A save that did not come from the Video Builder itself (the agent API, the MCP server, a job
//! finishing) is described and handed to the listeners, which tell the open Builder. The description is the one the Builder's own merge code reads
//! (`music_video_builder/external_changes.mjs`):
//!
//! - `{"kind": "scene_fields", "scenes": {scene_id: {"segment": [keys], "references": ["subjects", "locations"]}}, "marker_ids": [...]}` when only scene fields,
//!   the characters or location mapped to a scene, or Timeline Notes changed. The Builder merges these field by field and keeps what the user has not saved.
//! - `{"kind": "project"}` for anything else. The Builder then reloads the project when nothing is unsaved, focused or rendering.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use h3_core::pyval::Obj;
use serde_json::{json, Map, Value};

/// One save of a project, as the listeners see it.
#[derive(Debug, Clone)]
pub struct SavedProject {
    pub project_folder: PathBuf,
    pub revision: i64,
    pub builder_save_revision: i64,
    pub change: Value,
}

pub type Listener = Arc<dyn Fn(&SavedProject) + Send + Sync>;

/// Listeners with the project folders they care about (a project folder lies inside one of the roots).
static LISTENERS: RwLock<Vec<(Vec<String>, Listener)>> = RwLock::new(Vec::new());

/// A folder as a comparable text: absolute, forward slashes, lower case, no trailing slash.
fn key(path: &Path) -> String {
    let absolute = h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    absolute.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Call `listener` after every save of a project inside one of `roots` that the Video Builder did not make itself.
pub fn on_saved(roots: &[PathBuf], listener: Listener) {
    let keys = roots.iter().map(|r| key(r)).collect();
    LISTENERS.write().unwrap_or_else(|e| e.into_inner()).push((keys, listener));
}

fn is_inside(folder: &str, root: &str) -> bool {
    folder == root || folder.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

/// Whether anyone listens for saves of this project.
pub(crate) fn listening_to(folder: &Path) -> bool {
    let folder = key(folder);
    LISTENERS.read().unwrap_or_else(|e| e.into_inner()).iter().any(|(roots, _)| roots.iter().any(|r| is_inside(&folder, r)))
}

/// Tell the listeners of this project that it was saved. `previous` is the session as it was before.
pub(crate) fn announce(folder: &Path, previous: Option<&Obj>, saved: &Obj) {
    let number = |name: &str| saved.get(name).and_then(Value::as_i64).unwrap_or(0);
    let event = SavedProject { project_folder: folder.to_path_buf(), revision: number("revision"), builder_save_revision: number("builder_save_revision"), change: describe_change(previous, saved) };
    let wanted = key(folder);
    // The listeners are copied out first, so one that saves again cannot deadlock the registry.
    let chosen: Vec<Listener> = LISTENERS.read().unwrap_or_else(|e| e.into_inner()).iter().filter(|(roots, _)| roots.iter().any(|r| is_inside(&wanted, r))).map(|(_, l)| Arc::clone(l)).collect();
    for listener in chosen {
        listener(&event);
    }
}

/// Keys that change with every save, or that only the Builder window itself changes (its layout and view), so they never make a change "the project".
const NOT_THE_PROJECT: [&str; 22] = [
    "revision", "builder_save_revision", "updated", "project_folder", "render_logs", "active_render_log_id", "active_scene_id", "active_track", "selected_timeline_range",
    "active_timeline_marker_id", "left_panel_width", "left_panel_collapsed", "right_panel_width", "right_panel_collapsed", "left_panel_tab", "timeline_panel_height",
    "timeline_zoom", "llm_popout_open", "llm_popout_width", "llm_popout_height", "llm_popout_x", "llm_popout_y",
];
/// The parts of the session that are compared on their own.
const COMPARED_APART: [&str; 4] = ["segments", "overlay_segments", "flux_reference_builder", "timeline_markers"];
/// Parts of the reference builder that belong to a scene: which characters and which location it uses.
const SCENE_REFERENCES: [(&str, &str, &str); 2] = [("subjects", "subject_scene_map", "use_subject_reference"), ("locations", "scene_map", "use_location_references")];

fn project() -> Value {
    json!({"kind": "project"})
}

fn list<'a>(session: &'a Obj, name: &str) -> &'a [Value] {
    session.get(name).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn id_of(item: &Value) -> Option<&str> {
    item.get("id").and_then(Value::as_str)
}

/// Describe what a save changed, as the Builder's merge reads it. Without a previous session, or when the change is more than scene fields, it is `project`.
pub fn describe_change(previous: Option<&Obj>, saved: &Obj) -> Value {
    let Some(before) = previous else { return project() };
    // Anything outside the parts below that differs means the Builder should reload.
    let keys: std::collections::BTreeSet<&String> = before.keys().chain(saved.keys()).collect();
    for name in keys {
        if NOT_THE_PROJECT.contains(&name.as_str()) || COMPARED_APART.contains(&name.as_str()) {
            continue;
        }
        if before.get(name) != saved.get(name) {
            return project();
        }
    }

    let mut scenes: Map<String, Value> = Map::new();
    for name in ["segments", "overlay_segments"] {
        let (old, new) = (list(before, name), list(saved, name));
        if old.len() != new.len() || old.iter().zip(new).any(|(a, b)| id_of(a).is_none() || id_of(a) != id_of(b)) {
            return project();
        }
        for (a, b) in old.iter().zip(new) {
            let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else { return project() };
            let names: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).filter(|k| !k.starts_with('_')).collect();
            let changed: Vec<&str> = names.into_iter().filter(|k| a.get(*k) != b.get(*k)).map(String::as_str).collect();
            if !changed.is_empty() {
                scenes.insert(a.get("id").and_then(Value::as_str).unwrap_or_default().to_string(), json!({"segment": changed, "references": []}));
            }
        }
    }

    // The reference builder: only the per scene maps (and the switches that go with them) may differ.
    let empty = Obj::new();
    let (old_refs, new_refs) = (before.get("flux_reference_builder").and_then(Value::as_object).unwrap_or(&empty), saved.get("flux_reference_builder").and_then(Value::as_object).unwrap_or(&empty));
    let mut maps_changed = false;
    let names: std::collections::BTreeSet<&String> = old_refs.keys().chain(new_refs.keys()).collect();
    for name in names {
        let belongs_to_scenes = SCENE_REFERENCES.iter().any(|(_, map, switch)| name == map || name == switch);
        if !belongs_to_scenes && old_refs.get(name) != new_refs.get(name) {
            return project();
        }
    }
    for (api_name, map, switch) in SCENE_REFERENCES {
        let (old_map, new_map) = (old_refs.get(map).and_then(Value::as_object).unwrap_or(&empty), new_refs.get(map).and_then(Value::as_object).unwrap_or(&empty));
        let ids: std::collections::BTreeSet<&String> = old_map.keys().chain(new_map.keys()).collect();
        for id in ids {
            if old_map.get(id) != new_map.get(id) {
                maps_changed = true;
                let entry = scenes.entry(id.clone()).or_insert_with(|| json!({"segment": [], "references": []}));
                if let Some(names) = entry.get_mut("references").and_then(Value::as_array_mut) {
                    names.push(json!(api_name));
                }
            }
        }
        if old_refs.get(switch) != new_refs.get(switch) && !maps_changed {
            return project();
        }
    }

    // Timeline Notes: the ids that were added, changed or removed.
    let (old_markers, new_markers) = (list(before, "timeline_markers"), list(saved, "timeline_markers"));
    let mut marker_ids: Vec<String> = Vec::new();
    for marker in new_markers {
        let Some(id) = id_of(marker) else { return project() };
        if old_markers.iter().find(|m| id_of(m) == Some(id)) != Some(marker) {
            marker_ids.push(id.to_string());
        }
    }
    for marker in old_markers {
        let Some(id) = id_of(marker) else { return project() };
        if !new_markers.iter().any(|m| id_of(m) == Some(id)) {
            marker_ids.push(id.to_string());
        }
    }

    if scenes.is_empty() && marker_ids.is_empty() {
        // Nothing the Builder shows changed (a bookkeeping save). It still needs the revision, so it is announced as an empty field change.
        return json!({"kind": "scene_fields", "scenes": {}});
    }
    let mut change = json!({"kind": "scene_fields", "scenes": Value::Object(scenes)});
    if !marker_ids.is_empty() {
        change["marker_ids"] = json!(marker_ids);
    }
    change
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn base() -> Obj {
        session(json!({
            "revision": 4, "builder_save_revision": 4, "updated": 1, "video_type": "singing",
            "segments": [{"id": "a", "label": "One", "start": 0, "end": 4, "_runtime": 1}, {"id": "b", "label": "Two", "start": 4, "end": 8}],
            "flux_reference_builder": {"subjects": [{"id": "s1"}], "subject_scene_map": {"a": ["s1"]}, "scene_map": {}, "use_subject_reference": true},
            "timeline_markers": [{"id": "m1", "start": 1}],
        }))
    }

    #[test]
    fn a_first_save_and_a_new_project_setting_are_the_project() {
        assert_eq!(describe_change(None, &base()), json!({"kind": "project"}));
        let mut after = base();
        after.insert("video_type".into(), json!("speaking"));
        assert_eq!(describe_change(Some(&base()), &after), json!({"kind": "project"}));
    }

    #[test]
    fn scene_fields_name_the_scene_and_the_keys_that_changed() {
        let mut after = base();
        after["segments"][0]["label"] = json!("Opening");
        after["segments"][0]["minimax_h3_prompt"] = json!("A singer.");
        after["segments"][0]["_runtime"] = json!(2);
        after.insert("revision".into(), json!(5));
        let change = describe_change(Some(&base()), &after);
        assert_eq!(change["kind"], "scene_fields");
        assert_eq!(change["scenes"]["a"]["segment"], json!(["label", "minimax_h3_prompt"]));
        assert!(change["scenes"].get("b").is_none(), "{change}");
    }

    #[test]
    fn a_scene_added_removed_or_moved_is_the_project() {
        let mut added = base();
        added["segments"].as_array_mut().unwrap().push(json!({"id": "c"}));
        assert_eq!(describe_change(Some(&base()), &added), json!({"kind": "project"}));
        let mut swapped = base();
        swapped["segments"].as_array_mut().unwrap().reverse();
        assert_eq!(describe_change(Some(&base()), &swapped), json!({"kind": "project"}));
    }

    #[test]
    fn the_character_or_location_of_a_scene_is_a_reference_change() {
        let mut after = base();
        after["flux_reference_builder"]["scene_map"] = json!({"b": ["loc1"]});
        let change = describe_change(Some(&base()), &after);
        assert_eq!((change["kind"].clone(), change["scenes"]["b"]["references"].clone()), (json!("scene_fields"), json!(["locations"])));
        // A new character card is more than a scene field.
        let mut card = base();
        card["flux_reference_builder"]["subjects"] = json!([{"id": "s1"}, {"id": "s2"}]);
        assert_eq!(describe_change(Some(&base()), &card), json!({"kind": "project"}));
    }

    #[test]
    fn timeline_notes_are_named_by_id() {
        let mut after = base();
        after["timeline_markers"] = json!([{"id": "m1", "start": 2}, {"id": "m2", "start": 5}]);
        assert_eq!(describe_change(Some(&base()), &after)["marker_ids"], json!(["m1", "m2"]));
        after["timeline_markers"] = json!([]);
        assert_eq!(describe_change(Some(&base()), &after)["marker_ids"], json!(["m1"]));
    }

    #[test]
    fn a_save_that_changed_nothing_the_builder_shows_still_carries_its_revision() {
        let mut after = base();
        after.insert("revision".into(), json!(5));
        after.insert("updated".into(), json!(99));
        assert_eq!(describe_change(Some(&base()), &after), json!({"kind": "scene_fields", "scenes": {}}));
    }

    #[test]
    fn a_project_folder_is_inside_its_root_and_not_a_sibling_of_it() {
        assert!(is_inside("c:/projects/song", "c:/projects"));
        assert!(is_inside("c:/projects", "c:/projects"));
        assert!(!is_inside("c:/projects-other/song", "c:/projects"));
    }
}
