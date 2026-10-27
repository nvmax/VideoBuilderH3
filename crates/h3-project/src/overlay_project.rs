//! The overlay layers saved in the session (`overlay_layer_track`, `overlay_layers`) for the agent API: read it, replace it, add clips to it. The keys and the clip rules are the
//! ones the Video Builder saves, so a project an agent edited opens in the Builder with the same overlay lane.

use std::path::Path;

use h3_core::overlay_layers::{normalize_overlay_layers, normalize_overlay_layer_track, MAX_CLIPS};
use h3_core::pyval::Obj;
use serde_json::{json, Value};

use crate::layout::Stamp;
use crate::scene_ops::{current_revision, open, OpError};

/// The keys `PUT /projects/{pid}/overlay-layers` accepts.
pub const OVERLAY_KEYS: [&str; 2] = ["overlay_layer_track", "overlay_layers"];

/// `fx_clips` and `fx_track` are what the first builds saved the overlays under, before they were called overlays. They are read when the new keys are not
/// there and dropped when the new ones are written.
fn clips_of(session: &Obj) -> Vec<Value> {
    let saved = session.get("overlay_layers").or_else(|| session.get("fx_clips")).unwrap_or(&Value::Null);
    normalize_overlay_layers(saved).into_iter().map(Value::Object).collect()
}

/// A clip an agent sends without a lock setting is unlocked, so the person can drag it on the Builder's lane at once. (The data default for a clip saved
/// without one is locked, like the overlay track's clips, and that stays for what the Builder itself saves.)
fn unlocked(clip: &Value) -> Value {
    let mut clip = clip.clone();
    if let Some(map) = clip.as_object_mut() {
        map.entry("locked").or_insert(json!(false));
    }
    clip
}

fn track_of(session: &Obj) -> Value {
    Value::Object(normalize_overlay_layer_track(session.get("overlay_layer_track").or_else(|| session.get("fx_track")).unwrap_or(&Value::Null)))
}

/// The overlays of a loaded session as the render sees it: every setting filled in and clamped.
pub fn overlay_view(session: &Obj) -> Value {
    let clips = clips_of(session);
    json!({"overlay_layer_track": track_of(session), "overlay_layers": clips, "count": clips.len(), "revision": current_revision(session)})
}

fn save(mut edit: crate::scene_ops::Edit, clips: Vec<Value>, track: Option<Value>, now: &Stamp) -> Result<Value, OpError> {
    edit.session.insert("overlay_layers".into(), Value::Array(clips));
    edit.session.remove("fx_clips");
    if let Some(track) = track {
        edit.session.insert("overlay_layer_track".into(), track);
        edit.session.remove("fx_track");
    }
    let revision = edit.save(now)?;
    Ok(json!({"revision": revision}))
}

/// Replace the track switch and/or the whole clip list. What is saved is what the normalizer made of it (values clamped, ids made unique), and
/// the reply shows that, so a caller sees what its input became.
pub fn put_overlay(project: &Path, fields: &Obj, if_match: Option<i64>, now: &Stamp) -> Result<Value, OpError> {
    let unknown: Vec<&str> = fields.keys().map(String::as_str).filter(|key| !OVERLAY_KEYS.contains(key)).collect();
    if !unknown.is_empty() {
        return Err(OpError::Validation(format!("Unknown field(s): {}. Supported: {}.", unknown.join(", "), OVERLAY_KEYS.join(", "))));
    }
    if fields.is_empty() {
        return Err(OpError::Validation(format!("Send {} (or both).", OVERLAY_KEYS.join(" and/or "))));
    }
    if let Some(clips) = fields.get("overlay_layers") {
        let Some(list) = clips.as_array() else { return Err(OpError::Validation("overlay_layers must be a list of clips.".into())) };
        if list.len() > MAX_CLIPS {
            return Err(OpError::Validation(format!("overlay_layers has {} clips. At most {MAX_CLIPS} are kept.", list.len())));
        }
    }
    if let Some(track) = fields.get("overlay_layer_track") {
        if !track.is_object() {
            return Err(OpError::Validation("overlay_layer_track must be an object such as {\"enabled\": true}.".into()));
        }
    }
    let edit = open(project, if_match)?;
    let clips = match fields.get("overlay_layers") {
        Some(sent) => normalize_overlay_layers(&Value::Array(sent.as_array().into_iter().flatten().map(unlocked).collect())).into_iter().map(Value::Object).collect(),
        None => clips_of(&edit.session),
    };
    let track = fields.get("overlay_layer_track").map(|sent| Value::Object(normalize_overlay_layer_track(sent)));
    save(edit, clips, track, now)?;
    let loaded = crate::session::load_session(project)?;
    Ok(overlay_view(&loaded.session))
}

