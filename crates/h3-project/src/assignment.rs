//! Assign saved characters and locations to scenes with a pattern ("Assign Scenes"). Port of `plan_scene_assignment` and
//! `assign_scenes` in `agent_api/orchestrator/reference_orchestrator.py`. The random pattern uses its own generator, so a
//! seed gives the same plan every time here but not the plan Python would give.

use std::collections::HashSet;
use std::path::Path;

use h3_core::pyval::{py_str, truthy, Obj};
use serde_json::{json, Value};

use crate::layout::Stamp;
use crate::scene_ops::{open, OpError, OpResult};
use crate::session_ops::reference_map;

pub const PATTERNS: [&str; 4] = ["random", "rotate", "blocks", "unchanged"];

#[derive(Debug, Clone)]
pub struct AssignParams {
    pub scope: String,
    pub range_start: i64,
    pub range_end: Option<i64>,
    pub scene_ids: Vec<String>,
    pub character_pattern: String,
    pub character_block_size: i64,
    pub location_pattern: String,
    pub location_block_size: i64,
    pub replace_existing: bool,
    pub avoid_location_repeat: bool,
    pub seed: Option<u64>,
}

impl Default for AssignParams {
    fn default() -> Self {
        Self {
            scope: "all".into(),
            range_start: 1,
            range_end: None,
            scene_ids: vec![],
            character_pattern: "unchanged".into(),
            character_block_size: 10,
            location_pattern: "unchanged".into(),
            location_block_size: 4,
            replace_existing: false,
            avoid_location_repeat: true,
            seed: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanItem {
    pub scene_id: Value,
    pub scene_number: usize,
    pub subject_ids: Vec<String>,
    pub location_id: String,
}

impl PlanItem {
    pub fn to_json(&self) -> Value {
        json!({"scene_id": self.scene_id, "scene_number": self.scene_number, "subject_ids": self.subject_ids, "location_id": self.location_id})
    }
}

/// Small xorshift generator for the random pattern.
struct Rng(u64);

impl Rng {
    fn new(seed: Option<u64>) -> Self {
        let start = seed.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64));
        Rng(start.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// `str(value or "")`.
fn text(value: Option<&Value>) -> String {
    match value {
        Some(v) if truthy(v) => py_str(Some(v)),
        _ => String::new(),
    }
}

fn card_id(card: &Value) -> String {
    text(card.get("id"))
}

fn extra_target(subject: &Value) -> String {
    let first = text(subject.get("extra_reference_for"));
    if first.trim().is_empty() { text(subject.get("same_subject_as")).trim().to_string() } else { first.trim().to_string() }
}

/// Characters that can be mapped to scenes. Extra views of a character are not separate people.
pub fn logical_subjects(refs: Option<&Value>) -> Vec<Value> {
    list_of(refs, "subjects").into_iter().filter(|s| s.is_object() && extra_target(s).is_empty()).collect()
}

fn list_of(refs: Option<&Value>, key: &str) -> Vec<Value> {
    refs.and_then(|r| r.get(key)).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn pick<'a>(rng: &mut Rng, items: &'a [Value], previous: &str, avoid: bool) -> Option<&'a Value> {
    let pool: Vec<&Value> = if avoid && items.len() > 1 { items.iter().filter(|i| card_id(i) != previous).collect() } else { items.iter().collect() };
    if pool.is_empty() { None } else { Some(pool[rng.below(pool.len())]) }
}

/// One entry per target scene: the characters and location it would get.
pub fn plan_scene_assignment(session: &Obj, params: &AssignParams) -> Result<Vec<PlanItem>, OpError> {
    if !PATTERNS.contains(&params.character_pattern.as_str()) || !PATTERNS.contains(&params.location_pattern.as_str()) {
        return Err(OpError::Validation(format!("Patterns must be one of: {}.", PATTERNS.join(", "))));
    }
    let mut rng = Rng::new(params.seed);
    let refs = session.get("flux_reference_builder");
    let scenes: Vec<&Value> = session.get("segments").and_then(Value::as_array).map(|a| a.iter().filter(|s| s.is_object()).collect()).unwrap_or_default();
    let subjects = logical_subjects(refs);
    let locations: Vec<Value> = list_of(refs, "locations").into_iter().filter(Value::is_object).collect();
    let count = scenes.len() as i64;

    let targets: Vec<(&Value, usize)> = match params.scope.as_str() {
        "range" => {
            let start = 1.max(count.min(if params.range_start != 0 { params.range_start } else { 1 }));
            let end = start.max(count.min(params.range_end.filter(|e| *e != 0).unwrap_or(count)));
            scenes.iter().enumerate().filter(|(i, _)| (start..=end).contains(&(*i as i64 + 1))).map(|(i, s)| (*s, i)).collect()
        }
        "selected" | "scene_ids" => {
            if params.scene_ids.is_empty() {
                return Err(OpError::Validation("scene_ids is required when scope is 'selected'.".into()));
            }
            let wanted: HashSet<&str> = params.scene_ids.iter().map(String::as_str).collect();
            scenes.iter().enumerate().filter(|(i, s)| wanted.contains(py_str(s.get("id")).as_str()) || wanted.contains((i + 1).to_string().as_str())).map(|(i, s)| (*s, i)).collect()
        }
        _ => scenes.iter().enumerate().map(|(i, s)| (*s, i)).collect(),
    };

    let empty = Obj::new();
    let subject_map = refs.and_then(|r| r.get("subject_scene_map")).and_then(Value::as_object).unwrap_or(&empty);
    let location_map = refs.and_then(|r| r.get("scene_map")).and_then(Value::as_object).unwrap_or(&empty);
    let char_block = params.character_block_size.max(1) as usize;
    let loc_block = params.location_block_size.max(1) as usize;
    let mut previous_location = String::new();
    let mut plan = Vec::new();
    for (target_index, (scene, index)) in targets.into_iter().enumerate() {
        let scene_key = text(scene.get("id"));
        let stored = subject_map.get(&scene_key);
        let existing_subjects: Vec<String> = match stored {
            Some(Value::Array(items)) => items.iter().map(|x| py_str(Some(x))).filter(|x| !x.trim().is_empty()).collect(),
            other => text(other).split(',').filter(|x| !x.trim().is_empty()).map(str::to_string).collect(),
        };
        let existing_location = text(location_map.get(&scene_key)).trim().to_string();
        let no_character = scene.get("no_character_present").is_some_and(truthy);
        let (mut subject_id, mut location_id) = (String::new(), String::new());
        if !no_character && params.character_pattern != "unchanged" && !subjects.is_empty() {
            let chosen = match params.character_pattern.as_str() {
                "random" => pick(&mut rng, &subjects, "", false).map(card_id).unwrap_or_default(),
                "rotate" => card_id(&subjects[target_index % subjects.len()]),
                _ => card_id(&subjects[(target_index / char_block) % subjects.len()]),
            };
            subject_id = chosen;
        }
        if params.location_pattern != "unchanged" && !locations.is_empty() {
            let chosen = match params.location_pattern.as_str() {
                "rotate" => Some(&locations[target_index % locations.len()]),
                "blocks" => Some(&locations[(target_index / loc_block) % locations.len()]),
                _ => pick(&mut rng, &locations, &previous_location, params.avoid_location_repeat),
            };
            location_id = chosen.map(card_id).unwrap_or_default();
            if !location_id.is_empty() {
                previous_location = location_id.clone();
            }
        }
        let final_subjects = if no_character {
            vec![]
        } else if params.character_pattern == "unchanged" || (!params.replace_existing && !existing_subjects.is_empty()) {
            existing_subjects
        } else if subject_id.is_empty() {
            vec![]
        } else {
            vec![subject_id]
        };
        let final_location = if params.location_pattern == "unchanged" || (!params.replace_existing && !existing_location.is_empty()) { existing_location } else { location_id };
        plan.push(PlanItem { scene_id: scene.get("id").cloned().unwrap_or(Value::Null), scene_number: index + 1, subject_ids: final_subjects, location_id: final_location });
    }
    Ok(plan)
}

/// Plan the assignment and, unless `dry_run`, save it into the scene maps.
pub fn assign_scenes(project: &Path, params: &AssignParams, dry_run: bool, if_match: Option<i64>, now: &Stamp) -> OpResult {
    let mut edit = open(project, if_match)?;
    let plan = plan_scene_assignment(&edit.session, params)?;
    let mut revision = Value::Null;
    if !dry_run {
        if params.character_pattern != "unchanged" {
            let map = reference_map(&mut edit.session, "subjects");
            for item in &plan {
                let key = text(Some(&item.scene_id));
                if item.subject_ids.is_empty() { map.shift_remove(&key); } else { map.insert(key, json!(item.subject_ids)); }
            }
        }
        if params.location_pattern != "unchanged" {
            let map = reference_map(&mut edit.session, "locations");
            for item in &plan {
                let key = text(Some(&item.scene_id));
                if item.location_id.is_empty() { map.shift_remove(&key); } else { map.insert(key, json!(item.location_id)); }
            }
        }
        let refs = edit.session.entry("flux_reference_builder").or_insert_with(|| json!({}));
        let (has_subjects, has_locations) = (!logical_subjects(Some(refs)).is_empty(), !list_of(Some(refs), "locations").is_empty());
        refs["use_subject_reference"] = json!(has_subjects);
        refs["use_location_references"] = json!(has_locations);
    }
    let names: Vec<(String, String)> = list_of(edit.session.get("flux_reference_builder"), "locations").iter().map(|l| (card_id(l), text(l.get("name")))).collect();
    let mut location_use = Obj::new();
    for (id, name) in &names {
        let used = plan.iter().filter(|i| &i.location_id == id).count();
        if used > 0 {
            location_use.insert(name.clone(), json!(used));
        }
    }
    if !dry_run {
        revision = json!(edit.save(now)?);
    }
    Ok(json!({"dry_run": dry_run, "scenes_assigned": plan.len(), "location_use": location_use, "plan": plan.iter().map(PlanItem::to_json).collect::<Vec<_>>(), "revision": revision}))
}
