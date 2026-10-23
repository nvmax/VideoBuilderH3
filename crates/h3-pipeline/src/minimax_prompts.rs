//! Writing a scene's MiniMax H3 prompt with the project's LLM. Port of `generate_scene_prompt` in
//! `agent_api/orchestrator/minimax_prompt_orchestrator.py` for reference to video scenes.
//!
//! The LLM writes the creative shot descriptions from a task the app builds. The format around them (shot labels, cut
//! times, reference definitions, the 7,000 character limit) is built by the ported `h3_core::prompt` code.
//!
//! Timed cue maps cut the scene at each cue. A scene continued with masked latent continuation is written as the next
//! moment of the previous take, from the previous scene's final frame when one is given (the LLM looks at it) and from
//! that scene's last shot as text when not.
//!
//! Not in this app yet, and said so as an error: the "omit lyrics" mode.

use h3_core::prompt::continuation::location_continuity_contract;
use h3_core::prompt::cues::{cue_cut_plan, enforce_cue_shot, timed_cue_rows};
use h3_core::prompt::emotion::{emotion_expression_input, emotion_expression_instruction};
use h3_core::prompt::frame::{character_budget, compact_reference_prompt, reference_frame, reference_labels, validate_prompt, wrap_reference_prompt};
use h3_core::prompt::lyrics::{ensure_quoted_lyrics, last_shot_text};
use h3_core::prompt::masked::masked_continuation_context;
use h3_core::prompt::performance::strip_singing_articulation;
use h3_core::prompt::plan::{apply_cut_style, shot_plan, HARD_LIMIT};
use h3_core::prompt::task::{build_shot_task, ShotTaskArgs};
use h3_core::prompt::text::{assemble_prompt, parse_shot_descriptions};
use h3_core::pyval::{text_or_empty, to_float, truthy, Obj};
use h3_core::scene::ordered_reference_items;
use h3_core::style::{append_temporal_world_effect, apply_fx, fx_reserve, storyboard_cut_plan_for_duration, temporal_world_effect_verbiage, video_style_guidance, video_style_label};
use h3_core::timeline::sort_segments;
use h3_core::CoreError;
use h3_llm::{chat, ChatRequest, LlmConfig, LlmError};
use serde_json::{json, Value};

pub const INSTRUCTIONS: &str = include_str!("../../h3-llm/data/minimax_h3_reference_to_video.txt");
/// The Video Builder's instruction for a scene written from the previous scene's final frame.
pub const FRAME_INSTRUCTIONS: &str = include_str!("../../h3-llm/data/minimax_h3_frame_continuity.txt");
const MODE: &str = "reference_to_video";
const MAX_ATTEMPTS: usize = 3;
const DEFAULT_NOTES: &str = "Create cinematic camera movement and natural subject/environment motion that fits the scene.";

#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    /// Something about the scene or project has to change first.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error("{}", .0.message)]
    Shot(CoreError),
}

#[derive(Debug, Clone)]
pub struct PromptParams {
    pub temperature: f64,
    pub top_p: f64,
    pub max_new_tokens: u32,
    /// The final frame of the previous scene's rendered video. Used for a scene that continues the previous one.
    pub previous_frame: Option<std::path::PathBuf>,
}

impl Default for PromptParams {
    fn default() -> Self {
        Self { temperature: 0.45, top_p: 0.92, max_new_tokens: 4000, previous_frame: None }
    }
}

#[derive(Debug, Clone)]
pub struct WrittenPrompt {
    pub prompt: String,
    pub characters: usize,
    pub shots: usize,
    pub attempts: usize,
    pub model: String,
    pub warnings: Vec<String>,
    /// The LLM looked at the previous scene's final frame to write this prompt.
    pub used_previous_frame: bool,
}

fn field(obj: &Obj, key: &str) -> String {
    text_or_empty(obj.get(key)).trim().to_string()
}

fn number(value: Option<&Value>, fallback: f64) -> f64 {
    value.and_then(to_float).filter(|n| n.is_finite()).unwrap_or(fallback)
}

fn obj_of(value: Option<&Value>) -> Obj {
    value.and_then(Value::as_object).cloned().unwrap_or_default()
}

