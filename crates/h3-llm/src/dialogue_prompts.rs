//! The instruction texts of the MiniMax short-film scene planner (Speaking mode, "Auto-Generate Scenes"). Port of the dialogue planner functions in
//! `llm/prompts/storyboard.py`, for the MiniMax profile only (the ID-LoRA profile belongs to LTX).

use serde_json::{json, Map, Value};

const IDENTITY: &str = "You are the dedicated MiniMax H3 short-film scene planner. Create model-ready scene cards for a dialogue-driven MiniMax project.\n\nCreate a preview storyboard plan. The user will review it before anything is applied to the Video Builder timeline.\n\nImportant behavior:\n";

const AUTHORITATIVE_RULE: &str = r#"AUTHORITATIVE SCRIPT CONTRACT:
- The SCRIPT MAPPER SEGMENT PLAN below is immutable. Return exactly one scene per supplied segment, in the same order.
- Copy every dialogue cue word-for-word with its exact character_id and speaker. Never rewrite, paraphrase, correct, shorten, extend, merge, reorder, or invent dialogue.
- Do not add narration, voice-over, new speakers, or extra spoken words.
- Your creative job is only the surrounding visual direction: story beat, actions, reactions, blocking, location choice, shot, camera, facial delivery, ambience, and continuity.
- Continuation segments must preserve the prior segment's character identity, wardrobe, location, props, spatial positions, and screen direction unless the locked script plan starts a new source scene.

"#;

const SOURCE_RULES: &str = r#"- If USER STORY / SCRIPT has text, use it as the source. It may be a premise, outline, or pasted script.
- If USER STORY / SCRIPT is empty, invent an original short-film premise from the available characters and locations.
- A scene may contain one or more ordered dialogue_cues. Use exact character ids from AVAILABLE CHARACTERS. Keep every cue short enough to fit naturally in one generated clip.
- When two or more characters speak in one scene, preserve their exact turn order in dialogue_cues and give each character only their own words.
- Use only location ids from AVAILABLE LOCATIONS when possible.
- Each scene needs a story beat, visual direction for image prep, a full text-to-image prompt, and optional camera/facial direction.
"#;

const CAMERA_RULES: &str = "Use controlled cinematic camera variation across the sequence. Do not default every scene to static or locked-off framing when camera speed is above 0. At camera speed 7-8, every camera_motion value must use energetic, visibly active wording and must not say slow, gentle, subtle, restrained, locked-off, static, or hold. At speed 9-10, prefer two coordinated readable camera actions. An inward move (push-in, dolly-in, zoom-in, track forward, or drift closer) is a rare accent: use at most one inward move in any six neighboring scenes. Never assign inward moves to alternating scenes. Prefer lateral drift, restrained pan, orbit, pull-back, rack focus, handheld hold, and intentional locked coverage. The requested shot_type is the literal first-frame scale: never begin wider or farther away and move inward to reach it. Reserve a static camera for an intentional dramatic beat and keep neighboring camera-motion families visibly different.\n";

const SCENE_RULES: &str = r#"- camera_motion must contain the actual concise camera direction. motion_summary is optional and must only contain additional custom motion direction that is not already stated in camera_motion; otherwise leave motion_summary empty.
- The image_prompt must follow the existing NanoBanana/Krea-style still-image prompt structure, not a short keyword list.
- For image_prompt, write one polished paragraph, about 65-115 words, practical for text-to-image generation.
- For image_prompt, include concrete subject identity, wardrobe, hair, makeup or facial detail when known, pose/body language, shot/framing, lens feel, lighting setup, environment, materials, atmosphere, color palette, texture, and cinematic finish.
- For image_prompt, create a still frame only. Do not describe animation, camera movement, future action, lip sync, audio, captions, text overlays, or printed dialogue.
- For image_prompt, when no shot is specified, vary the shot scale across scenes (wide, medium-wide, medium, waist-up, over-the-shoulder, low-angle) and do not default to close-ups. Use shallow depth of field, foreground framing, bokeh, rim light, and atmospheric lighting as fits.
- For image_prompt, if character or location reference images are available, start naturally with 'Using the provided character reference...' or 'Using the provided character reference and location reference...' and preserve the important identity/setting details without copying the exact pose, crop, or camera angle.
- Do not mention MiniMax H3, models, nodes, workflow files, voice cloning, prompts, or metadata in dialogue.
- Do not write markdown, explanations, or code fences.

