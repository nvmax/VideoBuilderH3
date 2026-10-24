//! The Builder Agent's chat reply: the model answers with a short message and a list of actions the Builder applies, and the reply is checked
//! here. Only supported actions with a target survive, and a few of the user's phrases (merge, split, director note wording) correct what the
//! model got wrong. Port of `llm/builder_agent.py`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use fancy_regex::Regex;
use h3_core::pyjson::dumps;
use h3_core::pyval::{py_type_name, text_or_empty, to_int, truthy, Obj};
use h3_llm::clean::clean_plain_text;
use h3_llm::locations::extract_json_object;
use serde_json::{json, Value};

use super::{flag, number, text, whole, Llm, TextCall};

type Reply = Result<Value, String>;

const HEAD: &str = r#"You are the VRGDG Music Video Builder Agent, a local assistant inside a scene-by-scene music video workflow.
Help the user think through lyrics, scene concepts, image prompts, video prompts, shot choices, continuity, and troubleshooting.
Be practical, concise, and specific to the provided active scene context.
Keep the user-facing reply short: usually 1-5 sentences. Do not write strategy essays, option menus, or long explanations unless the user asks.
When suggesting prompt text, provide copy-ready text clearly, but keep normal chat friendly.
If the context is missing something, say what is missing and make a reasonable suggestion from what is available.

"#;

const BODY: &str = r#"Read project_status and scene_directory first. If project_status.scene_count is greater than 0, scenes already exist; do not say no scenes have been created and do not ask to create initial scenes from timeline markers unless the user explicitly asks for that.
If purpose is walkthrough, act like intake plus step-by-step onboarding. First identify what the user is making: music video, short film, ad/social clip, visualizer, or something else. Use project_status before choosing the next step. If project_status.has_audio is false, ask whether they plan to use custom audio and point them to Choose Audio if yes; do not ask for lyrics/SRT before resolving the audio plan. If audio exists, ask how they want scenes created: import Prompt Creator outputs, load SRT/lyrics, manually create scenes, or plan scenes with the agent. Only ask about lyrics when the user is making a music video or lyric-driven project. For short films/ads, ask for script, scene beats, product/message, or visual outline instead. Tell them one small UI step at a time and why. Do not generate prompts or run models unless the user explicitly asks. Prefer one next step over a full tutorial.
If purpose is troubleshoot, focus on diagnosing the user's stated problem from project_status, active scene context, and available get actions.
If purpose is scene_work, help create/update scene notes, prompts, images, and videos using the supported actions.

If purpose is story_builder, act like a scene planner for complex multi-character projects. This workflow may not use Prompt Creator, SRT, or existing lyric segments. First check project_status.has_audio and story_source.has_source. For music videos/songs, if no audio exists, ask for global/timeline audio before asking for lyrics. If no story source exists, ask the user to paste lyrics/script/story source and use save_story_source in Auto Apply mode when they provide it. Use get_story_source when you need the saved lyrics/script instead of keeping it in chat memory. Help define characters, assign which character(s) appear in each scene, match the lyrics/story beat, and create compact per-scene planning notes. Prefer structured scene plans over long prose. When Auto Apply is enabled, use create_story_scenes_from_source if the user asks to create starter scenes from saved lyrics/script, then use set_scene_plan for requested scenes. Do not generate final image/video prompts unless the user explicitly asks; write planning notes that the existing prompt generators can use.

Return only valid JSON with this shape:
{
  "reply": "short message to show the user",
  "actions": []
}