/// The scene's directions as `Title:\nvalue` blocks, in the Builder's order. The Storyboard card JSON the Builder adds
/// after them is left out: this app has no Storyboard.
fn direction_lines(segment: &Obj) -> Vec<String> {
    let mut lines = Vec::new();
    let mut add = |title: &str, value: String| {
        if !value.is_empty() {
            lines.push(format!("{title}:\n{value}"));
        }
    };
    add("Director note (timeline)", field(segment, "timeline_note"));
    add("Storyboard scene story beat", field(segment, "story_beat"));
    let motion = field(segment, "i2v_notes");
    add("Storyboard motion/video summary", motion.clone());
    if motion.is_empty() {
        add("Storyboard camera motion", field(segment, "camera_motion"));
    }
    add("Storyboard shot type", field(segment, "shot_type"));
    add("Storyboard character motion guidance", field(segment, "character_motion"));
    add("Storyboard performance direction", field(segment, "performance_style"));
    let facial = Some(field(segment, "facial_performance_custom")).filter(|s| !s.is_empty()).unwrap_or_else(|| field(segment, "facial_performance"));
    add("Storyboard facial performance direction", facial);
    add("Storyboard lyric section", field(segment, "lyric_section"));
    add("Exact manual audio / sound direction", field(segment, "audio_direction"));
    add("Exact manual continuity requirements", field(segment, "continuity"));
    lines
}

fn describe(item: &Obj) -> String {
    let (name, description) = (field(item, "name"), field(item, "description"));
    if description.is_empty() { name } else { format!("{name}: {description}") }
}

/// The text sent to the LLM: the instructions, the scene concept (the shot task), the notes and the acting direction.
pub fn compose_request(task: &str, notes: &str, emotion: &str) -> String {
    let notes = if notes.trim().is_empty() { DEFAULT_NOTES } else { notes };
    format!("{INSTRUCTIONS}\n\nScene concept:\n{task}\n\nUser motion/camera notes:\n{notes}\n\n{emotion}")
}

/// The request for a scene written from the previous scene's final frame (Attached Picture 1).
pub fn compose_frame_request(task: &str, notes: &str, emotion: &str) -> String {
    let notes = if notes.trim().is_empty() { DEFAULT_NOTES } else { notes };
    format!(
        "{FRAME_INSTRUCTIONS}\n\nVision attachment mapping:\n- Picture 1 is the previous render's final frame and the authoritative opening state. It is prompt-writing input, not a renderer Image N label.\n- Later pictures are the supporting inputs documented by the resolved Scene concept and cannot replace Picture 1's opening state.\n\nScene concept:\n{task}\n\nUser motion/camera notes:\n{notes}\n\n{emotion}"
    )
}

/// The scene's mapped location as the Storyboard card shows it: id, name and description.
fn location_ref(session: &Obj, segment: &Obj) -> Option<Obj> {
    let refs = session.get("flux_reference_builder")?;
    if refs.get("locations_cleared").is_some_and(truthy) {
        return None;
    }
    let wanted = text_or_empty(refs.get("scene_map").and_then(|m| m.get(field(segment, "id")))).trim().to_string();
    let card = refs.get("locations")?.as_array()?.iter().find(|l| text_or_empty(l.get("id")).trim() == wanted)?.as_object()?;
    let mut slim = Obj::new();
    for key in ["id", "name", "description"] {
        slim.insert(key.into(), json!(field(card, key)));
    }
    Some(slim)
}

