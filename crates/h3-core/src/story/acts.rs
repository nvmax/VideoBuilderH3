//! Story Mode acts and scene plan rules (pure code, no LLM). Port of `storyboard/story_acts.py`.
//!
//! A chapter owns a contiguous run of timeline scenes. Its acts split that run into contiguous pieces, and every scene belongs to
//! exactly one act. Scene numbers here are 1-based timeline positions.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::pyval::{text_or_empty, truthy, Obj};

pub use super::mode::DEFAULT_SPOT_MIN_SCENES;

/// Scenes per chapter, proportional to each chapter's paragraphs (largest remainder), at least 1 each. With more chapters than scenes
/// the last chapters get 0 scenes.
pub fn allocate_chapter_scenes(paragraph_counts: &[i64], scene_count: i64) -> Vec<i64> {
    let weights: Vec<i64> = paragraph_counts.iter().map(|c| (*c).max(1)).collect();
    if weights.is_empty() || scene_count <= 0 {
        return vec![0; weights.len()];
    }
    if weights.len() as i64 >= scene_count {
        let mut counts = vec![1; scene_count as usize];
        counts.extend(vec![0; weights.len() - scene_count as usize]);
        return counts;
    }
    let spare = scene_count - weights.len() as i64;
    let total: i64 = weights.iter().sum();
    let exact: Vec<f64> = weights.iter().map(|w| (spare * w) as f64 / total as f64).collect();
    let mut counts: Vec<i64> = exact.iter().map(|v| 1 + v.trunc() as i64).collect();
    let left = scene_count - counts.iter().sum::<i64>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|a, b| {
        let (fa, fb) = (-(exact[*a] - exact[*a].trunc()), -(exact[*b] - exact[*b].trunc()));
        fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b))
    });
    for index in order.into_iter().take(left.max(0) as usize) {
        counts[index] += 1;
    }
    counts
}

/// `[(first, last)]` 1-based scene ranges for chapter scene counts. `None` for a chapter with no scenes.
pub fn chapter_ranges(counts: &[i64]) -> Vec<Option<(i64, i64)>> {
    let mut ranges = Vec::new();
    let mut start = 1;
    for count in counts {
        if *count <= 0 {
            ranges.push(None);
            continue;
        }
        ranges.push(Some((start, start + count - 1)));
        start += count;
    }
    ranges
}

/// Contiguous act ranges covering `first..last` built from the starts the LLM proposed (`None` for one that is not a number).
/// Proposals that start outside the range are dropped, the rest are sorted. Each act starts where its proposal starts, the first act
/// starts at `first` and every act ends where the next begins. Nothing usable gives one act.
pub fn repair_act_ranges(proposed: &[Option<i64>], first: i64, last: i64) -> Vec<(i64, i64)> {
    let mut starts: Vec<i64> = Vec::new();
    for start in proposed.iter().flatten() {
        if first <= *start && *start <= last && !starts.contains(start) {
            starts.push(*start);
        }
    }
    starts.sort_unstable();
    if starts.first() != Some(&first) {
        starts.insert(0, first);
    }
    starts.iter().enumerate().map(|(i, start)| (*start, if i + 1 < starts.len() { starts[i + 1] - 1 } else { last })).collect()
}

/// Move act borders so no timed note range is cut in two. Borders move to the nearer edge of the note. A note longer than the whole
/// chapter is left alone.
pub fn keep_notes_inside_acts(ranges: &[(i64, i64)], note_ranges: &[(i64, i64)]) -> Vec<(i64, i64)> {
    if ranges.len() < 2 {
        return ranges.to_vec();
    }
    let (first, last) = (ranges[0].0, ranges[ranges.len() - 1].1);
    let mut starts: Vec<i64> = ranges[1..].iter().map(|r| r.0).collect();
    for (note_first, note_last) in note_ranges {
        if *note_first <= first && *note_last >= last {
            continue;
        }
        starts = starts
            .iter()
            .map(|start| {
                if *note_first < *start && *start <= *note_last {
                    let (before, after) = (*note_first, *note_last + 1);
                    if start - before <= after - start { before } else { after }
                } else {
                    *start
                }
            })
            .collect();
    }
    let mut unique: Vec<i64> = starts.into_iter().filter(|s| first < *s && *s <= last).collect();
    unique.sort_unstable();
    unique.dedup();
    let mut bounds = vec![first];
    bounds.extend(unique);
    bounds.iter().enumerate().map(|(i, start)| (*start, if i + 1 < bounds.len() { bounds[i + 1] - 1 } else { last })).collect()
}

/// One act per paragraph when the LLM gives nothing usable, scenes shared out evenly.
pub fn fallback_act_ranges(paragraph_count: i64, first: i64, last: i64) -> Vec<(i64, i64)> {
    let scene_total = last - first + 1;
    let count = paragraph_count.max(1).min(scene_total).max(1);
    let sizes = allocate_chapter_scenes(&vec![1; count as usize], scene_total);
    chapter_ranges(&sizes).into_iter().flatten().map(|(a, b)| (first + a - 1, first + b - 1)).collect()
}