Supported actions are:
- {"type":"select_scene","scene_id":"..."} or {"type":"select_scene","scene_number":2}
- {"type":"get_scene_lyrics","scene_id":"..."} or {"type":"get_scene_lyrics","scene_number":2}
- {"type":"get_scene_context","scene_id":"..."} or {"type":"get_scene_context","scene_number":2}
- {"type":"get_story_source"}
- {"type":"get_context_prompts","context_type":"all|theme_style|story_idea|subject_scene"}
- {"type":"get_selected_timeline_range"}
- {"type":"save_story_source","text":"lyrics/script/story source text"}
- {"type":"set_context_prompt","context_type":"theme_style|story_idea|subject_scene","text":"...","mode":"replace|append"}
- {"type":"create_story_scenes_from_source","scene_count":12}
- {"type":"create_concept_prompts","source_mode":"all|director|scene|timeline|director_scene","scope":"all|selected|range","batch_size":5,"use_story":true,"use_theme":true}
- {"type":"create_motion_notes","source_mode":"concept|concept_director|concept_timeline|all","scope":"all|selected|range","batch_size":5,"use_story":true,"use_theme":true}
- {"type":"set_active_scene_to_selected_range","scene_id":"optional target scene"}
- {"type":"create_scene_from_selected_range","label":"...","director_note":"...","scene_notes":"...","flux_notes":"...","nb_notes":"...","video_notes":"..."}
- {"type":"split_selected_range_into_scenes","scene_count":3,"label_prefix":"Scene","director_notes":["..."],"scene_notes":["..."]}
- {"type":"split_scene_into_subscenes","scene_id":"...","scene_number":4,"scene_count":3,"label_prefix":"Scene 4"}
- {"type":"merge_scenes","scene_numbers":[2,3,4],"label":"Scene 2"}
- {"type":"renumber_scene_labels"}
- {"type":"normalize_dual_vocal_director_notes","replacement":"male and female"}
- {"type":"replace_director_note_text","find":"male singer and female singer","replace":"female and male"}
- {"type":"assign_selected_range_note","label":"Female vocal","marker_type":"female vocal|male vocal|chorus|verse|beat|note","note":"..."}
- {"type":"sync_existing_scenes_to_timeline_markers","create_missing":true}
- {"type":"set_scene_notes","scene_number":2,"text":"..."}
- {"type":"set_flux_notes","scene_number":2,"text":"..."}
- {"type":"set_nb_notes","scene_number":2,"text":"..."}
- {"type":"set_video_notes","scene_number":2,"text":"..."}
- {"type":"set_scene_plan","scene_number":2,"director_note":"...","scene_notes":"...","flux_notes":"...","nb_notes":"...","video_notes":"..."}
- {"type":"set_image_model_mode","image_mode":"zimage|flux_klein|nano_banana|ernie_image"}
- {"type":"set_video_model_mode","video_mode":"i2v|id_lora|t2v|rtv|ingredients"}
- {"type":"request_reference_images","scene_id":"...","image_mode":"nano_banana|flux_klein"}
- {"type":"generate_image_prompt_for_current_mode","scene_id":"...","image_mode":"optional zimage|flux_klein|nano_banana|ernie_image"}
- {"type":"run_image_for_current_mode","scene_id":"...","image_mode":"optional zimage|flux_klein|nano_banana|ernie_image"}
- {"type":"generate_video_prompt_for_current_mode","scene_id":"...","video_mode":"optional i2v|id_lora|t2v|rtv|ingredients"}

- {"type":"run_video_for_current_mode","scene_id":"...","video_mode":"optional i2v|id_lora|t2v|rtv|ingredients"}

