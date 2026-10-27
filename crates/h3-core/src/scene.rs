//! Scene-level render inputs: reference images, start and last frames, video references. Port of
//! `minimax/scene_inputs.py`. Reference maps live in `session["flux_reference_builder"]`, exactly where the Video
//! Builder saves them. File checks are injected so this module does no IO.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::continuity::{continuity_allowed_for_mode, normalize_continuity_mode, MASKED};
use crate::error::CoreError;
use crate::pyval::{text_or_empty, to_float, truthy, Obj};

pub const MAX_REFERENCE_IMAGES: usize = 9;
/// The most Reference Builder images a scene sends. H3 takes nine images in all, so the scene start frame shares the ninth
/// slot with the references and never raises this number. Port of `MAX_SCENE_REFERENCES` in `minimax/scene_inputs.py`.
pub const MAX_SCENE_REFERENCES: usize = 8;
const REFERENCE_BUILDER_MODES: [&str; 3] = ["reference_to_video", "image_reference_to_video", "video_to_video"];

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

fn field(obj: &Obj, key: &str) -> String {
    text(obj.get(key))
}

/// `a or b`: the first truthy value.
fn first_truthy<'a>(obj: &'a Obj, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().filter_map(|k| obj.get(*k)).find(|v| truthy(v))
}

fn objs(value: Option<&Value>) -> Vec<&Obj> {
    value.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_object).collect()).unwrap_or_default()
}

fn flag(obj: &Obj, key: &str) -> bool {
    obj.get(key).is_some_and(truthy)
}

/// Comparison key for media paths: forward slashes, no doubled slashes, lower case.
pub fn media_path_key(path: Option<&Value>) -> String {
    key_of_text(&text(path))
}

fn key_of_text(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    s.to_lowercase()
}

fn clean_choice(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_lowercase().replace('-', " ").split_whitespace().collect::<Vec<_>>().join("_")
}

fn uses_scene_image_as_start_frame(segment: &Obj) -> bool {
    let choice = clean_choice(segment.get("minimax_h3_scene_image_use"));
    flag(segment, "minimax_h3_use_scene_image_as_start_frame")
        || matches!(choice.as_str(), "exact" | "exact_start" | "exact_start_frame")
}

fn has_image(image: Option<&Value>) -> bool {
    image.and_then(Value::as_object).is_some_and(|o| !field(o, "path").is_empty() || !field(o, "data").is_empty())
}

pub(crate) fn split_ids(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(|i| text(Some(i))).filter(|t| !t.is_empty()).collect(),
        other => text_or_empty(other).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Look up by scene id, else by 1-based number when every key is a number.
pub fn scene_map_value(mapping: Option<&Value>, segment: &Obj, index: usize) -> Option<Value> {
    let empty = Obj::new();
    let source = mapping.and_then(Value::as_object).unwrap_or(&empty);
    let id = field(segment, "id");
    if !id.is_empty() {
        if let Some(v) = source.get(&id) {
            return Some(v.clone());
        }
    }
    if source.keys().any(|k| !is_digits(k.trim())) {
        return None;
    }
    source.get(&(index + 1).to_string()).cloned()
}

pub(crate) fn builder(session: &Value) -> Obj {
    session.get("flux_reference_builder").and_then(Value::as_object).cloned().unwrap_or_default()
}

fn extra_target_id(subject: &Obj) -> String {
    text(first_truthy(subject, &["extra_reference_for", "extraReferenceFor", "same_subject_as", "sameSubjectAs"]))
}

fn logical_subjects(refs: &Obj) -> Vec<&Obj> {
    objs(refs.get("subjects")).into_iter().filter(|s| extra_target_id(s).is_empty()).collect()
}

fn subject_ids_for_scene(refs: &Obj, segment: &Obj, index: usize) -> Vec<String> {
    let ids = split_ids(scene_map_value(refs.get("subject_scene_map"), segment, index).as_ref());
    let subjects = objs(refs.get("subjects"));
    let logical: HashSet<Option<&Value>> = logical_subjects(refs).iter().map(|s| s.get("id")).collect();
    let mut mapped: Vec<String> = Vec::new();
    for subject_id in ids {
        let Some(subject) = subjects.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(subject_id.as_str())) else {
            continue;
        };
        let target = {
            let t = extra_target_id(subject);
            if t.is_empty() { subject.get("id").map(|v| text(Some(v))).unwrap_or_default() } else { t }
        };
        let in_logical = logical.iter().any(|l| l.is_some_and(|v| text(Some(v)) == target));
        if !target.is_empty() && in_logical && !mapped.contains(&target) {
            mapped.push(target);
        }
    }
    mapped
}

