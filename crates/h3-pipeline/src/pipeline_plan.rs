//! What a full pipeline run would do, and how a song is cut into scenes. Port of `get_pipeline_plan`,
//! `_filter_target_scenes` and `plan_scene_boundaries` in `agent_api/orchestrator/pipeline_orchestrator.py`.

use std::path::Path;

use h3_core::pyval::{py_float_str, py_str, round_to, text_or_empty, to_float, to_int, truthy, Obj};
use serde_json::{json, Value};

use crate::stitch_plan::session_audio_path;

pub const DEFAULT_SCENE_SECONDS: f64 = 4.0;
pub const MIN_SCENE_SECONDS: f64 = 1.0;

/// `(index, scene id, scene)` for the scenes a pipeline touches. Explicit ids win over the scope.
pub fn filter_target_scenes(segments: &[Value], scope: &str, scene_ids: Option<&[String]>) -> Vec<(usize, String, Value)> {
    let items: Vec<(usize, String, Value)> = segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let id = seg.get("id").filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_else(|| (i + 1).to_string());
            (i, id, seg.clone())
        })
        .collect();
    if items.is_empty() {
        return items;
    }
    if let Some(ids) = scene_ids.filter(|ids| ids.iter().any(|s| !s.trim().is_empty())) {
        let wanted: Vec<&str> = ids.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        return items.into_iter().filter(|(i, id, _)| wanted.contains(&id.as_str()) || wanted.contains(&(i + 1).to_string().as_str())).collect();
    }
    match scope.trim().to_lowercase().as_str() {
        "selected" => {
            let selected: Vec<_> = items.iter().filter(|(_, _, s)| s.get("selected").is_some_and(truthy)).cloned().collect();
            if selected.is_empty() { vec![items[0].clone()] } else { selected }
        }
        "from_selected" => {
            let first = items.iter().position(|(_, _, s)| s.get("selected").is_some_and(truthy)).unwrap_or(0);
            items[first..].to_vec()
        }
        _ => items,
    }
}

/// Cut `duration` seconds into equal scenes of about `scene_seconds`. The last scene absorbs a remainder shorter than the
/// minimum scene length.
pub fn plan_scene_boundaries(duration: f64, scene_seconds: f64) -> Vec<(f64, f64)> {
    let total = duration.max(0.0);
    if total <= 0.0 || !total.is_finite() {
        return vec![];
    }
    let length = MIN_SCENE_SECONDS.max(if scene_seconds == 0.0 { DEFAULT_SCENE_SECONDS } else { scene_seconds });
    let mut count = ((total / length).floor() as i64).max(1);
    if total - count as f64 * length >= MIN_SCENE_SECONDS {
        count += 1;
    }
    let step = total / count as f64;
    (0..count)
        .map(|i| (round_to(i as f64 * step, 3), round_to(if i == count - 1 { total } else { (i + 1) as f64 * step }, 3)))
        .collect()
}