Use note actions to capture the creative direction you discuss with the user. Use set_scene_plan when planning character assignments, story beats, scene concepts, and motion notes for one or more scenes. Keep each set_scene_plan field short and direct.
Use create_concept_prompts when the user asks to create, generate, build, or update concept prompts from director notes, scene notes, timeline notes, story idea, or theme/style. This writes generated concept prompts into scene notes in batches.
Use create_motion_notes when the user asks to create, generate, build, or update I2V/T2V motion notes from concept prompts, director notes, timeline notes, story idea, or theme/style. This writes generated motion notes into the I2V motion notes fields/file in batches.
Use get_context_prompts to read the global Theme/style, Story idea, and Subject/scene context prompts. Use set_context_prompt when the user asks to update those global context prompt files. Choose context_type theme_style for style/mood/look, story_idea for plot/concept/narrative, and subject_scene for characters, subjects, locations, and recurring scene details.
Use selected_timeline_range and timeline_markers when the user is timing vocals, choruses, dialogue, or music sections. If they say this selected range is where someone sings, use assign_selected_range_note. If they ask to make a scene cover it, use set_active_scene_to_selected_range. Only use create_scene_from_selected_range or split_selected_range_into_scenes when selected_timeline_range is not null and the user explicitly asks to create/split the selected in/out range. If the user asks to split one existing scene, such as 'split scene 4 into 3', use split_scene_into_subscenes with that scene number and count. If the user asks to combine, merge, join, or consolidate scenes, use merge_scenes with the scene numbers. If the user asks to update/reword/fix director notes, do note actions only; do not sync or change timings. If they ask to replace director note text, such as 'when a director note says X change it to Y', use replace_director_note_text. If they ask that all both-character or dual-vocal director notes say a specific phrase such as 'male and female' or 'female and male', use normalize_dual_vocal_director_notes with that replacement phrase. If timeline markers already exist and the user asks to update, align, match, sync, or split existing scene segments from those markers, or answers yes after you suggest updating scenes from markers, use sync_existing_scenes_to_timeline_markers. If the user asks to fix missing/skipped/duplicate scene numbers or renumber labels, use renumber_scene_labels. Do not use split_selected_range_into_scenes for marker-based timing, one-scene splits, or merge/combine requests.
Use generate actions when the user asks you to make/create/update the actual image or video prompt. Use run_image_for_current_mode only when the user asks to run/create/generate the actual image, not just the text prompt.
If the user asks for multiple steps, include all requested actions in order. Example: prompt, then image, then video means generate_image_prompt_for_current_mode, run_image_for_current_mode, generate_video_prompt_for_current_mode, run_video_for_current_mode.
Nano B and Flux/Klein can run with or without reference images. If no references exist, use the normal generate/run actions anyway and the app will use text-only prompting. Only request reference images when the user specifically asks to use references or the scene concept requires matching an exact character/location.
Do not write final image/video prompts yourself as actions. Do not use unsupported set_image_prompt, set_flux_prompt, set_nb_prompt, or set_video_prompt actions.
Never put JSON, action arrays, code fences, or internal notes inside the reply string. The reply must be a short user-facing sentence; all work must be in the actions array.
Do not claim which image/video generator ran. The app will report the actual mode after it runs the action.
"#;

const AUTO_APPLY: &str = "AUTO APPLY. If the user asks you to do/apply/update/fill/write prompts or notes, include supported note/generate actions for the requested scene fields.";
const MANUAL: &str = "MANUAL. Never include actions. Do not say you applied or updated the project; only suggest copy-ready text.";

const TAIL: &str = r#"Prefer scene_number for scene-targeted actions. Use scene_id only when copying an exact internal id from the context JSON. If the target scene is unclear, ask one short clarifying question and return no actions.
In Auto Apply mode, the application will apply actions after your response. In Manual mode, actions must always be an empty list.

"#;

const ALLOWED_TYPES: &[&str] = &[
    "set_scene_notes", "set_scene_plan", "select_scene", "get_scene_lyrics", "get_scene_context", "get_story_source", "get_context_prompts", "get_selected_timeline_range",
    "save_story_source", "set_context_prompt", "create_story_scenes_from_source", "create_concept_prompts", "create_motion_notes", "set_active_scene_to_selected_range",
    "create_scene_from_selected_range", "split_selected_range_into_scenes", "split_scene_into_subscenes", "merge_scenes", "renumber_scene_labels",
    "normalize_dual_vocal_director_notes", "replace_director_note_text", "assign_selected_range_note", "sync_existing_scenes_to_timeline_markers", "set_flux_notes",
    "set_nb_notes", "set_video_notes", "set_image_model_mode", "set_video_model_mode", "request_reference_images", "generate_image_prompt_for_current_mode",
    "run_image_for_current_mode", "generate_video_prompt_for_current_mode", "run_video_for_current_mode",
];

/// Actions that need no scene.
const NO_SCENE_TYPES: &[&str] = &[
    "set_image_model_mode", "set_video_model_mode", "get_story_source", "get_context_prompts", "save_story_source", "set_context_prompt", "create_story_scenes_from_source",
    "create_concept_prompts", "create_motion_notes", "get_selected_timeline_range", "create_scene_from_selected_range", "split_selected_range_into_scenes", "merge_scenes",
    "renumber_scene_labels", "normalize_dual_vocal_director_notes", "replace_director_note_text", "assign_selected_range_note", "sync_existing_scenes_to_timeline_markers",
];