fn expand_subject_references(refs: &Obj, subjects: Vec<Obj>) -> Vec<Obj> {
    let selected: HashSet<String> = subjects.iter().map(|s| field(s, "id")).filter(|s| !s.is_empty()).collect();
    let mut expanded = subjects;
    for subject in objs(refs.get("subjects")) {
        if selected.contains(&extra_target_id(subject)) {
            expanded.push(subject.clone());
        }
    }
    let mut seen = HashSet::new();
    expanded
        .into_iter()
        .filter(|s| {
            let id = field(s, "id");
            !id.is_empty() && seen.insert(id)
        })
        .collect()
}

fn primary_subject(refs: &Obj) -> Obj {
    let subjects = objs(refs.get("subjects"));
    let empty = Obj::new();
    let primary = refs.get("subject").and_then(Value::as_object).unwrap_or(&empty);
    let first = subjects.first().copied().unwrap_or(&empty);
    let mut out = first.clone();
    let description = first_of(&[primary.get("description"), first.get("description")]).unwrap_or_else(|| json!(""));
    let image = first_of(&[primary.get("image"), first.get("image")]).unwrap_or_else(|| json!({}));
    out.insert("description".into(), description);
    out.insert("image".into(), image);
    out
}

fn first_of(values: &[Option<&Value>]) -> Option<Value> {
    values.iter().flatten().find(|v| truthy(v)).map(|v| (*v).clone())
}

/// Mirrors `hasMeaningfulSubject`: the UI switches subject references on once a subject has a name, description,
/// trigger or image, so a project built through the API behaves like one built in the UI.
fn has_meaningful_subject(refs: &Obj) -> bool {
    if flag(refs, "cleared") {
        return false;
    }
    let empty = Obj::new();
    let primary = refs.get("subject").and_then(Value::as_object).unwrap_or(&empty);
    let image = primary.get("image").and_then(Value::as_object).unwrap_or(&empty);
    let image_set = |o: &Obj| ["path", "data", "name"].iter().any(|k| !field(o, k).is_empty());
    if !field(primary, "description").is_empty() || image_set(image) {
        return true;
    }
    if refs.get("subject_scene_map").is_some_and(truthy) {
        return true;
    }
    for subject in objs(refs.get("subjects")) {
        let name = field(subject, "name");
        let lowered = name.to_lowercase();
        let placeholder = lowered.starts_with("character ") && name.split_once(' ').is_some_and(|(_, rest)| is_digits(rest));
        let subject_image = subject.get("image").and_then(Value::as_object).unwrap_or(&empty);
        if (!name.is_empty() && !placeholder)
            || !field(subject, "description").is_empty()
            || !field(subject, "trigger_phrase").is_empty()
            || image_set(subject_image)
        {
            return true;
        }
    }
    false
}