fn spot_runs(spots: &[String]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = 0;
    for index in 1..=spots.len() {
        if index == spots.len() || spots[index].to_lowercase() != spots[start].to_lowercase() {
            runs.push((start, index - 1));
            start = index;
        }
    }
    runs
}

/// Spots with every run at least `min_run` scenes long. Blank spots take the spot before them. The shortest short run is merged into
/// its longer neighbour (the one before it on a tie), again until no short run is left. Runs holding a scene in `locked` (indexes whose
/// spot came from a user note) are kept.
pub fn enforce_spot_runs(spots: &[String], min_run: i64, locked: &HashSet<usize>) -> Vec<String> {
    let mut result: Vec<String> = spots.iter().map(|s| s.trim().to_string()).collect();
    if result.is_empty() {
        return result;
    }
    let min_run = min_run.max(1) as usize;
    if result[0].is_empty() {
        result[0] = result.iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
    }
    for index in 1..result.len() {
        if result[index].is_empty() {
            result[index] = result[index - 1].clone();
        }
    }
    loop {
        let runs = spot_runs(&result);
        let short: Vec<(usize, usize)> = runs.iter().enumerate().filter(|(_, (a, b))| b - a + 1 < min_run && !(*a..=*b).any(|i| locked.contains(&i))).map(|(position, (a, b))| (b - a + 1, position)).collect();
        if runs.len() < 2 || short.is_empty() {
            return result;
        }
        let (_, position) = *short.iter().min().expect("not empty");
        let (a, b) = runs[position];
        let before = if position > 0 { Some(runs[position - 1]) } else { None };
        let after = runs.get(position + 1).copied();
        let size = |run: Option<(usize, usize)>| run.map_or(-1, |(x, y)| (y - x + 1) as i64);
        let target = if size(before) >= size(after) { before } else { after }.expect("a neighbour exists");
        let name = result[target.0].clone();
        for slot in result.iter_mut().take(b + 1).skip(a) {
            *slot = name.clone();
        }
    }
}

fn scene_ids_of(act: &Value) -> Vec<Value> {
    match act.get("scene_ids") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn id_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => text_or_empty(Some(other)),
    }
}

pub fn act_scene_numbers(act: &Value, scene_number_by_id: &HashMap<String, i64>) -> Vec<i64> {
    let mut numbers: Vec<i64> = scene_ids_of(act).iter().filter_map(|sid| scene_number_by_id.get(&id_text(sid)).copied()).collect();
    numbers.sort_unstable();
    numbers
}

/// Problems with an edited act list, as plain sentences. Empty means the acts are valid. Rules: every scene is in exactly one act, each
/// act's scenes are contiguous, acts are in timeline order, every act belongs to a chapter of the book, and chapters keep book order
/// along the timeline.
pub fn validate_story_acts(acts: &[Value], chapters: &[Value], scene_ids: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let number_by_id: HashMap<String, i64> = scene_ids.iter().enumerate().map(|(i, s)| (s.clone(), i as i64 + 1)).collect();
    let chapter_order: HashMap<String, usize> = chapters.iter().enumerate().map(|(i, c)| (id_text(&c["id"]), i)).collect();
    let mut seen: HashMap<String, String> = HashMap::new();
    let (mut previous_last, mut previous_chapter) = (0i64, -1i64);
    for act in acts {
        let label = [act.get("title"), act.get("id")].into_iter().flatten().find(|v| truthy(v)).map(id_text).unwrap_or_else(|| "An act".into());
        let order = match act.get("chapter_id") {
            Some(Value::String(id)) => chapter_order.get(id).copied(),
            _ => None,
        };
        let Some(order) = order else {
            problems.push(format!("{label} belongs to no chapter of the story."));
            continue;
        };
        let unknown: Vec<String> = scene_ids_of(act).iter().map(id_text).filter(|sid| !number_by_id.contains_key(sid)).collect();
        if !unknown.is_empty() {
            problems.push(format!("{label} lists scenes that are not on the timeline: {}.", unknown.join(", ")));
        }
        let numbers = act_scene_numbers(act, &number_by_id);
        if numbers.is_empty() {
            problems.push(format!("{label} has no scenes."));
            continue;
        }
        for sid in scene_ids_of(act).iter().map(id_text) {
            if let Some(earlier) = seen.get(&sid) {
                let shown = number_by_id.get(&sid).map_or_else(|| sid.clone(), |n| n.to_string());
                problems.push(format!("Scene {shown} is in both {earlier} and {label}."));
            }
            seen.insert(sid, label.clone());
        }
        let (low, high) = (numbers[0], numbers[numbers.len() - 1]);
        if numbers != (low..=high).collect::<Vec<i64>>() {
            problems.push(format!("{label} skips scenes; an act's scenes must be next to each other."));
        }
        if low <= previous_last {
            problems.push(format!("{label} is out of timeline order."));
        }
        if (order as i64) < previous_chapter {
            problems.push(format!("{label} puts its chapter before an earlier chapter on the timeline."));
        }
        previous_last = high;
        previous_chapter = order as i64;
    }
    let missing: Vec<String> = scene_ids.iter().filter(|sid| !seen.contains_key(*sid)).map(|sid| number_by_id[sid].to_string()).collect();
    if !missing.is_empty() {
        problems.push(format!("Scenes not in any act: {}.", missing.join(", ")));
    }
    problems
}

