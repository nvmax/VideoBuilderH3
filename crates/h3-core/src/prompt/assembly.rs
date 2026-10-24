//! Prompt assembly, validation and the authoring brief for outside agents. Port of `minimax/prompt_assembly.py`.

use serde_json::{json, Value};

use crate::prompt::rx::rx;

use crate::pyval::{py_float_str, py_str, round_to, text_or_empty, to_float, truthy, Obj};
use crate::scene::{ordered_reference_items, segment_image_path};
use crate::settings::settings_for_scene;

use super::emotion::{emotion_expression_input, emotion_expression_instruction};
use super::frame::{character_reference_bindings, compact_reference_prompt, reference_frame, wrap_reference_prompt};
use super::lyric_free::{apply_shots, prompt_context, scene_cut_plan as frequency_cut_plan, scene_enabled, scene_performers, Labels};
use super::masked::masked_continuation_context;
use super::performance::strip_singing_articulation;
use super::plan::{apply_cut_style, shot_plan, timecode_f, CONTINUOUS_CUTS_OPENING, FALLBACK_SHOT};
use super::text::{assemble_prompt, continuous_cut_text, normalize_description, strip_negative_sentences};

const MAX_PROMPT_CHARS: usize = 7000;
const MAX_REFERENCE_IMAGES: usize = 9;
const PAD_SHOT: &str = "A stable cinematic shot with natural camera movement.";

/// `float(value or 0.0)`.
fn fnum(value: Option<&Value>) -> f64 {
    value.filter(|v| truthy(v)).and_then(to_float).unwrap_or(0.0)
}

fn first_text(values: &[Option<&Value>]) -> String {
    values.iter().flatten().find(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default()
}

/// Normalize a MiniMax H3 mode name.
pub fn normalize_minimax_mode(mode: &str) -> &'static str {
    match mode.trim().to_lowercase().as_str() {
        "image_to_video" | "i2v" => "image_to_video",
        "reference_to_video" | "r2v" => "reference_to_video",
        "image_reference_to_video" | "image_plus_reference_to_video" | "i2v_r2v" => "image_reference_to_video",
        "video_to_video" | "v2v" => "video_to_video",
        _ => "text_to_video",
    }
}

/// Drop lines that carry quality boilerplate, join the rest.
pub fn strip_negative_prompt_phrasing(text: &str) -> String {
    let re = rx!(r"(?i)\b(no blur|no distortion|high quality|photorealistic|4k|hyperrealistic|cinematic lighting)\b");
    let kept: Vec<&str> = text.split('\n').map(str::trim).filter(|l| !re.is_match(l).unwrap_or(false)).collect();
    kept.join(" ").trim().to_string()
}

/// Tidy one shot description for the plain (non reference) formats.
pub fn normalize_shot_description(text: &str) -> String {
    let t = text.trim();
    let t = rx!(r"(?i)^\[Shot\s*\d+\]\s*(?:At\s*\d+:\d+(?:\.\d+)?,\s*)?").replace(t, "").trim().to_string();
    let t = rx!(r"\s+").replace_all(&t, " ").to_string();
    let mut t = strip_negative_prompt_phrasing(&t);
    if !t.is_empty() && !rx!(r"[.!?…]$").is_match(&t).unwrap_or(false) {
        t.push('.');
    }
    t
}

fn defaults_of(session: &Obj) -> Obj {
    session.get("builder_storyboard_defaults").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// The scene's MiniMax mode: the caller's, else the scene's, else the project's saved `video_mode`.
pub fn effective_mode(segment: &Obj, session: &Obj, mode: Option<&str>) -> &'static str {
    let settings = session.get("minimax_h3_settings").and_then(Value::as_object).cloned().unwrap_or_default();
    let mode_value = mode.map(|m| json!(m));
    let chosen = first_text(&[mode_value.as_ref(), segment.get("minimax_h3_mode"), segment.get("video_mode"), settings.get("video_mode"), session.get("video_mode")]);
    normalize_minimax_mode(&chosen)
}