pub(crate) fn subject_items_for_segment(refs: &Obj, segment: &Obj, index: usize) -> Vec<Obj> {
    let empty = Obj::new();
    let primary_has_image = has_image(refs.get("subject").and_then(|s| s.get("image")));
    let subjects: Vec<Obj> = objs(refs.get("subjects")).into_iter().cloned().collect();
    if !(flag(refs, "use_subject_reference") || has_meaningful_subject(refs) || primary_has_image) {
        return vec![];
    }
    if flag(segment, "no_character_present") {
        if let Some(backed) = subjects.iter().find(|s| has_image(s.get("image"))) {
            return vec![backed.clone()];
        }
        return if primary_has_image { vec![primary_subject(refs)] } else { vec![] };
    }
    let count = refs.get("subject_count").filter(|v| truthy(v)).and_then(to_float).map_or(subjects.len() as i64, |f| f as i64);
    if count > 1 {
        let ids: HashSet<String> = subject_ids_for_scene(refs, segment, index).into_iter().collect();
        let mapped: Vec<Obj> = subjects
            .iter()
            .filter(|s| s.get("id").is_some_and(|v| ids.contains(&text(Some(v)))))
            .cloned()
            .collect();
        if !mapped.is_empty() {
            return expand_subject_references(refs, mapped);
        }
        let backed: Vec<&Obj> = logical_subjects(refs).into_iter().filter(|s| has_image(s.get("image"))).collect();
        if backed.len() == 1 {
            return vec![backed[0].clone()];
        }
        if backed.is_empty() && primary_has_image {
            return vec![primary_subject(refs)];
        }
        return vec![];
    }
    let subject = primary_subject(refs);
    let _ = empty;
    if has_image(subject.get("image")) || !text(first_truthy(&subject, &["name", "description"])).is_empty() {
        return vec![subject];
    }
    vec![]
}

/// Every reference that has an image, in the order the picker lists them.
pub fn reference_catalog(refs: &Obj) -> Vec<Obj> {
    let mut catalog: Vec<Obj> = Vec::new();
    let mut add = |kind: &str, item: &Obj, fallback: &str| {
        let item_id = field(item, "id");
        let empty = Obj::new();
        let image = item.get("image").and_then(Value::as_object).unwrap_or(&empty);
        if item_id.is_empty() || (field(image, "path").is_empty() && field(image, "data").is_empty()) {
            return;
        }
        let label_source = first_truthy(item, &["name", "title"]).cloned().unwrap_or_else(|| json!(fallback));
        let label = text(Some(&label_source));
        let mut entry = Obj::new();
        entry.insert("key".into(), json!(format!("{kind}:{item_id}")));
        entry.insert("kind".into(), json!(kind));
        entry.insert("source_id".into(), json!(item_id));
        entry.insert("label".into(), json!(if label.is_empty() { "Reference".to_string() } else { label }));
        entry.insert("description".into(), json!(field(item, "description")));
        let image_type = field(item, "reference_image_type");
        entry.insert("reference_image_type".into(), json!(if image_type.is_empty() { "single".to_string() } else { image_type }));
        entry.insert("image".into(), Value::Object(image.clone()));
        catalog.push(entry);
    };
    for subject in objs(refs.get("subjects")) {
        add("subject", subject, "Character reference");
    }
    for extra in objs(refs.get("extra_subjects")) {
        if flag(extra, "send_to_minimax") {
            add("extra", extra, "Extra character reference");
        }
    }
    for location in objs(refs.get("locations")) {
        add("location", location, "Location reference");
    }
    for sheet in objs(refs.get("ingredients_sheets")) {
        add("ingredients", sheet, "Ingredients sheet");
    }
    catalog
}

fn key_of(item: &Obj) -> String {
    field(item, "key")
}