fn first_text(values: &[Option<&Value>]) -> String {
    values.iter().flatten().find(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default()
}

fn num(seg: &Value, key: &str) -> f64 {
    seg.get(key).filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

fn has_text(seg: &Value, key: &str) -> bool {
    !text_or_empty(seg.get(key)).trim().is_empty()
}

fn has_items(seg: &Value, key: &str) -> bool {
    seg.get(key).and_then(Value::as_array).is_some_and(|a| !a.is_empty())
}

/// The scenes a pipeline would touch and the work each one needs, with a GPU time estimate and what is missing.
pub fn pipeline_plan(project_id: &str, session: &Obj, params: &Obj) -> Value {
    let p = |k: &str| params.get(k);
    let segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    let build_mode = Some(first_text(&[p("build_mode")])).filter(|s| !s.is_empty()).unwrap_or_else(|| "resume_missing".into()).trim().to_lowercase();
    let scope = Some(first_text(&[p("scope")])).filter(|s| !s.is_empty()).unwrap_or_else(|| "all".into()).trim().to_lowercase();
    let scene_ids: Option<Vec<String>> = match p("scene_ids") {
        Some(Value::String(s)) => Some(s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()),
        Some(Value::Array(a)) => Some(a.iter().map(|v| py_str(Some(v)).trim().to_string()).filter(|x| !x.is_empty()).collect()),
        _ => None,
    };
    let pipeline_kind = Some(first_text(&[p("pipeline")])).filter(|s| !s.is_empty()).unwrap_or_else(|| "full_video".into()).trim().to_lowercase();

    let video_engine = {
        let given = first_text(&[session.get("video_engine")]);
        if !given.is_empty() {
            given
        } else if text_or_empty(session.get("video_mode")) == "minimax_h3" {
            "minimax_h3".into()
        } else {
            "ltx".into()
        }
    };
    let fallback_mode = if video_engine == "minimax_h3" { "minimax_h3" } else { "i2v" };
    let mut video_mode = first_text(&[p("video_mode"), session.get("video_mode")]);
    if video_mode.is_empty() {
        video_mode = fallback_mode.into();
    }
    let mut video_mode = video_mode.trim().to_lowercase();
    if pipeline_kind == "flf" {
        video_mode = "flf".into();
    }
    let nested = session.get("settings").and_then(Value::as_object).and_then(|s| s.get("image_model_mode"));
    let image_mode = Some(first_text(&[p("image_mode"), session.get("image_model_mode"), nested])).filter(|s| !s.is_empty()).unwrap_or_else(|| "zimage".into()).trim().to_lowercase();

    let targets = filter_target_scenes(&segments, &scope, scene_ids.as_deref());
    let (mut images, mut videos, mut prompts) = (0usize, 0usize, 0usize);
    let image_applicable = !matches!(video_mode.as_str(), "t2v" | "rtv") && build_mode != "redo_videos";
    let video_field = if video_mode.starts_with("minimax") { "minimax_h3_prompt" } else { "i2v_prompt" };
    let mut scenes: Vec<Value> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (idx, sid, seg) in &targets {
        let duration = (num(seg, "end") - num(seg, "start")).max(0.0);
        let has_image = seg.get("approved_image_path").is_some_and(truthy) || seg.get("custom_image_path").is_some_and(truthy) || has_items(seg, "image_history");
        let has_video = seg.get("video_path").is_some_and(truthy) || seg.get("rendered_video_path").is_some_and(truthy) || has_items(seg, "video_history");
        let has_image_prompt = has_text(seg, "t2i_prompt");
        let has_video_prompt = has_text(seg, video_field);
        let need_img_prompt = image_applicable && (!has_image_prompt || build_mode == "fresh_rebuild");
        let need_img = image_applicable && (!has_image || build_mode == "fresh_rebuild");
        let need_vid_prompt = build_mode != "redo_videos" && (!has_video_prompt || matches!(build_mode.as_str(), "fresh_rebuild" | "redo_i2v_prompts_videos"));
        let need_vid = !has_video || matches!(build_mode.as_str(), "fresh_rebuild" | "redo_videos" | "redo_i2v_prompts_videos");
        if need_img_prompt || need_vid_prompt {
            prompts += usize::from(need_img_prompt) + usize::from(need_vid_prompt);
        }
        images += usize::from(need_img);
        videos += usize::from(need_vid);
        if duration <= 0.0 {
            warnings.push(format!("Scene {sid} has zero or negative duration ({}s).", py_float_str(duration)));
        }
        scenes.push(json!({
            "scene_id": sid, "scene_number": idx + 1, "start": num(seg, "start"), "end": num(seg, "end"), "duration": duration,
            "has_image": has_image, "has_video": has_video, "has_image_prompt": has_image_prompt, "has_video_prompt": has_video_prompt,
            "actions": {"generate_image_prompt": need_img_prompt, "generate_image": need_img, "generate_video_prompt": need_vid_prompt, "render_video": need_vid},
        }));
    }
    let video_seconds = if video_mode.starts_with("minimax") { 180.0 } else { 45.0 };
    let image_seconds = if image_mode.contains("flux") { 25.0 } else { 15.0 };
    let will_stitch = params.get("stitch").is_none_or(truthy) && !targets.is_empty();
    let total = images as f64 * image_seconds + videos as f64 * video_seconds + prompts as f64 * 10.0 + if will_stitch { 15.0 } else { 0.0 };

    let mut missing: Vec<String> = Vec::new();
    let audio = session_audio_path(session);
    if audio.is_empty() {
        missing.push("Project has no audio file assigned.".into());
    } else if !Path::new(&audio).is_file() {
        missing.push(format!("Project audio file was not found on disk: {audio}"));
    }
    if targets.is_empty() {
        missing.push("No scenes matched the requested scope or scene_ids.".into());
    }
    json!({
        "project_id": project_id, "pipeline": pipeline_kind, "build_mode": build_mode, "scope": scope, "video_engine": video_engine,
        "video_mode": video_mode, "image_mode": image_mode, "target_scenes": scenes,
        "summary": {
            "total_target_scenes": targets.len(), "images_to_generate": images, "videos_to_render": videos, "prompts_to_generate": prompts,
            "will_stitch": will_stitch, "estimated_gpu_seconds": round_to(total, 1), "estimated_gpu_minutes": round_to(total / 60.0, 2),
        },
        "missing_prerequisites": missing, "warnings": warnings, "can_run": missing.is_empty(),
    })
}

/// `int(value)` of a pipeline parameter, clamped.
pub fn clamped_int(params: &Obj, key: &str, default: i64, low: i64, high: i64) -> i64 {
    params.get(key).and_then(to_int).unwrap_or(default).clamp(low, high)
}