/// The scene's shot plan from its length and the project's saved cut frequency and cut style.
pub fn scene_cut_plan(segment: &Obj, session: &Obj) -> Obj {
    apply_cut_style(&frequency_cut_plan(segment, session), defaults_of(session).get("minimax_h3_cut_style"))
}

fn plan_int(plan: &Obj, key: &str) -> usize {
    plan.get(key).and_then(Value::as_i64).unwrap_or(0).max(0) as usize
}

fn style_of(segment: &Obj, session: &Obj, fallback: &str) -> String {
    let defaults = defaults_of(session);
    let style = first_text(&[segment.get("minimax_h3_video_style"), defaults.get("video_style")]);
    if style.is_empty() { fallback.to_string() } else { style }
}

fn ordered_index(session: &Obj, segment: &Obj) -> usize {
    let mut ordered: Vec<&Value> = session.get("segments").and_then(Value::as_array).map(|a| a.iter().filter(|x| x.is_object()).collect()).unwrap_or_default();
    ordered.sort_by(|a, b| fnum(a.get("start")).total_cmp(&fnum(b.get("start"))));
    ordered.iter().position(|x| x.get("id") == segment.get("id")).unwrap_or(0)
}

fn item_path(item: &Obj) -> String {
    item.get("image").and_then(Value::as_object).map(|i| text_or_empty(i.get("path"))).unwrap_or_default()
}