/// Extras mapped to this scene that go to MiniMax.
pub(crate) fn forced_extra_keys(refs: &Obj, segment: &Obj, index: usize) -> Vec<String> {
    if flag(segment, "no_character_present") || !flag(refs, "extras_enabled") {
        return vec![];
    }
    let extras: Vec<(String, &Obj)> = objs(refs.get("extra_subjects")).into_iter().map(|e| (field(e, "id"), e)).collect();
    let entries = scene_map_value(refs.get("extra_scene_map"), segment, index);
    let mut keys: Vec<String> = Vec::new();
    for entry in entries.as_ref().and_then(Value::as_array).into_iter().flatten() {
        let Some(entry) = entry.as_object() else { continue };
        let wanted = text(first_truthy(entry, &["extra_id", "extraId"]));
        let Some((_, extra)) = extras.iter().rev().find(|(id, _)| *id == wanted) else { continue };
        if !field(extra, "description").is_empty() && flag(extra, "send_to_minimax") && has_image(extra.get("image")) {
            let key = format!("extra:{}", field(extra, "id"));
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

fn mapped_keys(refs: &Obj, segment: &Obj, index: usize, catalog_keys: &HashSet<String>) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut add = |key: String| {
        if !key.is_empty() && catalog_keys.contains(&key) && !keys.contains(&key) {
            keys.push(key);
        }
    };
    if !flag(segment, "no_character_present") {
        for subject in subject_items_for_segment(refs, segment, index) {
            add(format!("subject:{}", field(&subject, "id")));
        }
        for key in forced_extra_keys(refs, segment, index) {
            add(key);
        }
    }
    let location_id = text(scene_map_value(refs.get("scene_map"), segment, index).as_ref());
    if !location_id.is_empty() {
        add(format!("location:{location_id}"));
    }
    let sheet_id = segment
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| refs.get("ingredients_scene_map").and_then(Value::as_object).and_then(|m| m.get(id)));
    let sheet_id = text(sheet_id);
    if !sheet_id.is_empty() {
        add(format!("ingredients:{sheet_id}"));
    }
    keys
}

/// Explicit keys else mapped keys, plus forced extras, with no limit.
pub fn all_desired_reference_keys(refs: &Obj, segment: &Obj, index: usize) -> Vec<String> {
    let catalog_keys: HashSet<String> = reference_catalog(refs).iter().map(key_of).collect();
    let selected: Vec<String> = match segment.get("minimax_h3_reference_keys") {
        Some(Value::Array(items)) => items.iter().map(|k| text(Some(k))).collect(),
        _ => mapped_keys(refs, segment, index, &catalog_keys),
    };
    let forced = forced_extra_keys(refs, segment, index);
    let mut seen: HashSet<String> = HashSet::new();
    let mut keys: Vec<String> = Vec::new();
    for key in selected.iter().chain(forced.iter()) {
        if key.is_empty()
            || !catalog_keys.contains(key)
            || (key.starts_with("extra:") && !forced.contains(key))
            || !seen.insert(key.clone())
        {
            continue;
        }
        keys.push(key.clone());
    }
    keys
}

/// The keys above, the first `MAX_SCENE_REFERENCES` of them.
pub fn desired_reference_keys(refs: &Obj, segment: &Obj, index: usize) -> Vec<String> {
    let mut keys = all_desired_reference_keys(refs, segment, index);
    keys.truncate(MAX_SCENE_REFERENCES);
    keys
}

/// The selected history image, else approved, else custom.
pub fn segment_image_path(segment: &Obj) -> String {
    if let Some(history) = segment.get("image_history").and_then(Value::as_array) {
        if !history.is_empty() {
            let wanted = segment.get("image_history_index").filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64);
            let position = wanted.min(history.len() as i64 - 1).max(0) as usize;
            if truthy(&history[position]) {
                return text(Some(&history[position]));
            }
        }
    }
    text(first_truthy(segment, &["approved_image_path", "custom_image_path"]))
}

