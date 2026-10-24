//! The instruction texts of the Scene Story Beat and its repairs. Port of the beat functions in `llm/prompts/storyboard.py`. The role and rules
//! template is the pack's constant in `data/prompt_constants.json`.

use crate::prompts::text as constant;

fn or_none<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() { default } else { value }
}

/// `_storyboard_scene_beat_output_rules`: plain paragraph output, or the five field FLF endpoint JSON.
pub fn scene_beat_output_rules(flf_mode: bool, beat_word_limit: i64) -> String {
    if flf_mode {
        return format!(
            "Return valid JSON only with exactly these string keys: story_beat, flf_start_state, flf_transformation, flf_end_state, flf_carry_forward.\nThe story_beat is a concise compatibility summary under {beat_word_limit} words.\nflf_start_state describes the concrete visible opening image. If Previous FLF end state is provided, copy it exactly as flf_start_state; do not reinterpret or redesign it.\nflf_transformation describes one continuous, progressive visual change that expresses the CURRENT lyric.\nflf_end_state describes the concrete visible destination image reached by the end of the CURRENT lyric.\nflf_carry_forward records the subject, anatomy, wardrobe, props, setting, lighting, and transformation state that the next scene must inherit.\nThe current lyric is authoritative. Previous and next lyrics provide continuity only and must not replace or steal this scene's action.\nDo not include Markdown fences or any text outside the JSON object."
        );
    }
    format!("Output one short paragraph only, no label, no bullets.\nKeep it under {beat_word_limit} words.")
}

/// `render_scene_beat_rules`: the beat prompt role and rules with the scene's cast wall and output rules filled in.
pub fn render_scene_beat_rules(template: Option<&str>, cast_wall: &str, output_rules: &str) -> String {
    let template = template.filter(|t| !t.is_empty()).unwrap_or_else(|| constant("storyboard", "SCENE_BEAT_RULES_TEMPLATE"));
    let text = template.replace("{{cast_wall_block}}", &if cast_wall.is_empty() { String::new() } else { format!("{cast_wall}\n\n") }).replace("{{output_rules}}", output_rules);
    // A saved instruction loses its last line breaks. The prompt's own text always starts after a blank line.
    if text.ends_with("\n\n") { text } else { format!("{}\n\n", text.trim_end()) }
}

/// The inputs of `scene_beat_instruction`.
#[derive(Default)]
pub struct BeatInput<'a> {
    pub flf_mode: bool,
    pub beat_word_limit: i64,
    pub lyric_story_strength_guidance_text: &'a str,
    pub user_story_arc: &'a str,
    pub song_story_brief: &'a str,
    pub previous_beat: &'a str,
    pub previous_lyrics: &'a str,
    pub previous_end_state: &'a str,
    pub previous_carry_forward: &'a str,
    pub current_lyrics: &'a str,
    pub next_lyrics: &'a str,
    pub scene_defaults_json: &'a str,
    pub performance_assignment_json: &'a str,
    pub extra_subjects_json: &'a str,
    pub scene_json: &'a str,
    pub cast_wall: &'a str,
    pub story_mode: bool,
    pub rules_template: Option<&'a str>,
}