/// Write the prompt of the scene at `index` (position in the timeline order).
pub async fn generate_scene_prompt(llm: &LlmConfig, session: &Obj, index: usize, params: &PromptParams) -> Result<WrittenPrompt, PromptError> {
    let mut segments: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    sort_segments(&mut segments);
    let segment = segments.get(index).and_then(Value::as_object).cloned().ok_or_else(|| PromptError::Invalid("The scene was not found.".into()))?;
    let session_value = Value::Object(session.clone());
    let label = Some(field(&segment, "label")).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("Scene {}", index + 1));
    let defaults = obj_of(session.get("builder_storyboard_defaults"));
    let settings = obj_of(session.get("minimax_h3_settings"));
    let mut warnings: Vec<String> = Vec::new();

    let items = ordered_reference_items(&session_value, &segment, MODE, index);
    if items.is_empty() {
        return Err(PromptError::Invalid(format!("{label} has no mapped Reference Builder image. Map a character to the scene first.")));
    }
    let audio_mode = Some(field(&settings, "audio_mode")).filter(|s| !s.is_empty()).unwrap_or_else(|| "input_audio".into());
    let performance = Some(field(&segment, "performance_mode")).filter(|s| !s.is_empty()).or_else(|| Some(field(session, "video_type")).filter(|s| !s.is_empty())).unwrap_or_else(|| "singing".into());
    let singing = performance == "singing" && audio_mode != "built_in_audio";
    if session.get("omit_lyrics_from_video_prompts").is_some_and(truthy) && singing {
        return Err(PromptError::Invalid("The project option 'Omit lyrics from video prompts' is not supported in this app yet. Turn it off to write prompts.".into()));
    }
    let scene_settings = h3_core::settings::settings_for_scene(&session_value, Some(&Value::Object(segment.clone())));
    let continued = index > 0 && scene_settings.get("continuity_mode").and_then(Value::as_str) == Some("latent_continuation_masked");

    let (start, end) = (number(segment.get("start"), 0.0), number(segment.get("end"), 0.0));
    let duration = (end - start).max(0.1);
    let mut plan = apply_cut_style(&storyboard_cut_plan_for_duration(duration, number(defaults.get("minimax_h3_cut_frequency"), 0.0) as i64), defaults.get("minimax_h3_cut_style"));
    let mut card = Obj::new();
    for (key, from) in [("video_style", "minimax_h3_video_style"), ("video_style_custom", "minimax_h3_video_style_custom"), ("temporal_world_effect_override", "temporal_world_effect_override"), ("temporal_world_effect_custom", "temporal_world_effect_custom")] {
        card.insert(key.into(), json!(field(&segment, from)));
    }
    let style = video_style_label(&card, &defaults);
    let style_text = video_style_guidance(&card, &defaults);
    let temporal = temporal_world_effect_verbiage(&card, &defaults);
    let lyric_raw = field(&segment, "lyric_text");
    let visual_only = segment.get("lyric_no_lip_sync").is_some_and(truthy) || segment.get("no_character_present").is_some_and(truthy) || h3_core::lyric_scenes::is_instrumental_lyric_text(&lyric_raw);
    let lyric = if visual_only { String::new() } else { h3_core::pyval::collapse_ws(&lyric_raw) };
    let labels = reference_labels(&items);
    let subject_text = items.iter().filter(|i| field(i, "kind") == "subject").map(describe).collect::<Vec<_>>().join("\n");
    let location_text = items.iter().filter(|i| field(i, "kind") == "location").map(describe).collect::<Vec<_>>().join("\n");

    // A scene continued with masked latent continuation is the next moment of the previous take.
    let mut continuation: Option<Obj> = None;
    let (mut previous_shot, mut location_contract) = (String::new(), String::new());
    if continued {
        let has_vocals = !lyric_raw.is_empty() && !visual_only;
        let context = masked_continuation_context(&segment, duration, 1, has_vocals);
        let hold = context.get("hold_seconds").and_then(Value::as_f64).unwrap_or(0.0);
        previous_shot = last_shot_text(&field(segments[index - 1].as_object().unwrap_or(&Obj::new()), "minimax_h3_prompt"), 700);
        let preset = Some(field(&scene_settings, "location_transition_preset")).filter(|s| !s.is_empty()).unwrap_or_else(|| "normal".into());
        let previous = segments[index - 1].as_object().and_then(|p| location_ref(session, p));
        location_contract = location_continuity_contract(location_ref(session, &segment).as_ref(), previous.as_ref(), &preset, &field(&scene_settings, "location_transition_custom"), hold, has_vocals);
        continuation = Some(context);
    }
    // A timed cue map ("Match exact sung words to each shot") cuts at each cue and gives every shot its own words. A
    // continued scene stays one take.
    let mut cues = Vec::new();
    if !continued && !lyric.is_empty() && audio_mode != "built_in_audio" && performance == "singing" {
        let rows = timed_cue_rows(&segment);
        if !rows.is_empty() {
            let cue_plan = apply_cut_style(&cue_cut_plan(&plan, &rows), defaults.get("minimax_h3_cut_style"));
            if shot_plan(&cue_plan).len() == rows.len() {
                plan = cue_plan;
                let subjects: Vec<_> = labels.iter().filter(|l| l.kind == "subject").collect();
                let pool: Vec<_> = if subjects.is_empty() { labels.iter().collect() } else { subjects };
                cues = rows;
                for cue in cues.iter_mut() {
                    let wanted = cue.singer_name.to_lowercase();
                    let singer = pool.iter().find(|l| !l.name.is_empty() && l.name.to_lowercase() == wanted).or_else(|| pool.first());
                    cue.label = Some(match singer {
                        Some(s) if !s.name.is_empty() => format!("{} ({})", s.label, s.name),
                        Some(s) => s.label.clone(),
                        None => "The singer".to_string(),
                    });
                }
            }
        }
    }
    let shots = shot_plan(&plan).len();

    let frame = reference_frame(&items, &plan, &style, &audio_mode, &field(&segment, "audio_direction"));
    let structured = session.get("use_structured_outputs").is_some_and(truthy);
    let blank_core = assemble_prompt(&vec![String::new(); shots], &plan, &style, "").map_err(PromptError::Shot)?;
    let compact_overhead = compact_reference_prompt(&blank_core, &items).chars().count() as i64 - blank_core.chars().count() as i64;
    let mut target_limit = HARD_LIMIT as i64 - if structured { (frame.head.chars().count() + frame.tail.chars().count()) as i64 } else { compact_overhead };
    if !temporal.is_empty() {
        target_limit -= temporal.chars().count() as i64 + 2;
    }
    if !style_text.is_empty() {
        target_limit -= style_text.chars().count() as i64 + 2;
    }
    target_limit -= fx_reserve(&defaults, shots) as i64;

    let mut emotion_payload = emotion_expression_input(&segment, session);
    emotion_payload.insert("lyric_text".into(), json!(lyric));
    emotion_payload.insert("performance_mode".into(), json!(if visual_only { "no_lip_sync" } else { "singing" }));
    emotion_payload.insert("audio_mode".into(), json!(audio_mode));
    let emotion = emotion_expression_instruction(&emotion_payload);

    let mut last_length = 0usize;
    // The previous scene's final frame, shown to the LLM as Attached Picture 1 for a continued scene.
    let mut use_picture = false;
    let mut frame_url = String::new();
    if let (true, Some(path)) = (continued, params.previous_frame.as_ref()) {
        match h3_project::images::preview_data_url(path, 512, 88) {
            Ok(url) => (use_picture, frame_url) = (true, url),
            Err(e) => warnings.push(format!("The loaded LLM could not read the previous scene's final frame ({e}). The previous scene's last shot was used instead.")),
        }
    }
    for attempt in 1..=MAX_ATTEMPTS {
        let budget = character_budget(&plan, &style, target_limit);
        if budget.fixed_chars >= budget.hard_limit {
            return Err(PromptError::Invalid("The fixed MiniMax H3 format already exceeds 7,000 characters.".into()));
        }
        let mut context = direction_lines(&segment).join("\n\n");
        if !style_text.is_empty() {
            context.push_str(&format!("\n\nThe builder adds this exact video style look to the opening line. Write shots that fit it; do not copy it into the shots:\n{style_text}"));
        }
        if !temporal.is_empty() {
            context.push_str(&format!("\n\nMANDATORY exact temporal / world effect verbiage — the builder appends it; do not copy or paraphrase it into the shots:\n{temporal}"));
        }
        let make_args = |with_picture: bool| ShotTaskArgs {
            continuation: continuation.clone(),
            previous_shot: if with_picture { String::new() } else { previous_shot.clone() },
            with_picture,
            location_contract: location_contract.clone(),
            cues: if cues.is_empty() { None } else { Some(cues.clone()) },
            mode_label: "Reference to Video".into(),
            duration,
            aspect_ratio: Some(field(&settings, "aspect_ratio")).filter(|s| !s.is_empty()).unwrap_or_else(|| "16:9".into()),
            audio_mode: audio_mode.clone(),
            cut_plan: plan.clone(),
            style: style.clone(),
            labels: labels.clone(),
            camera_speed: number(segment.get("camera_motion_speed").or_else(|| defaults.get("camera_motion_speed")), 4.0),
            character_speed: number(segment.get("character_motion_speed").or_else(|| defaults.get("character_motion_speed")), 4.0),
            camera_guidance: Some(field(&segment, "camera_motion_speed_guidance")).filter(|s| !s.is_empty()).unwrap_or_else(|| field(&defaults, "camera_guidance")),
            character_guidance: Some(field(&segment, "character_motion_guidance")).filter(|s| !s.is_empty()).unwrap_or_else(|| field(&defaults, "character_guidance")),
            lyric_text: lyric.clone(),
            visual_only,
            no_character: segment.get("no_character_present").is_some_and(truthy),
            story_beat: field(&segment, "story_beat"),
            lyric_section: field(&segment, "lyric_section"),
            scene_notes: Some(field(&segment, "notes")).filter(|s| !s.is_empty()).unwrap_or_else(|| field(&segment, "director_note")),
            subject_text: subject_text.clone(),
            location_text: location_text.clone(),
            seed: Some(field(&segment, "id")).filter(|s| !s.is_empty()).unwrap_or_else(|| label.clone()),
            target_limit,
            motion_request: field(&segment, "i2v_notes"),
            audio_direction: field(&segment, "audio_direction"),
            continuity: field(&segment, "continuity"),
            storyboard_context: context.clone(),
        };
        let max_new = params.max_new_tokens.min(((budget.shot_chars as f64 / 2.5) as u32 + 250).max(600));
        let ask = |with_picture: bool| {
            let task = build_shot_task(&make_args(with_picture));
            let text = if with_picture { compose_frame_request(&task, "", &emotion) } else { compose_request(&task, "", &emotion) };
            let mut request = ChatRequest::text(&text, params.temperature, params.top_p, max_new);
            if with_picture {
                request = request.with_images(vec![frame_url.clone()]);
            }
            request
        };
        let reply = match chat(llm, &ask(use_picture)).await {
            Ok(reply) => reply,
            // A loaded model that cannot read pictures: use the previous scene's last shot as text instead.
            Err(e) if use_picture => {
                use_picture = false;
                warnings.push(format!("The loaded LLM could not read the previous scene's final frame ({e}). The previous scene's last shot was used instead."));
                chat(llm, &ask(false)).await?
            }
            Err(e) => return Err(e.into()),
        };
        let raw = h3_llm::clean::clean_plain_text(&reply.text);
        let mut descriptions = parse_shot_descriptions(&raw, budget.shot_count as usize).map_err(PromptError::Shot)?;
        if !cues.is_empty() {
            descriptions = descriptions.iter().zip(cues.iter()).map(|(text, cue)| enforce_cue_shot(text, cue)).collect();
        } else if !lyric.is_empty() && audio_mode != "built_in_audio" && !labels.is_empty() {
            let cast: Vec<_> = labels.iter().filter(|l| l.kind == "subject").collect();
            let first = cast.first().copied().unwrap_or(&labels[0]);
            let performer = if first.name.is_empty() { first.label.clone() } else { format!("{} ({})", first.label, first.name) };
            descriptions = ensure_quoted_lyrics(&descriptions, &field(&segment, "lyric_text"), &performer);
        }
        if singing {
            descriptions = descriptions.iter().map(|d| strip_singing_articulation(d)).collect();
        }
        let core = assemble_prompt(&descriptions, &plan, &style, &style_text).map_err(PromptError::Shot)?;
        let styled = apply_fx(&core, &defaults);
        let prompt = if structured { wrap_reference_prompt(&styled, &frame) } else { compact_reference_prompt(&styled, &items) };
        let prompt = append_temporal_world_effect(&prompt, &temporal);
        let checked = validate_prompt(&core, &plan).map(|_| ()).and_then(|_| {
            let length = prompt.chars().count();
            if length > HARD_LIMIT {
                let mut e = CoreError::shot(format!("The MiniMax H3 prompt is {length} characters, over the {HARD_LIMIT} maximum by {}.", length - HARD_LIMIT), "MINIMAX_H3_PROMPT_TOO_LONG");
                e.length = length as i64;
                Err(e)
            } else {
                Ok(())
            }
        });
        match checked {
            Ok(()) => return Ok(WrittenPrompt { characters: prompt.chars().count(), prompt, shots: budget.shot_count as usize, attempts: attempt, model: reply.model, warnings, used_previous_frame: use_picture }),
            Err(e) if e.code.as_deref() == Some("MINIMAX_H3_REFERENCE_COMPOSITION_LEAK") && attempt < MAX_ATTEMPTS => continue,
            Err(e) if e.code.as_deref() == Some("MINIMAX_H3_PROMPT_TOO_LONG") && attempt < MAX_ATTEMPTS => {
                last_length = e.length as usize;
                target_limit = budget.fixed_chars.max(target_limit - 350.max(e.length - HARD_LIMIT as i64 + 250));
            }
            Err(e) => return Err(PromptError::Shot(e)),
        }
    }
    Err(PromptError::Invalid(format!("Could not fit the prompt in {HARD_LIMIT} characters after {MAX_ATTEMPTS} tries (last {last_length}).")))
}