/// Everything the "Choose MiniMax References" picker shows for one scene.
pub fn reference_choices(session: &Value, segment: &Obj, index: usize, mode: &str) -> Value {
    let refs = builder(session);
    let catalog = reference_catalog(&refs);
    let forced = forced_extra_keys(&refs, segment, index);
    let available: Vec<&Obj> =
        catalog.iter().filter(|i| field(i, "kind") != "extra" || forced.contains(&key_of(i))).collect();
    let catalog_keys: HashSet<String> = catalog.iter().map(key_of).collect();
    let start_reserved =
        mode == "image_reference_to_video" && uses_scene_image_as_start_frame(segment) && !segment_image_path(segment).is_empty();
    let max_choices = (MAX_REFERENCE_IMAGES - usize::from(start_reserved)).min(MAX_SCENE_REFERENCES);
    let mapped = mapped_keys(&refs, segment, index, &catalog_keys);
    let over_limit_by = all_desired_reference_keys(&refs, segment, index).len().saturating_sub(max_choices);
    let mut selected_keys = desired_reference_keys(&refs, segment, index);
    selected_keys.truncate(max_choices);
    let offset = if start_reserved { 2 } else { 1 };
    let position = |key: &str| selected_keys.iter().position(|k| k == key).map(|n| n + offset);
    let describe = |item: &Obj| {
        let key = key_of(item);
        let image = item.get("image").and_then(Value::as_object);
        json!({
            "key": key,
            "kind": item["kind"],
            "source_id": item["source_id"],
            "label": item["label"],
            "description": item["description"],
            "reference_image_type": item["reference_image_type"],
            "image_path": image.map(|i| field(i, "path")).unwrap_or_default(),
            "in_scene_mapping": mapped.contains(&key) || forced.contains(&key),
            "selected": position(&key).is_some(),
            "image_number": position(&key),
        })
    };
    let find = |key: &str| available.iter().find(|i| key_of(i) == key).copied();
    let mut automatic: Vec<String> = Vec::new();
    for key in mapped.iter().chain(forced.iter()) {
        if !automatic.contains(key) && find(key).is_some() {
            automatic.push(key.clone());
        }
    }
    automatic.truncate(max_choices);
    json!({
        "scene_id": field(segment, "id"),
        "scene_number": index + 1,
        "video_mode": mode,
        "custom": segment.get("minimax_h3_reference_keys").is_some_and(Value::is_array),
        "available": available.iter().map(|i| describe(i)).collect::<Vec<_>>(),
        "selected": selected_keys.iter().filter_map(|k| find(k)).map(describe).collect::<Vec<_>>(),
        "automatic_keys": automatic,
        // How many references the scene asks for beyond `max_choices`. The render sends the first ones and leaves these out.
        "over_limit_by": over_limit_by,
        "limits": {
            "max_images": MAX_REFERENCE_IMAGES,
            "start_frame_image_1": start_reserved,
            "max_choices": max_choices,
        },
    })
}