/// Which earlier clips a new group of clips replaces: the clips of `group` that overlap `range` (seconds, from and to), or all of them when
/// there is no range.
pub struct Replace<'a> {
    pub group: &'a str,
    pub range: Option<(f64, f64)>,
}

fn replaced(clip: &Value, replace: &Replace) -> bool {
    if clip.get("group").and_then(Value::as_str) != Some(replace.group) {
        return false;
    }
    let (start, end) = (clip.get("start").and_then(Value::as_f64).unwrap_or(0.0), clip.get("end").and_then(Value::as_f64).unwrap_or(0.0));
    replace.range.is_none_or(|(from, to)| end > from && start < to)
}

/// Add clips to the end of the track (they draw on top), first dropping the clips `replace` names. The ids of the added clips are in `added`.
pub fn add_clips(project: &Path, new: &[Value], replace: Option<Replace>, if_match: Option<i64>, now: &Stamp) -> Result<Value, OpError> {
    if new.is_empty() {
        return Err(OpError::Validation("There are no clips to add.".into()));
    }
    let edit = open(project, if_match)?;
    let mut clips = clips_of(&edit.session);
    if let Some(replace) = &replace {
        clips.retain(|clip| !replaced(clip, replace));
    }
    let before = clips.len();
    clips.extend(new.iter().map(unlocked));
    let clips: Vec<Value> = normalize_overlay_layers(&Value::Array(clips)).into_iter().map(Value::Object).collect();
    if clips.len() > MAX_CLIPS || clips.len() <= before {
        return Err(OpError::Validation(format!("The track holds at most {MAX_CLIPS} clips.")));
    }
    let added: Vec<Value> = clips[before..].iter().filter_map(|clip| clip.get("id").cloned()).collect();
    save(edit, clips, None, now)?;
    let loaded = crate::session::load_session(project)?;
    let mut view = overlay_view(&loaded.session);
    if let Some(map) = view.as_object_mut() {
        map.insert("added".into(), Value::Array(added));
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(clips: Value) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let session = json!({"revision": 4, "segments": [], "overlay_layers": clips});
        crate::atomic::atomic_write_json(&dir.path().join("vrgdg_builder_session.json"), &session).unwrap();
        dir
    }

    fn view_of(dir: &Path) -> Value {
        overlay_view(&crate::session::load_session(dir).unwrap().session)
    }

    #[test]
    fn an_empty_project_has_a_track_that_is_on_and_no_clips() {
        let dir = project(json!([]));
        let view = view_of(dir.path());
        assert_eq!(view["overlay_layer_track"]["enabled"], json!(true));
        assert_eq!(view["count"], json!(0));
        assert_eq!(view["revision"], json!(4));
    }

    #[test]
    fn put_clamps_what_it_is_given_and_shows_what_was_saved() {
        let dir = project(json!([]));
        let sent = json!({"overlay_layers": [{"kind": "grain", "start": 0, "end": 5, "strength": 99}, {"kind": "nonsense"}], "overlay_layer_track": {"enabled": false}});
        let view = put_overlay(dir.path(), sent.as_object().unwrap(), None, &Stamp::now()).unwrap();
        assert_eq!(view["count"], json!(2), "a clip of an unknown kind is kept, the render skips it and says why");
        assert_eq!(view["overlay_layers"][1]["kind"], json!("nonsense"));
        assert_eq!(view["overlay_layers"][0]["strength"], json!(60.0), "99 is clamped to the largest grain");
        assert_eq!(view["overlay_layers"][0]["locked"], json!(false), "a clip sent without a lock setting can be dragged in the Builder");
        assert_eq!(view["overlay_layer_track"]["enabled"], json!(false));
        assert_eq!(view["revision"], json!(5), "the save counts as one revision");
        let again = view_of(dir.path());
        assert_eq!(again["overlay_layers"], view["overlay_layers"], "what the reply showed is what is on disk");
    }

    #[test]
    fn put_checks_its_input_and_the_revision() {
        let dir = project(json!([]));
        let put = |body: Value, if_match| put_overlay(dir.path(), body.as_object().unwrap(), if_match, &Stamp::now());
        assert!(matches!(put(json!({}), None), Err(OpError::Validation(_))));
        assert!(matches!(put(json!({"clips": []}), None), Err(OpError::Validation(m)) if m.contains("Unknown field(s): clips")));
        assert!(matches!(put(json!({"overlay_layers": "x"}), None), Err(OpError::Validation(_))));
        assert!(matches!(put(json!({"overlay_layer_track": []}), None), Err(OpError::Validation(_))));
        let many: Vec<Value> = (0..MAX_CLIPS + 1).map(|_| json!({"kind": "grain"})).collect();
        assert!(matches!(put(json!({"overlay_layers": many}), None), Err(OpError::Validation(_))));
        assert!(matches!(put(json!({"overlay_layers": []}), Some(2)), Err(OpError::Conflict { current: 4, incoming: 2 })));
        assert!(put(json!({"overlay_layers": []}), Some(4)).is_ok());
        assert_eq!(view_of(dir.path())["revision"], json!(5));
    }

    #[test]
    fn a_project_saved_by_the_first_builds_under_the_fx_names_opens_and_moves_to_the_new_names() {
        let dir = tempfile::tempdir().unwrap();
        let session = json!({"revision": 2, "segments": [], "fx_clips": [{"kind": "vignette", "id": "v", "start": 0, "end": 3}], "fx_track": {"enabled": false}});
        crate::atomic::atomic_write_json(&dir.path().join("vrgdg_builder_session.json"), &session).unwrap();
        let view = view_of(dir.path());
        assert_eq!((view["count"].clone(), view["overlay_layer_track"]["enabled"].clone()), (json!(1), json!(false)), "the old keys are read");
        let sent = json!({"overlay_layers": [{"kind": "grain", "id": "g", "start": 0, "end": 3}], "overlay_layer_track": {"enabled": true}});
        put_overlay(dir.path(), sent.as_object().unwrap(), None, &Stamp::now()).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(dir.path().join("vrgdg_builder_session.json")).unwrap()).unwrap();
        assert!(saved.get("fx_clips").is_none() && saved.get("fx_track").is_none(), "the old keys are gone once the new ones are written");
        assert_eq!(saved["overlay_layers"][0]["id"], json!("g"));
    }

    #[test]
    fn a_track_switch_alone_keeps_the_clips() {
        let dir = project(json!([{"kind": "vignette", "id": "v", "start": 0, "end": 3}]));
        let view = put_overlay(dir.path(), json!({"overlay_layer_track": {"enabled": false}}).as_object().unwrap(), None, &Stamp::now()).unwrap();
        assert_eq!(view["count"], json!(1));
        assert_eq!(view["overlay_layer_track"]["enabled"], json!(false));
    }

    #[test]
    fn added_clips_get_their_own_ids_and_a_group_can_be_replaced_inside_a_range() {
        let caption = |id: &str, start: f64, end: f64| json!({"kind": "text", "id": id, "start": start, "end": end, "text": id, "group": "lyric_captions"});
        let dir = project(json!([{"kind": "grain", "id": "g", "start": 0, "end": 9}, caption("a", 0.0, 2.0), caption("b", 4.0, 6.0)]));
        let fresh = [caption("a", 4.5, 5.5)];
        let view = add_clips(dir.path(), &fresh, Some(Replace { group: "lyric_captions", range: Some((4.0, 8.0)) }), None, &Stamp::now()).unwrap();
        let ids: Vec<&str> = view["overlay_layers"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["g", "a", "a_2"], "b was inside the range and is gone, the old a was outside it and stays, the new a gets another id");
        assert_eq!(view["added"], json!(["a_2"]));
        let all = add_clips(dir.path(), &[caption("z", 0.0, 1.0)], Some(Replace { group: "lyric_captions", range: None }), None, &Stamp::now()).unwrap();
        let kinds: Vec<&str> = all["overlay_layers"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["g", "z"], "no range replaces every caption and leaves other clips alone");
        assert!(matches!(add_clips(dir.path(), &[], None, None, &Stamp::now()), Err(OpError::Validation(_))));
    }
}