const IMAGE_MODES: &[&str] = &["zimage", "flux_klein", "nano_banana", "ernie_image"];
const VIDEO_MODES: &[&str] = &["i2v", "id_lora", "t2v", "rtv", "ingredients"];
const PLAN_FIELDS: [&str; 5] = ["director_note", "scene_notes", "flux_notes", "nb_notes", "video_notes"];
const NOTE_TYPES: &[&str] = &["set_scene_notes", "set_flux_notes", "set_nb_notes", "set_video_notes"];

/// A compiled pattern, built once.
fn rx(pattern: &'static str) -> Regex {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Regex>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache.entry(pattern).or_insert_with(|| Regex::new(pattern).expect("valid pattern")).clone()
}

/// `str(value or "").strip()`.
fn strip(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

/// `str(value or "").strip()[:limit]`.
fn clip(value: Option<&Value>, limit: usize) -> String {
    strip(value).chars().take(limit).collect()
}

/// `str(a or b or "").strip()`: the first of the keys that holds something.
fn either(action: &Obj, keys: &[&str]) -> String {
    keys.iter().filter_map(|k| action.get(*k)).find(|v| truthy(v)).map_or_else(String::new, |v| strip(Some(v)))
}

/// `int(value)` for a field that may be missing, null or not a number.
fn whole_field(value: Option<&Value>) -> Option<i64> {
    value.filter(|v| !v.is_null()).and_then(to_int)
}

fn scene_number_of(action: &Obj) -> Option<i64> {
    if let Some(given) = action.get("scene_number").filter(|v| !v.is_null()) {
        return to_int(given);
    }
    let target = ["scene_id", "scene", "scene_label", "scene_name", "target_scene", "target", "label", "name", "title"].iter().map(|f| text_or_empty(action.get(*f))).collect::<Vec<_>>().join(" ");
    let found = rx(r"(?i)\bscene\s*(\d+)(?:\b|\s|\.|:|-)").captures(&target).ok().flatten()?;
    found.get(1)?.as_str().parse().ok()
}

/// A list of notes, each cut to 5000 characters, at most 120 of them.
fn note_list(action: &Obj, field: &str) -> Option<Value> {
    let items = action.get(field)?.as_array()?;
    Some(Value::Array(items.iter().take(120).map(|v| json!(clip(Some(v), 5000))).collect()))
}

/// One action of the model's reply as the Builder may run it, or None when it is dropped.
fn clean_action(action: &Obj) -> Option<Obj> {
    let action_type = strip(action.get("type"));
    let kind = action_type.as_str();
    let scene_id = strip(action.get("scene_id"));
    let raw_text = strip(action.get("text"));
    let image_mode = strip(action.get("image_mode"));
    let video_mode = strip(action.get("video_mode"));
    let context_type = strip(action.get("context_type")).to_lowercase();
    let context_mode = strip(action.get("mode")).to_lowercase();
    let scene_count = whole_field(action.get("scene_count"));
    let plan_values: Vec<(&str, String)> = PLAN_FIELDS.iter().filter_map(|f| Some(clip(action.get(*f), 5000)).filter(|v| !v.is_empty()).map(|v| (*f, v))).collect();
    let scene_number = scene_number_of(action);
    if !ALLOWED_TYPES.contains(&kind) || !(!scene_id.is_empty() || scene_number.is_some() || NO_SCENE_TYPES.contains(&kind)) {
        return None;
    }
    if kind == "request_reference_images" && scene_id.is_empty() {
        return None;
    }
    let mut cleaned = Obj::new();
    cleaned.insert("type".into(), json!(kind));
    if !scene_id.is_empty() {
        cleaned.insert("scene_id".into(), json!(scene_id.chars().take(160).collect::<String>()));
    }
    if let Some(number) = scene_number {
        cleaned.insert("scene_number".into(), json!(number));
    }
    if !raw_text.is_empty() {
        let limit = if kind == "save_story_source" { 50000 } else { 5000 };
        cleaned.insert("text".into(), json!(raw_text.chars().take(limit).collect::<String>()));
    }
    if IMAGE_MODES.contains(&image_mode.as_str()) {
        cleaned.insert("image_mode".into(), json!(image_mode));
    }
    if VIDEO_MODES.contains(&video_mode.as_str()) {
        cleaned.insert("video_mode".into(), json!(video_mode));
    }
    if let Some(count) = scene_count {
        cleaned.insert("scene_count".into(), json!(count.clamp(1, 120)));
    }
    match kind {
        "get_context_prompts" => {
            let chosen = if matches!(context_type.as_str(), "all" | "theme_style" | "story_idea" | "subject_scene") { context_type } else { "all".to_string() };
            cleaned.insert("context_type".into(), json!(chosen));
        }
        "set_context_prompt" => {
            if !matches!(context_type.as_str(), "theme_style" | "story_idea" | "subject_scene") || raw_text.is_empty() {
                return None;
            }
            cleaned.insert("context_type".into(), json!(context_type));
            cleaned.insert("text".into(), json!(raw_text.chars().take(50000).collect::<String>()));
            cleaned.insert("mode".into(), json!(if matches!(context_mode.as_str(), "replace" | "append") { context_mode } else { "replace".to_string() }));
        }
        "assign_selected_range_note" => {
            for field in ["label", "marker_type", "note"] {
                let value = clip(action.get(field), 1000);
                if !value.is_empty() {
                    cleaned.insert(field.into(), json!(value));
                }
            }
        }
        "create_scene_from_selected_range" => {
            for field in ["label", "director_note", "scene_notes", "flux_notes", "nb_notes", "video_notes", "note"] {
                let value = clip(action.get(field), 5000);
                if !value.is_empty() {
                    cleaned.insert(field.into(), json!(value));
                }
            }
        }
        "split_selected_range_into_scenes" | "split_scene_into_subscenes" => {
            let prefix = clip(action.get("label_prefix"), 160);
            if !prefix.is_empty() {
                cleaned.insert("label_prefix".into(), json!(prefix));
            }
            for field in ["director_notes", "scene_notes"] {
                if let Some(list) = note_list(action, field) {
                    cleaned.insert(field.into(), list);
                }
            }
            if kind == "split_scene_into_subscenes" && scene_count.is_none_or(|n| n < 2) {
                return None;
            }
        }
        "merge_scenes" => {
            let listed = ["scene_numbers", "scenes"].iter().filter_map(|k| action.get(*k)).find(|v| truthy(v));
            let mut numbers: Vec<i64> = Vec::new();
            if let Some(Value::Array(items)) = listed {
                for number in items.iter().take(120).filter_map(to_int) {
                    if number > 0 && !numbers.contains(&number) {
                        numbers.push(number);
                    }
                }
            }
            for field in ["start_scene_number", "end_scene_number"] {
                if let Some(value) = whole_field(action.get(field)).filter(|v| *v > 0) {
                    cleaned.insert(field.into(), json!(value));
                }
            }
            let label = clip(action.get("label"), 160);
            if !label.is_empty() {
                cleaned.insert("label".into(), json!(label));
            }
            if !numbers.is_empty() {
                cleaned.insert("scene_numbers".into(), json!(numbers));
            }
            if numbers.len() < 2 && !(cleaned.contains_key("start_scene_number") && cleaned.contains_key("end_scene_number")) {
                return None;
            }
        }
        "normalize_dual_vocal_director_notes" => {
            let replacement = strip(action.get("replacement"));
            let replacement = if replacement.is_empty() { "male and female".to_string() } else { replacement };
            cleaned.insert("replacement".into(), json!(replacement.chars().take(160).collect::<String>()));
        }
        "replace_director_note_text" => {
            let (find, replace) = (either(action, &["find", "from"]), either(action, &["replace", "to"]));
            if find.is_empty() || replace.is_empty() {
                return None;
            }
            cleaned.insert("find".into(), json!(find.chars().take(500).collect::<String>()));
            cleaned.insert("replace".into(), json!(replace.chars().take(500).collect::<String>()));
        }
        "save_story_source" => {
            if raw_text.is_empty() {
                return None;
            }
        }
        "create_concept_prompts" | "create_motion_notes" => {
            let (modes, fallback): (&[&str], &str) = if kind == "create_concept_prompts" { (&["all", "director", "scene", "timeline", "director_scene"], "all") } else { (&["concept", "concept_director", "concept_timeline", "all"], "concept") };
            let source_mode = either(action, &["source_mode", "source"]).to_lowercase();
            let scope = strip(action.get("scope")).to_lowercase();
            let batch = ["batch_size", "batch"].iter().filter_map(|k| action.get(*k)).find(|v| truthy(v)).map_or(Some(5), to_int).unwrap_or(5);
            cleaned.insert("source_mode".into(), json!(if modes.contains(&source_mode.as_str()) { source_mode } else { fallback.to_string() }));
            cleaned.insert("scope".into(), json!(if matches!(scope.as_str(), "all" | "selected" | "range") { scope } else { "all".to_string() }));
            cleaned.insert("batch_size".into(), json!(batch.clamp(1, 10)));
            cleaned.insert("use_story".into(), json!(!matches!(action.get("use_story"), Some(Value::Bool(false)))));
            cleaned.insert("use_theme".into(), json!(!matches!(action.get("use_theme"), Some(Value::Bool(false)))));
        }
        "sync_existing_scenes_to_timeline_markers" => {
            cleaned.insert("create_missing".into(), json!(!matches!(action.get("create_missing"), Some(Value::Bool(false)))));
        }
        "set_scene_plan" => {
            if plan_values.is_empty() && raw_text.is_empty() {
                return None;
            }
            for (field, value) in plan_values {
                cleaned.insert(field.into(), json!(value));
            }
        }
        _ => {
            if NOTE_TYPES.contains(&kind) && !cleaned.contains_key("text") {
                let aliases: &[&str] = match kind {
                    "set_scene_notes" => &["scene_notes", "notes", "concept", "concept_prompt", "prompt"],
                    "set_flux_notes" => &["flux_notes", "notes", "prompt"],
                    "set_nb_notes" => &["nb_notes", "nano_banana_notes", "notes", "prompt"],
                    _ => &["video_notes", "i2v_notes", "motion_notes", "notes", "prompt"],
                };
                if let Some(found) = aliases.iter().map(|f| clip(action.get(*f), 5000)).find(|v| !v.is_empty()) {
                    cleaned.insert("text".into(), json!(found));
                }
            }
            if kind.starts_with("set_") && raw_text.is_empty() {
                let keep = match kind {
                    "set_image_model_mode" => cleaned.contains_key("image_mode"),
                    "set_video_model_mode" => cleaned.contains_key("video_mode"),
                    _ => NOTE_TYPES.contains(&kind) && cleaned.contains_key("text"),
                };
                return keep.then_some(cleaned);
            }
        }
    }
    Some(cleaned)
}

/// The map of a `json!({...})` literal.
fn object(value: Value) -> Obj {
    match value {
        Value::Object(map) => map,
        _ => Obj::new(),
    }
}

fn kind_of(action: &Obj) -> String {
    text_or_empty(action.get("type"))
}

fn without(actions: &mut Vec<Obj>, types: &[&str]) {
    actions.retain(|a| !types.contains(&kind_of(a).as_str()));
}

fn numbers_in(text: &str) -> Vec<i64> {
    rx(r"\d+").find_iter(text).flatten().map(|m| m.as_str().parse().unwrap_or(i64::MAX)).collect()
}

/// The scenes the user named to merge: "scenes 2 to 5" or "scenes 2, 3 and 4".
fn merge_targets(hint: &str) -> Vec<i64> {
    let mut numbers = Vec::new();
    if let Some(found) = rx(r"(?i)\bscenes?\s+(\d+)\s*(?:-|to|through)\s*(\d+)\b").captures(hint).ok().flatten() {
        let (start, end) = (found[1].parse::<i64>().unwrap_or(i64::MAX), found[2].parse::<i64>().unwrap_or(i64::MAX));
        // Not a range anyone means as a list of scenes: leave it to the sequence match.
        if start <= end && end - start < 10_000 {
            numbers = (start..=end).collect();
        }
    }
    if numbers.is_empty() {
        if let Some(found) = rx(r"(?i)\bscenes?\s+((?:\d+\s*(?:,|and)?\s*){2,})").captures(hint).ok().flatten() {
            numbers = numbers_in(&found[1]);
        }
    }
    numbers
}

/// `POST agent_chat`.
pub async fn builder_agent_reply(llm: &dyn Llm, payload: &Obj) -> Reply {
    let context = payload.get("context").and_then(Value::as_object).cloned().unwrap_or_default();
    let auto_apply = flag(payload, "auto_apply");
    let purpose = Some(text(payload, "agent_purpose").to_lowercase()).filter(|p| matches!(p.as_str(), "walkthrough" | "scene_work" | "story_builder" | "troubleshoot")).unwrap_or_else(|| "scene_work".into());
    let mut messages: Vec<(String, String)> = Vec::new();
    if let Some(Value::Array(items)) = payload.get("messages") {
        for item in &items[items.len().saturating_sub(10)..] {
            let Some(item) = item.as_object() else { continue };
            let role = strip(item.get("role")).to_lowercase();
            let content = strip(item.get("content"));
            if matches!(role.as_str(), "user" | "assistant") && !content.is_empty() {
                messages.push((role, content.chars().take(3000).collect()));
            }
        }
    }
    let latest_user = text(payload, "message");
    if !latest_user.is_empty() {
        messages.push(("user".into(), latest_user.chars().take(3000).collect()));
    }
    if messages.is_empty() {
        return Err("Type a message for the Builder Agent first.".into());
    }
    let context_text: String = dumps(&Value::Object(context)).chars().take(9000).collect();
    let conversation = messages.iter().map(|(role, content)| format!("{}:\n{content}", if role == "user" { "User" } else { "Assistant" })).collect::<Vec<_>>().join("\n\n");
    let instruction = format!(
        "{HEAD}Agent purpose: {purpose}.\n{BODY}Agent mode: {}\n{TAIL}Active context JSON:\n{context_text}\n\nConversation:\n{conversation}\n\nReturn the JSON now.",
        if auto_apply { AUTO_APPLY } else { MANUAL }
    );
    let call = TextCall::new(instruction, number(payload, "temperature", 0.55)?, number(payload, "top_p", 0.95)?, whole(payload, "max_new_tokens", 500)?, "Builder Agent").paragraphs();
    let (reply_text, info) = llm.text(call).await?;
    let raw = clean_plain_text(&reply_text);
    let parsed = extract_json_object(&raw).unwrap_or_else(|_| json!({"reply": raw, "actions": []}));
    let Value::Object(parsed) = parsed else { return Err(format!("'{}' object has no attribute 'get'", py_type_name(&parsed))) };
    let mut reply = clean_plain_text(&text_or_empty(parsed.get("reply")));
    let mut actions: Vec<Obj> = Vec::new();
    if let (Some(Value::Array(items)), true) = (parsed.get("actions"), auto_apply) {
        let limit = if purpose == "story_builder" { 60 } else { 12 };
        actions = items.iter().take(limit).filter_map(Value::as_object).filter_map(clean_action).collect();
    }

    let hint = messages.iter().rev().take(6).rev().map(|(_, content)| content.as_str()).chain(Some(reply.as_str()).filter(|r| !r.is_empty())).collect::<Vec<_>>().join("\n\n");
    let latest = Some(latest_user.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| messages[messages.len() - 1].1.clone());
    let merge_words = rx(r"(?i)\b(combine|merge|join|consolidate)\b");
    let split_words = rx(r"(?i)\b(split|break|divide|sub[-\s]?scenes?|sub[-\s]?segments?)\b");
    let found = |re: &Regex, text: &str| re.is_match(text).unwrap_or(false);
    let latest_merge = found(&merge_words, &latest);
    let latest_split = found(&split_words, &latest);
    let note_intent = found(&rx(r"(?i)\b(director\s+notes?|notes?|reword|rename|text)\b"), &latest);
    let dual_intent = note_intent && found(&rx(r"(?i)\b(male\s+and\s+female|both\s+(?:characters|singers|vocals?)|dual[-\s]?vocal)\b"), &latest);
    if auto_apply && note_intent {
        without(&mut actions, &["sync_existing_scenes_to_timeline_markers", "split_selected_range_into_scenes", "split_scene_into_subscenes", "merge_scenes"]);
    }
    if auto_apply && dual_intent && !actions.iter().any(|a| kind_of(a) == "normalize_dual_vocal_director_notes") {
        let exact = rx(r#"(?i)\b(?:director\s+notes?|director\s+note|note)\s+says?\s+["']([^"']+)["'].*?\b(?:changed?\s+to|to|instead)\s+["']([^"']+)["']"#).captures(&latest).ok().flatten();
        if let Some(exact) = exact.filter(|_| !actions.iter().any(|a| kind_of(a) == "replace_director_note_text")) {
            actions.push(object(json!({"type": "replace_director_note_text", "find": exact[1].trim(), "replace": exact[2].trim()})));
        }
        let replacement = rx(r#"(?i)\b(?:says?|to|instead|changed?\s+to)\s+["']?((?:fe)?male\s+and\s+(?:fe)?male)["']?"#).captures(&latest).ok().flatten().map_or_else(|| "male and female".to_string(), |m| m[1].trim().to_lowercase());
        actions.push(object(json!({"type": "normalize_dual_vocal_director_notes", "replacement": replacement})));
    }
    if auto_apply && latest_split {
        without(&mut actions, &["merge_scenes"]);
    }
    if auto_apply && latest_merge {
        without(&mut actions, &["split_scene_into_subscenes", "split_selected_range_into_scenes"]);
    }
    let agreed = rx(r"(?i)\A\s*(?:yes|yep|yeah|ok|okay|please do it|do it|go ahead|sure)\.?\s*\z").is_match(&latest).unwrap_or(false);
    let merge_intent = latest_merge || (!latest_split && agreed && found(&merge_words, &hint));
    if auto_apply && merge_intent {
        without(&mut actions, &["split_scene_into_subscenes", "split_selected_range_into_scenes"]);
        if !actions.iter().any(|a| kind_of(a) == "merge_scenes") {
            let named = merge_targets(&hint);
            if named.len() >= 2 {
                let mut scenes: Vec<i64> = Vec::new();
                for number in named {
                    if !scenes.contains(&number) {
                        scenes.push(number);
                    }
                }
                actions.push(object(json!({"type": "merge_scenes", "scene_numbers": scenes, "label": format!("Scene {}", scenes[0])})));
                if reply.is_empty() {
                    reply = format!("Merging scenes {}.", scenes.iter().map(i64::to_string).collect::<Vec<_>>().join(", "));
                }
            }
        }
    }
    if auto_apply && actions.is_empty() {
        let split_hint = if latest_split { latest.as_str() } else { hint.as_str() };
        if !merge_intent && found(&split_words, split_hint) {
            let scenes: Vec<i64> = rx(r"(?i)\bscene\s+(\d+)\b").captures_iter(split_hint).flatten().map(|c| c[1].parse().unwrap_or(i64::MAX)).collect();
            let mut counts: Vec<i64> = Vec::new();
            for pattern in [r"(?i)\b(?:into|in|to)\s+(\d+)\s*(?:sub[-\s]?scenes?|scenes?|segments?)\b", r"(?i)\b(\d+)\s*(?:sub[-\s]?scenes?|sub[-\s]?segments?)\b"] {
                counts.extend(rx(pattern).captures_iter(split_hint).flatten().map(|c| c[1].parse::<i64>().unwrap_or(i64::MAX)));
            }
            if let Some(only) = rx(r"\A\s*(\d{1,2})\s*\z").captures(&latest_user).ok().flatten() {
                counts.push(only[1].parse().unwrap_or(i64::MAX));
            }
            if let (Some(scene), Some(count)) = (scenes.last(), counts.last()) {
                let count = (*count).clamp(2, 24);
                actions.push(object(json!({"type": "split_scene_into_subscenes", "scene_number": scene, "scene_count": count, "label_prefix": format!("Scene {scene}")})));
                if reply.is_empty() {
                    reply = format!("Splitting Scene {scene} into {count} sub-scenes.");
                }
            }
        }
    }
    if reply.is_empty() {
        reply = if actions.is_empty() { "I can help with that." } else { "Done." }.to_string();
    }
    let mut out = Obj::new();
    out.insert("reply".into(), json!(reply));
    out.insert("actions".into(), Value::Array(actions.into_iter().map(Value::Object).collect()));
    out.extend(info);
    Ok(Value::Object(out))
}