/// Start frame first, then mapped references, at most nine, one per image path.
pub fn ordered_reference_items(session: &Value, segment: &Obj, mode: &str, index: usize) -> Vec<Obj> {
    let refs = builder(session);
    let mut ordered: Vec<Obj> = Vec::new();
    if mode == "image_reference_to_video" && uses_scene_image_as_start_frame(segment) {
        let start = segment_image_path(segment);
        if !start.is_empty() {
            let entry = json!({"key": "scene:start_frame", "kind": "start_frame", "label": "Scene start frame", "image": {"path": start}});
            if let Value::Object(o) = entry {
                ordered.push(o);
            }
        }
    }
    let catalog = reference_catalog(&refs);
    for key in desired_reference_keys(&refs, segment, index) {
        if let Some(item) = catalog.iter().find(|i| key_of(i) == key) {
            ordered.push(item.clone());
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<Obj> = Vec::new();
    for item in ordered {
        let path = item.get("image").and_then(Value::as_object).map(|i| field(i, "path")).unwrap_or_default();
        let fingerprint = if path.is_empty() { String::new() } else { format!("path:{}", key_of_text(&path)) };
        if !fingerprint.is_empty() && seen.insert(fingerprint) {
            result.push(item);
        }
    }
    result.truncate(MAX_REFERENCE_IMAGES);
    result
}

fn normalize_paths(value: &[Value]) -> Vec<String> {
    value
        .iter()
        .map(|item| match item {
            Value::Object(o) => text(first_truthy(o, &["path", "file"])),
            other => text(Some(other)),
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// Image paths a render sends, in order. `configured` replaces the scene's own saved paths.
pub fn render_reference_image_paths(
    session: &Value,
    segment: &Obj,
    mode: &str,
    index: usize,
    configured: Option<&[Value]>,
) -> Vec<String> {
    let uses_builder = REFERENCE_BUILDER_MODES.contains(&mode);
    let mut builder_paths: Vec<String> = Vec::new();
    if uses_builder && configured.is_none() {
        builder_paths = ordered_reference_items(session, segment, mode, index)
            .iter()
            .map(|item| item.get("image").and_then(Value::as_object).map(|i| field(i, "path")).unwrap_or_default())
            .filter(|p| !p.is_empty())
            .take(MAX_REFERENCE_IMAGES)
            .collect();
    }
    let saved = first_truthy(segment, &["minimax_h3_image_paths", "minimax_image_paths"]).and_then(Value::as_array);
    let extra_paths = match configured {
        Some(list) => normalize_paths(list),
        None => saved.map(|l| normalize_paths(l)).unwrap_or_default(),
    };
    let mut paths: Vec<String> = Vec::new();
    if uses_builder {
        let mut seen: HashSet<String> = HashSet::new();
        for path in builder_paths.into_iter().chain(extra_paths) {
            let key = key_of_text(&path);
            if !key.is_empty() && seen.insert(key) {
                paths.push(path);
            }
        }
        paths.truncate(MAX_REFERENCE_IMAGES);
    }
    if mode == "image_to_video" || mode == "image_reference_to_video" {
        let selected = segment_image_path(segment);
        if !selected.is_empty() {
            let selected_key = key_of_text(&selected);
            let mut out = vec![selected];
            out.extend(paths.into_iter().filter(|p| key_of_text(p) != selected_key));
            out.truncate(MAX_REFERENCE_IMAGES);
            paths = out;
        }
    }
    paths
}

/// Reference videos, only for `video_to_video`.
pub fn video_references_for_scene(segment: &Obj, mode: &str, configured: Option<&[Value]>) -> Vec<Value> {
    let raw: Option<Vec<Value>> = match configured {
        Some(list) => Some(list.to_vec()),
        None => first_truthy(segment, &["minimax_h3_video_references", "minimax_video_references"])
            .map(|v| v.as_array().cloned().unwrap_or_default()),
    };
    if mode == "video_to_video" {
        raw.unwrap_or_default()
    } else {
        vec![]
    }
}

/// Image to Video can end on an explicit last frame.
pub fn last_frame_path_for_scene(segment: &Obj, mode: &str) -> String {
    if mode != "image_to_video" || segment.get("minimax_h3_i2v_frame_mode").and_then(Value::as_str) == Some("normal") {
        return String::new();
    }
    field(segment, "first_last_frame_end_image_path")
}

/// Scene specific inputs of a render besides settings, prompt and timing.
pub struct SceneInputsArgs<'a> {
    pub continuity_mode: &'a Value,
    pub has_previous_segment: bool,
    pub scene_number: i64,
    pub configured_image_paths: Option<&'a [Value]>,
    pub configured_video_references: Option<&'a [Value]>,
    pub render_pass: &'a str,
}

/// Resolve one scene's inputs. `is_file` checks a path on disk. Errors use the browser's wording.
pub fn resolve_scene_inputs(
    session: &Value,
    segment: &Obj,
    mode: &str,
    scene_index: usize,
    args: &SceneInputsArgs,
    is_file: &dyn Fn(&str) -> bool,
) -> Result<Obj, CoreError> {
    let mut continuity = normalize_continuity_mode(Some(args.continuity_mode));
    if !continuity_allowed_for_mode(continuity, mode, args.render_pass) {
        continuity = "off";
    }
    let image_paths = render_reference_image_paths(session, segment, mode, scene_index, args.configured_image_paths);
    if continuity != "off" && args.has_previous_segment && continuity == MASKED && args.scene_number <= 1 {
        return Err(CoreError::value(
            "Scene 1 cannot use Latent Continuation Masked because there is no predecessor scene. Set continuity to off.",
        ));
    }
    let videos = video_references_for_scene(segment, mode, args.configured_video_references);
    if (mode == "image_to_video" || mode == "image_reference_to_video") && image_paths.is_empty() {
        return Err(CoreError::value("This scene needs a selected scene image for MiniMax Image to Video."));
    }
    if mode == "reference_to_video" && image_paths.is_empty() {
        return Err(CoreError::value("This scene needs at least one ordered Reference Builder image."));
    }
    if mode == "video_to_video"
        && !videos.iter().filter_map(Value::as_object).any(|v| !field(v, "path").is_empty())
    {
        return Err(CoreError::value("This scene needs at least one reference video path."));
    }
    let mut missing: Vec<String> = image_paths.iter().filter(|p| !is_file(p)).cloned().collect();
    let last_frame = last_frame_path_for_scene(segment, mode);
    if mode == "image_to_video"
        && segment.get("minimax_h3_i2v_frame_mode").and_then(Value::as_str) == Some("flf")
        && last_frame.is_empty()
    {
        return Err(CoreError::value("First\u{2013}Last Frame needs a saved last-frame image. Choose or upload it before rendering."));
    }
    let mut result = Obj::new();
    result.insert("continuity_mode".into(), json!(continuity));
    result.insert("continuity_image_number".into(), json!(0));
    if !last_frame.is_empty() && !is_file(&last_frame) {
        missing.push(last_frame.clone());
    }
    result.insert("missing_image_paths".into(), json!(missing));
    result.insert("image_paths".into(), json!(image_paths));
    result.insert("video_references".into(), Value::Array(videos));
    if !last_frame.is_empty() {
        result.insert("last_frame_path".into(), json!(last_frame));
    }
    Ok(result)
}

/// The cleaned list of reference keys to save for a scene, or what is wrong and what is allowed. Port of
/// `validate_reference_keys` in `minimax/scene_inputs.py`.
pub fn validate_reference_keys(choices: &Value, keys: &Value) -> Result<Vec<String>, String> {
    let Some(list) = keys.as_array() else {
        return Err("`keys` must be a list of reference keys such as \"location:abc\". Use `automatic: true` to follow the scene mappings.".into());
    };
    let cleaned: Vec<String> = list.iter().map(|k| text(Some(k)).trim().to_string()).collect();
    if cleaned.iter().any(String::is_empty) {
        return Err("`keys` must not contain empty values.".into());
    }
    let mut duplicates: Vec<&String> = cleaned.iter().filter(|k| cleaned.iter().filter(|o| o == k).count() > 1).collect();
    duplicates.sort();
    duplicates.dedup();
    if !duplicates.is_empty() {
        return Err(format!("Each reference can be listed once. Repeated: {}.", duplicates.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let allowed: Vec<String> = choices["available"].as_array().map(|a| a.iter().map(|i| text(i.get("key"))).collect()).unwrap_or_default();
    let unknown: Vec<&String> = cleaned.iter().filter(|k| !allowed.contains(k)).collect();
    if !unknown.is_empty() {
        let shown = if allowed.is_empty() { "(none, add reference images in the Reference Builder first)".to_string() } else { allowed.join(", ") };
        return Err(format!("Not available for this scene: {}. Available keys: {shown}.", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let limit = choices["limits"]["max_choices"].as_i64().unwrap_or(0) as usize;
    if cleaned.len() > limit {
        let reserved = if choices["limits"]["start_frame_image_1"].as_bool().unwrap_or(false) { " (Image 1 is the scene's start frame)" } else { "" };
        return Err(format!("MiniMax takes at most {limit} chosen references for this scene{reserved}; {} were given.", cleaned.len()));
    }
    Ok(cleaned)
}

/// The characters and the location mapped to a scene, as cards. Unlike the reference lists, a card without a picture is
/// still returned.
pub fn scene_cast_and_location(session: &Value, segment: &Obj, index: usize) -> (Vec<Obj>, Option<Obj>) {
    let refs = builder(session);
    let cast = subject_items_for_segment(&refs, segment, index);
    let location = if refs.get("locations_cleared").is_some_and(truthy) {
        None
    } else {
        let wanted = scene_map_value(refs.get("scene_map"), segment, index).map(|v| text(Some(&v))).unwrap_or_default();
        if wanted.is_empty() {
            None
        } else {
            objs(refs.get("locations")).into_iter().find(|l| text(l.get("id")) == wanted).cloned()
        }
    };
    (cast, location)
}