/// Assemble the saved prompt text from shot descriptions.
pub fn assemble_minimax_h3_prompt(segment: &Obj, session: &Obj, descriptions: &[String], mode: Option<&str>) -> Result<Obj, String> {
    let norm_mode = effective_mode(segment, session, mode);
    let dur = (fnum(segment.get("end")) - fnum(segment.get("start"))).max(0.1);
    let cut_plan = scene_cut_plan(segment, session);
    let omit_lyrics = scene_enabled(session, segment);
    let session_value = Value::Object(session.clone());
    let settings = settings_for_scene(&session_value, Some(&Value::Object(segment.clone())));
    let performance = first_text(&[segment.get("performance_mode"), session.get("video_type")]);
    let supplied_singing = (if performance.is_empty() { "singing" } else { performance.as_str() }) == "singing" && settings.get("audio_mode").and_then(Value::as_str) != Some("built_in_audio");
    let (performer, labels) = if omit_lyrics { scene_performers(session, segment, norm_mode) } else { (String::new(), Labels(vec![])) };
    let expected_shots = plan_int(&cut_plan, "shot_count");
    let continuous = cut_plan.get("continuous").is_some_and(truthy);

    let mut out = Obj::new();
    if norm_mode == "reference_to_video" {
        let wanted = shot_plan(&cut_plan).len();
        let mut cleaned: Vec<String> = descriptions
            .iter()
            .map(|d| {
                let text = normalize_description(&strip_negative_sentences(d));
                if text.is_empty() { FALLBACK_SHOT.to_string() } else { text }
            })
            .take(wanted)
            .collect();
        while cleaned.len() < wanted {
            cleaned.push(FALLBACK_SHOT.to_string());
        }
        let style = style_of(segment, session, "");
        if omit_lyrics {
            cleaned = apply_shots(&cleaned, segment, &cut_plan, &performer, &labels, session);
        }
        if supplied_singing {
            cleaned = cleaned.iter().map(|t| strip_singing_articulation(t)).collect();
        }
        let mut prompt_text = assemble_prompt(&cleaned, &cut_plan, &style, "").map_err(|e| e.to_string())?;
        let items = ordered_reference_items(&session_value, segment, "reference_to_video", ordered_index(session, segment));
        if !items.is_empty() {
            let project = session.get("minimax_h3_settings").and_then(Value::as_object).and_then(|s| s.get("audio_mode")).filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_else(|| "input_audio".into());
            let frame = reference_frame(&items, &cut_plan, &style, &project, &text_or_empty(segment.get("audio_direction")));
            prompt_text = if session.get("use_structured_outputs").is_some_and(truthy) { wrap_reference_prompt(&prompt_text, &frame) } else { compact_reference_prompt(&prompt_text, &items) };
        }
        out.insert("prompt".into(), json!(prompt_text));
        out.insert("characters".into(), json!(prompt_text.chars().count()));
        out.insert("shots_used".into(), json!(wanted));
        out.insert("mode".into(), json!(norm_mode));
        out.insert("cut_plan".into(), Value::Object(cut_plan));
        return Ok(out);
    }

    let mut cleaned: Vec<String> = descriptions.iter().map(|d| normalize_shot_description(d)).collect();
    while cleaned.len() < expected_shots {
        cleaned.push(PAD_SHOT.to_string());
    }
    cleaned.truncate(expected_shots);
    if omit_lyrics {
        cleaned = apply_shots(&cleaned, segment, &cut_plan, &performer, &labels, session);
    }
    if supplied_singing {
        cleaned = cleaned.iter().map(|t| strip_singing_articulation(t)).collect();
    }

    let cut_times: Vec<f64> = super::plan::cut_times(&cut_plan);
    let mut parts: Vec<String> = Vec::new();
    for (i, desc) in cleaned.iter().enumerate() {
        let shot_num = i + 1;
        if shot_num == 1 {
            parts.push(format!("[Shot 1] {desc}"));
        } else {
            let time = cut_times.get(i - 1).copied().unwrap_or(dur * i as f64 / expected_shots as f64);
            let text = if continuous { continuous_cut_text(desc, shot_num as i64) } else { desc.clone() };
            parts.push(format!("[Shot {shot_num}] At {}, {text}", timecode_f(time)));
        }
    }
    let mut shots_body = parts.join("\n\n");
    let style = style_of(segment, session, "photorealistic");
    let continuity = if continuous { format!(" {CONTINUOUS_CUTS_OPENING}") } else { String::new() };

    let prompt_text = if norm_mode == "image_reference_to_video" {
        let refs = ordered_reference_items(&session_value, segment, norm_mode, ordered_index(session, segment));
        let mut items: Vec<Obj> = Vec::new();
        let start = segment_image_path(segment);
        let end = text_or_empty(segment.get("first_last_frame_end_image_path"));
        if !start.is_empty() {
            items.push(json!({"kind": "start_frame", "image": {"path": start}}).as_object().cloned().unwrap_or_default());
        }
        if !end.is_empty() && end != start {
            items.push(json!({"kind": "end_frame", "image": {"path": end}}).as_object().cloned().unwrap_or_default());
        }
        let mut seen: Vec<String> = items.iter().map(item_path).collect();
        for item in refs {
            let path = item_path(&item);
            let kind = text_or_empty(item.get("kind"));
            if kind == "start_frame" || kind == "end_frame" || seen.contains(&path) {
                continue;
            }
            seen.push(path);
            let mut item = item;
            if kind == "subject" {
                let face_only = segment.get("minimax_h3_use_scene_image_as_start_frame").is_some_and(truthy)
                    && segment.get("minimax_h3_start_frame_character_influence").and_then(Value::as_str) == Some("face_hair_only");
                let purpose = if face_only { "face identity and hair reference only" } else { "character identity, face, hair, complete outfit, footwear, accessories, and body-proportion reference" };
                item.insert("purpose".into(), json!(purpose));
            }
            items.push(item);
        }
        items.truncate(MAX_REFERENCE_IMAGES);
        shots_body = shots_body.replacen("[Shot 1] ", "[Shot 1] The shot begins exactly from <Picture 1>. ", 1);
        if !end.is_empty() && end != start {
            shots_body.push_str(" The final motion converges exactly on <Picture 2> as the last frame.");
        }
        let detailed = format!("detailed_description:\nThe target video is in a {style} music-video style.{continuity}\n\n{shots_body}");
        if session.get("use_structured_outputs").is_some_and(truthy) {
            let audio_mode = settings.get("audio_mode").filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_else(|| "input_audio".into());
            let frame = reference_frame(&items, &cut_plan, &style, &audio_mode, &text_or_empty(segment.get("audio_direction")));
            wrap_reference_prompt(&detailed, &frame)
        } else {
            let bindings = character_reference_bindings(&shots_body, &items);
            let mut text = compact_reference_prompt(&detailed, &items);
            if !bindings.is_empty() {
                text = text.replacen("\n\n[Shot 1]", &format!("\n\n{bindings}\n\n[Shot 1]"), 1);
            }
            text
        }
    } else if matches!(norm_mode, "text_to_video" | "image_to_video") {
        let mut header = format!("integrated_multimodal_description:\nGenerate a {}-second 16:9 {style} video.", py_float_str(round_to(dur, 1)));
        if norm_mode == "image_to_video" {
            header.push_str(" <Picture 1> is the exact opening composition, character identity, and visual anchor.");
        }
        format!("{header}\n\n{shots_body}\n\noverall_soundscape:\nAmbient music video soundstage.\n\nnon_diegetic_music:\nFull synchronized musical performance.")
    } else {
        let subject_defs = [
            "<Subject 1> is the featured performer in <Picture 1>.",
            "<Picture 1> is the first frame of [Shot 1], used as the opening composition and visual anchor.",
            "<Audio 1> is the complete synchronized vocal track.",
        ];
        let summary = format!("summary:\n[reference generation + audio reuse] The target video is a {style} cinematic scene featuring <Subject 1>.");
        let retention = [
            "retention_analysis:",
            "<Subject 1> (appears in all shots): fully_preserved - the character identity and performance are retained.",
            "<Picture 1> ([Shot 1] first frame): fully_preserved - the opening framing and lighting are retained.",
            "<Audio 1>: fully_copy - the synchronized music and vocal audio track is fully copied.",
        ];
        let detailed = format!("detailed_description:\nThe target video is in a {style} music-video style.{continuity}\n\n{shots_body}");
        [format!("subject_definitions:\n{}", subject_defs.join("\n")), summary, retention.join("\n"), detailed].join("\n\n")
    };
    out.insert("prompt".into(), json!(prompt_text));
    out.insert("characters".into(), json!(prompt_text.chars().count()));
    out.insert("shots_used".into(), json!(cleaned.len()));
    out.insert("mode".into(), json!(norm_mode));
    out.insert("cut_plan".into(), Value::Object(cut_plan));
    Ok(out)
}