Return only valid JSON with this exact shape:
{
  "title": "short title",
  "premise": "one paragraph premise",
  "scenes": [
    {
      "label": "Scene 1 title",
      "dialogue_cues": [{"character_id": "exact character id", "speaker": "character name", "dialogue": "exact spoken words"}],
      "location_id": "exact id from available locations or empty",
      "story_beat": "one concise story beat",
      "visual_direction": "short first-frame visual direction for image prep",
      "image_prompt": "full NanoBanana/Krea-style still image prompt paragraph for creating the scene image",
      "shot_type": "optional shot/framing",
      "camera_motion": "optional camera movement",
      "character_motion": "visible character blocking or action",
      "facial_performance": "optional facial/emotional direction",
      "delivery": "optional voice/performance delivery note",
      "audio_direction": "ambience, sound effects, silence, breathing, and other non-dialogue audio direction",
      "continuity": "identity, wardrobe, props, location, screen direction, and spatial continuity requirements"
    }
  ]
}

"#;

/// The inputs of `dialogue_planner_instruction`.
pub struct PlannerInput<'a> {
    pub has_authoritative_script: bool,
    pub camera_flow: &'a str,
    pub camera_motion_speed: i64,
    pub character_motion_speed: i64,
    pub scene_count: i64,
    pub story_source: &'a str,
    pub script_mapper_plan_json: &'a str,
    pub story_layer_json: &'a str,
    pub project_motion_settings_json: &'a str,
    pub subjects_json: &'a str,
    pub locations_json: &'a str,
    pub compact_existing_json: &'a str,
}

/// `_storyboard_dialogue_planner_instruction` for the MiniMax short-film profile: the persona, the dialogue and camera rules, the lock on an
/// imported script, and the JSON shape of the plan.
pub fn dialogue_planner_instruction(i: &PlannerInput) -> String {
    let authoritative = if i.has_authoritative_script { AUTHORITATIVE_RULE } else { "" };
    let source = if i.story_source.is_empty() { "[blank - invent an original short-film premise]" } else { i.story_source };
    format!(
        "{IDENTITY}{authoritative}{SOURCE_RULES}- Follow the project camera plan: camera flow is {} and camera motion speed is {}/10. {CAMERA_RULES}- Character motion speed is {}/10. At speed 4 or higher, every scene needs at least one clear physical body action, gesture, step, or interaction with the set; facial expression, blinking, breathing, and mouth movement alone do not count. Keep dialogue lip sync practical.\n{SCENE_RULES}Requested scene count: {}\n\nUSER STORY / SCRIPT:\n{source}\n\nSCRIPT MAPPER SEGMENT PLAN (authoritative when present):\n{}\n\nStory layer:\n{}\n\nProject motion settings:\n{}\n\nAvailable characters:\n{}\n\nAvailable locations:\n{}\n\nExisting starter scenes:\n{}",
        h3_core::pyval::repr_text(i.camera_flow),
        i.camera_motion_speed,
        i.character_motion_speed,
        i.scene_count,
        i.script_mapper_plan_json,
        i.story_layer_json,
        i.project_motion_settings_json,
        i.subjects_json,
        i.locations_json,
        i.compact_existing_json,
    )
}

/// `_storyboard_dialogue_plan_schema` (MiniMax): the plan as JSON, every field a required string, the dialogue as a list of cues.
pub fn dialogue_plan_schema() -> Value {
    let strings = |names: &[&str]| -> Map<String, Value> { names.iter().map(|n| (n.to_string(), json!({"type": "string"}))).collect() };
    let mut scene = strings(&["label", "location_id", "story_beat", "visual_direction", "image_prompt", "shot_type", "camera_motion", "character_motion", "facial_performance", "delivery", "audio_direction", "continuity"]);
    let cue = strings(&["character_id", "speaker", "dialogue"]);
    let cue_names: Vec<String> = cue.keys().cloned().collect();
    scene.insert("dialogue_cues".into(), json!({"type": "array", "items": {"type": "object", "properties": cue, "required": cue_names, "additionalProperties": false}}));
    let scene_names: Vec<String> = scene.keys().cloned().collect();
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string"},
            "premise": {"type": "string"},
            "scenes": {"type": "array", "items": {"type": "object", "properties": scene, "required": scene_names, "additionalProperties": false}},
        },
        "required": ["title", "premise", "scenes"],
        "additionalProperties": false,
    })
}

/// `_storyboard_dialogue_json_repair_instruction` (MiniMax): sent when the plan's JSON does not parse.
pub fn dialogue_json_repair_instruction(malformed_text: &str) -> String {
    format!(
        "Repair this malformed JSON for a MiniMax short-film dialogue scene plan.\nReturn only valid JSON. Do not add prose, markdown, code fences, comments, or trailing commas.\nEvery property name must be enclosed in double quotes. Every string value must be enclosed in double quotes.\nKeep the same title, premise, and scenes when possible.\n\nMALFORMED JSON:\n{malformed_text}"
    )
}