// -- edit operations (the Acts tab) ------------------------------------------------------------------------------------------

fn renumber(acts: Vec<Value>) -> Vec<Value> {
    acts.into_iter()
        .enumerate()
        .map(|(index, act)| {
            let mut map: Obj = act.as_object().cloned().unwrap_or_default();
            map.insert("id".into(), json!(format!("act_{}", index + 1)));
            Value::Object(map)
        })
        .collect()
}

fn with(act: &Value, changes: &[(&str, Value)]) -> Value {
    let mut map: Obj = act.as_object().cloned().unwrap_or_default();
    for (key, value) in changes {
        map.insert((*key).into(), value.clone());
    }
    Value::Object(map)
}

fn texts(items: &[Value]) -> Vec<String> {
    items.iter().map(id_text).collect()
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| json!(s)).collect())
}

/// Move the border between act `act_index - 1` and `act_index` so act `act_index` starts at the scene. Both acts keep at least one scene
/// and must be in the same chapter. Otherwise the acts are returned unchanged.
pub fn move_act_border(acts: &[Value], act_index: i64, new_first_scene_id: &str, scene_ids: &[String]) -> Vec<Value> {
    if act_index <= 0 || act_index as usize >= acts.len() || !scene_ids.iter().any(|s| s == new_first_scene_id) {
        return acts.to_vec();
    }
    let index = act_index as usize;
    let (before, after) = (&acts[index - 1], &acts[index]);
    if before.get("chapter_id") != after.get("chapter_id") {
        return acts.to_vec();
    }
    let mut joined = texts(&scene_ids_of(before));
    joined.extend(texts(&scene_ids_of(after)));
    let Some(cut) = joined.iter().position(|s| s == new_first_scene_id) else { return acts.to_vec() };
    if cut == 0 || cut >= joined.len() {
        return acts.to_vec();
    }
    let mut result = acts.to_vec();
    result[index - 1] = with(before, &[("scene_ids", strings(&joined[..cut]))]);
    result[index] = with(after, &[("scene_ids", strings(&joined[cut..]))]);
    result
}

/// Merge act `act_index` into the act before it when both are in the same chapter.
pub fn merge_acts(acts: &[Value], act_index: i64) -> Vec<Value> {
    if act_index <= 0 || act_index as usize >= acts.len() {
        return acts.to_vec();
    }
    let index = act_index as usize;
    let (before, after) = (&acts[index - 1], &acts[index]);
    if before.get("chapter_id") != after.get("chapter_id") {
        return acts.to_vec();
    }
    let summary: Vec<String> = [before.get("summary"), after.get("summary")].into_iter().flatten().filter(|v| truthy(v)).map(id_text).collect();
    let mut cast: Vec<String> = Vec::new();
    for id in texts(&cast_ids_of(before)).into_iter().chain(texts(&cast_ids_of(after))) {
        if !cast.contains(&id) {
            cast.push(id);
        }
    }
    let mut scenes = texts(&scene_ids_of(before));
    scenes.extend(texts(&scene_ids_of(after)));
    let merged = with(before, &[("summary", json!(summary.join(" "))), ("cast_ids", strings(&cast)), ("scene_ids", strings(&scenes))]);
    let mut result: Vec<Value> = acts[..index - 1].to_vec();
    result.push(merged);
    result.extend_from_slice(&acts[index + 1..]);
    renumber(result)
}

fn cast_ids_of(act: &Value) -> Vec<Value> {
    match act.get("cast_ids") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// Split an act in two. The new act starts at the given scene. Both parts keep at least one scene.
pub fn split_act(acts: &[Value], act_index: i64, first_scene_id_of_new_act: &str) -> Vec<Value> {
    if act_index < 0 || act_index as usize >= acts.len() {
        return acts.to_vec();
    }
    let index = act_index as usize;
    let act = &acts[index];
    let scene_ids = texts(&scene_ids_of(act));
    let Some(cut) = scene_ids.iter().position(|s| s == first_scene_id_of_new_act) else { return acts.to_vec() };
    if cut == 0 {
        return acts.to_vec();
    }
    let first = with(act, &[("scene_ids", strings(&scene_ids[..cut]))]);
    let title = Some(text_or_empty(act.get("title"))).filter(|t| !t.is_empty()).unwrap_or_else(|| "Act".into());
    let second = with(act, &[("title", json!(format!("{title} (continued)"))), ("scene_ids", strings(&scene_ids[cut..]))]);
    let mut result: Vec<Value> = acts[..index].to_vec();
    result.push(first);
    result.push(second);
    result.extend_from_slice(&acts[index + 1..]);
    renumber(result)
}

/// `str(value or "")` on one line, cut at `limit` characters.
pub fn clean_name(value: &str, limit: usize) -> String {
    super::mode::line_s(value, limit * 4).chars().take(limit).collect()
}