/// `_storyboard_scene_beat_instruction`: the main Scene Story Beat prompt. It keeps the beat purely visual, respects the scene defaults, the
/// mapped extras and the mapped location.
pub fn scene_beat_instruction(i: &BeatInput) -> String {
    let output_rules = scene_beat_output_rules(i.flf_mode, i.beat_word_limit);
    // Story Mode: the story comes from the scene plan. The lyric only sets mood and emotion.
    let lyric_label = if i.story_mode { "CURRENT scene lyric text (mood and emotion only, never events or places)" } else { "CURRENT scene lyric text (main authority)" };
    let rules = render_scene_beat_rules(if i.story_mode { i.rules_template } else { None }, i.cast_wall, &output_rules);
    format!(
        "{rules}{}\n\nUser Story Arc:\n{}\n\nSong Story Brief:\n{}\n\nPrevious scene beat:\n{}\n\nPrevious scene lyric text (continuity only):\n{}\n\nPrevious FLF end state (required opening state when present):\n{}\n\nPrevious FLF carry-forward constraints:\n{}\n\n{lyric_label}:\n{}\n\nNext scene lyric text:\n{}\n\nScene defaults and motion direction (authoritative when supplied):\n{}\n\nPerformance assignment metadata (do not mention this in the story beat; the \"singing\" list is in singing order, so the first name sings first and the next ones come in after):\n{}\n\nMapped extras and exact scene roles:\n{}\n\nSelected scene JSON:\n{}{}\n\nFINAL SCENE-BEAT OVERRIDE \u{2014} FOLLOW THIS LAST:\nReturn only visual information: setting, visible actions, blocking, props, small human movements, camera idea, light, and continuity. Use only the people listed in PEOPLE IN THIS SCENE. Exclude all audio, lyric, vocal, singing, lip-sync, and performance-delivery language.",
        i.lyric_story_strength_guidance_text,
        or_none(i.user_story_arc, "[none]"),
        or_none(i.song_story_brief, "[none]"),
        or_none(i.previous_beat, "[none]"),
        or_none(i.previous_lyrics, "[none]"),
        or_none(i.previous_end_state, "[none \u{2014} this is the first scene]"),
        or_none(i.previous_carry_forward, "[none]"),
        or_none(i.current_lyrics, "[none]"),
        or_none(i.next_lyrics, "[none]"),
        i.scene_defaults_json,
        i.performance_assignment_json,
        i.extra_subjects_json,
        i.scene_json,
        if i.cast_wall.is_empty() { String::new() } else { format!("\n\n{}", i.cast_wall) },
    )
}

/// `_storyboard_scene_beat_audio_language_repair_instruction`: sent when the beat leaked singing, lyric or vocal wording.
pub fn audio_language_repair_instruction(original_text: &str) -> String {
    format!(
        "Rewrite this scene story beat as visual narrative guidance only. Preserve its setting, visible actions, props, emotion, symbolism, and continuity. Remove every reference to singing, lyrics, vocals, rapping, lip-sync, music, instrumental sections, dialogue delivery, or audio/performance metadata. Do not replace those references with a statement that the subject is silent; simply describe the visible action. Output one concise paragraph only, with no label or bullets.\n\nOriginal scene beat:\n{original_text}"
    )
}

/// `_storyboard_scene_beat_location_repair_instruction`: sent when the beat drifted onto a location that is not mapped.
pub fn location_repair_instruction(location_context: &str, drift_terms: &[String], original_text: &str) -> String {
    format!(
        "Rewrite the scene story beat so it obeys the mapped location.\n\nHard rules:\n- Keep the same emotional purpose and subject energy.\n- Use only the mapped location as the physical setting.\n- Remove every incompatible place/object listed below.\n- Do not mention a warehouse, loading dock, industrial corridor, metal door, concrete hall, steel stairs, pipes, or massive window unless those details are explicitly in the mapped location.\n- Output one short paragraph only, under 80 words.\n\nMapped location:\n{}\n\nIncompatible leaked location terms:\n{}\n\nOriginal scene beat:\n{original_text}",
        or_none(location_context, "[none]"),
        drift_terms.join(", "),
    )
}

/// `_storyboard_feeling_word_repair_instruction`: sent when a note leans on feeling words instead of what the camera sees.
pub fn feeling_word_repair_instruction(feeling_terms: &[String], original_text: &str, word_limit: i64) -> String {
    format!(
        "Rewrite this music video note so it only describes what the camera sees.\n\nHard rules:\n- Remove feeling words, memories, and symbolism.\n- Show mood only as a visible movement such as breath, gaze, hands, or posture.\n- Keep the same people, the same location, and the same action. Do not add any person, hand, or shadow.\n- Keep one camera idea, one small human movement, and one light or set detail.\n- Output the note only, under {word_limit} words.\n\nWords to remove: {}\n\nOriginal note:\n{original_text}",
        feeling_terms.join(", "),
    )
}