/// Check a prompt against the length, reference count and (optionally) the required sections.
pub fn validate_minimax_h3_prompt(prompt: &str, segment: Option<&Obj>, mode: Option<&str>, fail_on_invalid_prompt_formats: bool) -> Value {
    let text = prompt.trim();
    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();
    if text.is_empty() {
        errors.push(json!({"code": "EMPTY_PROMPT", "message": "The MiniMax prompt is empty."}));
        return json!({"valid": false, "errors": errors, "warnings": warnings, "length": 0});
    }
    let length = text.chars().count();
    if length > MAX_PROMPT_CHARS {
        errors.push(json!({
            "code": "MINIMAX_H3_PROMPT_TOO_LONG",
            "message": format!("Prompt length ({length}) exceeds the 7,000 character maximum by {}.", length - MAX_PROMPT_CHARS),
            "length": length, "limit": MAX_PROMPT_CHARS, "over_by": length - MAX_PROMPT_CHARS,
        }));
    }
    let mut pictures: Vec<String> = Vec::new();
    for caps in rx!(r"(?i)<Picture\s+(\d+)>").captures_iter(text).flatten() {
        let n = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        if !pictures.contains(&n) {
            pictures.push(n);
        }
    }
    if pictures.len() > MAX_REFERENCE_IMAGES {
        errors.push(json!({"code": "TOO_MANY_REFERENCES", "message": format!("Prompt references {} pictures, exceeding the maximum of {MAX_REFERENCE_IMAGES}.", pictures.len())}));
    }
    let given = mode.map(str::to_string).filter(|m| !m.is_empty()).unwrap_or_else(|| segment.map(|s| text_or_empty(s.get("video_mode"))).unwrap_or_default());
    let norm_mode = normalize_minimax_mode(&given);
    if fail_on_invalid_prompt_formats {
        if matches!(norm_mode, "text_to_video" | "image_to_video") {
            if !text.contains("integrated_multimodal_description:") {
                errors.push(json!({"code": "MISSING_HEADER", "message": "Missing 'integrated_multimodal_description:' header in prompt."}));
            }
        } else {
            let required: &[&str] = if norm_mode == "reference_to_video" { &["detailed_description:"] } else { &["subject_definitions:", "summary:", "retention_analysis:", "detailed_description:"] };
            for section in required {
                if !text.contains(section) {
                    errors.push(json!({"code": "MISSING_SECTION", "message": format!("Missing required section '{section}' in prompt.")}));
                }
            }
        }
        let shots: Vec<i64> = rx!(r"(?i)\[Shot\s*(\d+)\]").captures_iter(text).flatten().filter_map(|c| c.get(1)).filter_map(|m| m.as_str().parse().ok()).collect();
        match shots.first() {
            None => errors.push(json!({"code": "NO_SHOTS_FOUND", "message": "No [Shot N] blocks found in prompt."})),
            Some(first) if *first != 1 => warnings.push(json!({"code": "SHOT_ORDER", "message": format!("First shot is numbered {first} instead of 1.")})),
            _ => {}
        }
    }
    json!({
        "valid": errors.is_empty(), "errors": errors, "warnings": warnings, "length": length,
        "character_budget_remaining": MAX_PROMPT_CHARS.saturating_sub(length),
    })
}

const MULTI_SHOT: &str = "MULTI-SHOT CONTINUITY — MANDATORY: When a scene has multiple shots, write them as consecutive camera views of one uninterrupted physical performance in the same place and moment. Shot 1 establishes staging. In every later description, write a complete sentence explicitly continuing directly from the previous shot's final moment. At the instant of the cut, preserve the previous ending position, posture, body orientation, eyeline and prop contact, then carry the ongoing movement forward at its existing pace and direction. For a scene with active singing, for example: 'Continuing directly from the previous shot's final moment, <Subject 1> remains seated in the same position on the curb, her torso still angled toward the doorway, and continues the same uninterrupted singing performance in sync with the assigned audio.' Use the actual preceding physical state, not a generic continuity phrase. Do not restart an action, reset a pose, or replace continued performance with an unrelated beauty shot. An ongoing action may span scheduled cuts. Only camera angle, framing and camera movement change at a cut unless the scene explicitly requests a new action or transition. Show any requested physical change developing naturally from the inherited state. During every shot's active vocal interval, explicitly state that the assigned performer continues singing or speaking in sync with the assigned audio. Preserve cue timing and speaker assignments; instrumental, no-lip-sync and silent intervals remain non-vocal. Follow supplied lyric/dialogue allocations without duplicating an already quoted cue or restarting the scene's lyrics after a cut. Use complete grammatical sentences rather than fragments such as 'Still seated on the curb.' Self-contained shot prose and varied camera coverage must preserve the shared staging. Before returning, check every adjacent pair for a matching end-to-start physical state and continuous performance.";