/// `_storyboard_scene_beat_cast_repair_instruction`: sent when the beat refers to a character who is not selected for the scene.
pub fn cast_repair_instruction(cast_wall: &str, leaked_terms: &[String], original_text: &str) -> String {
    format!(
        "Rewrite the scene story beat so it only involves the characters selected for this scene.\n\nHard rules:\n- Keep the same location, mood, and emotional purpose.\n- Remove every mention of a character who is not in the scene cast, including names, pronouns, hands, shadows, reflections, voices, and off-screen presence.\n- Do not replace a removed character with another person or a vague companion.\n- Output one short paragraph only, under 80 words.\n\n{cast_wall}\n\nTerms that must not appear:\n{}\n\nOriginal scene beat:\n{original_text}",
        leaked_terms.join(", "),
    )
}

/// `_storyboard_scene_beat_extra_mapping_repair_instruction`: sent when the beat left out one or more mapped extras.
pub fn extra_mapping_repair_instruction(extra_subjects_json: &str, original_text: &str) -> String {
    format!(
        "Rewrite this music-video scene beat so it includes every mapped extra by exact name and gives each the assigned action/blocking role.\n\nHard rules:\n- Preserve the original narrative purpose, mapped location, main subject action, and emotional progression.\n- Include every exact extra name from the mapping.\n- Apply each interaction role exactly. Group only extras sharing background or background_dancing.\n- Extras do not sing, speak, or receive speaker IDs.\n- Use identity details only when required to distinguish characters; do not copy wardrobe biographies.\n- Output one concise paragraph only, under 100 words, with no label or bullets.\n\nMapped extras:\n{extra_subjects_json}\n\nOriginal scene beat:\n{original_text}"
    )
}

/// `_storyboard_image_world_style_contract`: appended to the text-to-image instructions of every scene. It says which Image World Style governs
/// the visual world of the still image.
pub fn image_world_style_contract(image_world_style: &str, custom_direction: &str) -> String {
    let presets = crate::prompts::constant("storyboard", "_STORYBOARD_IMAGE_WORLD_STYLE_PRESETS");
    let pick = |key: &str| presets.get(key).and_then(serde_json::Value::as_str);
    let style = pick(image_world_style).or_else(|| pick("natural")).unwrap_or("");
    let custom = if custom_direction.is_empty() { String::new() } else { format!("- User's custom visual direction: {custom_direction}\n") };
    format!(
        "\n\nGLOBAL IMAGE WORLD STYLE CONTRACT:\n- {style}\n{custom}- Apply this contract to the complete frame, not only the main subject. Preserve required scene content and endpoint facts while expressing them through this style.\n- Do not mention this contract, preset names, or workflow settings in the final prompt."
    )
}

/// `_storyboard_flf_endpoint_instruction`: appended when a First/Last Frame endpoint still image is written instead of a normal scene image.
pub fn flf_endpoint_instruction(flf_image_target: &str, target_state: &str, transformation: &str, carry_forward: &str) -> String {
    let context = if flf_image_target == "start" {
        "- This is strictly the untouched opening condition before the scene action begins.\n- Do not include, foreshadow, partially reveal, or imply the later transformation, destination anatomy, destination objects, or completed action.\n- Ignore transformation, end-state, and carry-forward fields when writing this START image.\n- Lyrics and the general story beat may guide mood only; do not include any lyric/story action that is not already explicitly visible in the required opening state.\n".to_string()
    } else {
        format!("- Planned transformation context: {}\n- Carry-forward continuity: {}\n", or_none(transformation, "[none]"), or_none(carry_forward, "[none]"))
    };
    format!(
        "\n\nFIRST / LAST FRAME STILL-IMAGE RULES:\n- You are writing the {} endpoint still image, not a video prompt.\n- Required visible endpoint state: {}\n{context}- Make the required endpoint state visually concrete in one frozen image.\n- Preserve mapped subject identity, wardrobe, environment, lighting, and established anatomy unless the required endpoint explicitly changes one of them.\n- Do not describe motion over time, a transition, morphing process, first/last frames, or workflow instructions in the final image prompt.\n- Output only the image prompt.",
        flf_image_target.to_uppercase(),
        or_none(target_state, "[use the scene card literally]"),
    )
}