/// The brief an outside agent needs to write this scene's shot descriptions.
pub fn build_minimax_prompt_context(segment: &Obj, session: &Obj, mode: Option<&str>) -> Obj {
    let norm_mode = effective_mode(segment, session, mode);
    let dur = (fnum(segment.get("end")) - fnum(segment.get("start"))).max(0.1);
    let cut_plan = scene_cut_plan(segment, session);
    let shot_count = plan_int(&cut_plan, "shot_count").max(1);
    let overhead = match norm_mode {
        "text_to_video" | "image_to_video" => 400,
        "reference_to_video" => 300,
        _ => 1200,
    };
    let available = MAX_PROMPT_CHARS.saturating_sub(overhead).max(500);
    let per_shot = available / shot_count;

    let builder = session.get("flux_reference_builder").and_then(Value::as_object).cloned().unwrap_or_default();
    let subjects: Vec<Value> = builder.get("subjects").and_then(Value::as_array).cloned().unwrap_or_default();
    let locations: Vec<Value> = builder.get("locations").and_then(Value::as_array).cloned().unwrap_or_default();
    let cast: Vec<Value> = subjects
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let n = i + 1;
            let name = s.get("name").cloned().unwrap_or_else(|| json!(format!("Character {n}")));
            let role = s.get("auto_build_role").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("lead performer"));
            let kind = s.get("reference_type").cloned().unwrap_or_else(|| json!("character"));
            json!({"label": format!("<Subject {n}>"), "name": name, "role": role, "reference_type": kind})
        })
        .collect();

    let mut instruction = format!(
        "Generate {shot_count} cinematic shot descriptions for a {}s {norm_mode} scene.\nReturn JSON: {{\"shots\": [{{\"description\": \"...\"}}]}}.\nTarget approx {per_shot} characters per shot description.",
        py_float_str(round_to(dur, 1))
    );
    if shot_count > 1 {
        instruction.push_str("\n\n");
        instruction.push_str(MULTI_SHOT);
    }
    let session_value = Value::Object(session.clone());
    let settings = settings_for_scene(&session_value, Some(&Value::Object(segment.clone())));
    let mut continuation = None;
    if settings.get("continuity_mode").and_then(Value::as_str) == Some("latent_continuation_masked") {
        let has_vocals = !segment.get("lyric_no_lip_sync").is_some_and(truthy) && !text_or_empty(segment.get("lyric_text")).trim().is_empty();
        continuation = Some(masked_continuation_context(segment, dur, shot_count, has_vocals));
        instruction.push_str("\nThis scene continues the previous scene (latent_continuation_masked): follow continuation.rules.");
    }
    let mut context = Obj::new();
    context.insert("mode".into(), json!(norm_mode));
    context.insert("duration_seconds".into(), json!(round_to(dur, 2)));
    context.insert("shot_plan".into(), Value::Object(cut_plan.clone()));
    context.insert(
        "budget".into(),
        json!({"hard_limit": MAX_PROMPT_CHARS, "estimated_overhead_chars": overhead, "available_description_chars": available, "per_shot_chars": per_shot}),
    );
    context.insert("cast".into(), Value::Array(cast));
    context.insert("references".into(), json!({"subjects_count": subjects.len(), "locations_count": locations.len()}));
    context.insert("notes".into(), segment.get("notes").cloned().unwrap_or_else(|| json!("")));
    context.insert("t2i_prompt".into(), segment.get("t2i_prompt").cloned().unwrap_or_else(|| json!("")));
    context.insert("i2v_prompt".into(), segment.get("i2v_prompt").cloned().unwrap_or_else(|| json!("")));
    context.insert("instruction_text".into(), json!(instruction));
    if let Some(c) = continuation {
        context.insert("continuation".into(), Value::Object(c));
    }
    let omit = scene_enabled(session, segment);
    context.insert("omit_lyrics_from_video_prompts".into(), json!(omit));
    if omit {
        let text = prompt_context(&instruction, segment, &cut_plan, "The assigned performer", &Labels(vec![]), session);
        context.insert("instruction_text".into(), json!(text));
        let cues: Vec<Value> = segment
            .get("lyric_cue_map")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|cue| Value::Object(cue.as_object().map(|o| o.iter().filter(|(k, _)| k.as_str() != "text").map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()))
            .collect();
        context.insert("lyric_cue_map".into(), Value::Array(cues));
    }
    context.insert("lyric_text".into(), json!(segment.get("lyric_text").filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default()));
    for (k, v) in emotion_expression_input(segment, session) {
        context.insert(k, v);
    }
    let base = text_or_empty(context.get("instruction_text"));
    let extra = emotion_expression_instruction(&context);
    context.insert("instruction_text".into(), json!(format!("{base}\n\n{extra}")));
    context
}
