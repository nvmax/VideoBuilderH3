//! The instruction texts of the Story Brief and the Story Arc. Port of the story brief and story arc functions in `llm/prompts/storyboard.py`.

use serde_json::{json, Value};

fn or_text<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() { default } else { value }
}

/// `_storyboard_script_story_instruction`: the Story Brief (`purpose` "brief") or the Story Arc ("premise") around a locked, imported script.
pub fn script_story_instruction(purpose: &str, story_idea: &str, subjects_json: &str, locations_json: &str, script_text: &str, compact_plan_json: &str) -> String {
    let task = if purpose == "brief" {
        "Create a compact short-film production brief that Guided Film Automation can use to design visual scenes around the authoritative script. Use these headings exactly: Story premise:, Character dynamics:, Visual progression:, Continuity rules:, Scene-direction guidance:. Keep the complete response under 450 words."
    } else {
        "Create one cohesive short-film premise and visual narrative arc from the authoritative script. Explain the dramatic situation, character goals, emotional progression, and how the film can develop visually across the supplied timed segments. Output plain prose under 500 words with no screenplay rewrite and no dialogue list."
    };
    format!(
        "You are a short-film development director working from a locked screenplay.\n\n{task}\n\nNON-NEGOTIABLE SCRIPT CONTRACT:\n- The supplied dialogue is authoritative and immutable. Never rewrite, paraphrase, shorten, extend, reorder, merge, or invent spoken words.\n- Do not add narration, voice-over, replacement dialogue, or extra speakers.\n- Develop only the visual story around the locked dialogue: actions, reactions, motivations, blocking, locations, props, shots, camera language, atmosphere, and continuity.\n- Use Reference Builder character names and descriptions as identity authority.\n- Use only supplied Reference Builder locations when locations are available.\n\nOptional user story idea:\n{}\n\nReference Builder characters:\n{subjects_json}\n\nReference Builder locations:\n{locations_json}\n\nAuthoritative exact script:\n{script_text}\n\nAuthoritative timed segment plan:\n{compact_plan_json}",
        or_text(story_idea, "[none]")
    )
}

/// `_storyboard_story_brief_instruction`: the compact brief built from the lyrics, the story arc and the scene map.
pub fn story_brief_instruction(lyric_story_strength_guidance_text: &str, user_story_arc: &str, lyrics: &str, compact_scenes_json: &str) -> String {
    format!(
        "You are a music video story planner.\nCreate a compact story brief that can guide per-scene video prompts without sending the full lyrics every time.\n\nRules:\n- Use the user story arc as the strongest direction when it exists.\n- Use the lyrics and song sections to infer emotional progression, recurring symbols, visual motifs, and character journey.\n- Do not summarize every lyric line.\n- Do not quote long lyric sections.\n- Keep it useful for music-video scene prompting.\n- Output plain text only, no markdown table.\n- Keep it under 250 words.\n\n{lyric_story_strength_guidance_text}\n\nInclude these compact headings exactly:\nStory premise:\nEmotional arc:\nVisual motifs:\nScene guidance:\n\nUser story arc:\n{}\n\nFull/pasted lyrics:\n{}\n\nScene lyric map:\n{compact_scenes_json}",
        or_text(user_story_arc, "[none]"),
        or_text(lyrics, "[not provided]"),
    )
}

/// `_storyboard_story_arc_structure_instruction`: follow the lyric's section headers exactly, or infer a structure.
pub fn arc_structure_instruction(required_section_labels: &[String]) -> String {
    if required_section_labels.is_empty() {
        return "The reference lyrics do not contain explicit section headers. Infer a sensible compact song structure from lyrical and narrative changes. Do not add an Intro or Outro unless the supplied material clearly supports one.".into();
    }
    let labels: Vec<String> = required_section_labels.iter().map(|label| format!("- {label}")).collect();
    format!(
        "The reference lyrics contain explicit section headers. Preserve exactly this section order and these output headings; do not add, remove, merge, rename, or invent sections:\n{}\nRepeated sections have been numbered by occurrence so every real section has its own summary.",
        labels.join("\n")
    )
}

/// `_storyboard_story_arc_motion_guidance`: how physically active the singer is, by the character motion speed setting.
pub fn arc_motion_guidance(character_motion_speed: i64) -> &'static str {
    if character_motion_speed <= 2 {
        "Character motion level: mostly still. The singer may hold poses, but each section still needs a visible micro-action such as reaching, turning, touching fabric, shifting weight, raising a hand, or interacting with one object."
    } else if character_motion_speed <= 5 {
        "Character motion level: moderate. Give the singer controlled performance movement: steps, turns, gestures, changes in blocking, and occasional interaction with the set."
    } else if character_motion_speed <= 8 {
        "Character motion level: active. The singer should usually move through the location, walk, approach, retreat, touch objects, use architecture, cross rooms, lean into weather, or physically interact with the environment."
    } else {
        "Character motion level: highly active. Build big physical beats: dancing, running, climbing, struggling, sweeping gestures, forceful environmental interaction, or kinetic performance movement."
    }
}

/// `_storyboard_timeline_note_story_instruction`: how timed user directions constrain the story.
pub fn timeline_note_story_instruction(notes_json: &str) -> String {
    format!(
        "\n\nUSER TIMELINE NOTES \u{2014} STORY EVENT AND TIMING GUIDANCE:\nUse every supplied note to shape the story progression. Start/end values are seconds on the project timeline. Apply a range note to its overlapping scene_numbers and lyric sections. A null end marks an event at start, not a direction for the whole song. Do not move a timed event to unrelated sections. If no scene times are available, use the supplied timestamps as ordering guidance; do not invent exact scene timing. Preserve the required lyric-section headings and order, mapped cast and locations, and exact imported dialogue. Translate the notes into visible story action; keep technical/audio context out of the visual arc. Do not print this JSON or extra Timeline Note headings in the response.\n{notes_json}"
    )
}

/// The inputs of `story_arc_instruction`.
pub struct ArcInput<'a> {
    pub required_section_labels: &'a [String],
    pub section_word_limit: i64,
    pub story_arc_seed: &'a str,
    pub camera_flow: &'a str,
    pub camera_motion_speed: i64,
    pub character_motion_speed: i64,
    pub performance_style: &'a str,
    pub facial_performance: &'a str,
    pub lyric_story_strength_guidance_text: &'a str,
    pub story_idea: &'a str,
    pub previous_story_arc: &'a str,
    pub style_theme: &'a str,
    pub lyrics_source: &'a str,
    pub lyrics: &'a str,
    pub compact_scenes_json: &'a str,
    pub subjects_json: &'a str,
    pub locations_json: &'a str,
    pub song_story_brief: &'a str,
}

const ARC_RULES: &str = r#"* Use the entire section, not only its first lyric line.
* Do not summarize the lyrics line by line.
* Write like a music video director's shot list. Describe what the camera sees. Do not describe feelings, memories, or symbolism.
* Use the character names from the Character descriptions, not pronouns.
* Each scene in the Scene lyric map lists its cast. Write each section using only the cast of that section's scenes. Do not add unnamed people, crowds, or characters who are not in those scenes.
* The main character or singer should not default to standing still, standing alone, staring, looking, being framed, or holding a pose.
* Unless the character motion level is very low, every section must include a distinct physical action by the singer or main character.
* Vary the action between sections. Avoid repeating stand, stare, gaze, look, walk, or turn as the only beat.
* Make the location support the action; do not let the location be the whole story beat.
* If Location descriptions are provided, use only those locations as the physical settings for the arc.
* Do not invent warehouses, loading docks, corridors, steel stairs, metal doors, concrete halls, or other industrial spaces unless those are explicitly present in the provided Location descriptions.
* To create variety, change subject actions, camera energy, props, lighting, blocking, and use of the mapped locations instead of inventing unrelated places.
* The Scene lyric map may include mapped_extras. Use those exact extras only in the scenes where they are mapped, and use each interaction role to shape the section's larger action progression.
* Plan recurring extra relationships across sections when the same named extra returns, especially direct, dancing_with, and alongside roles. Background and background_dancing extras may support group progression without becoming principal singers.
* Extras never sing, speak, or receive dialogue unless the scene mapping explicitly identifies them as a vocal subject elsewhere. Do not move an extra into an unmapped scene.
* Never force a standard pop-song template over explicit lyric section headers.
* When a lyric header line contains several bracketed tags, only the required section heading listed above is structural. Tags such as [Whispered], [High Energy], [Dark Atmosphere], and [Explosive] are performance or mood notes, not output headings. [End] is only an end marker.
* Values such as [instrumental], [music only], or [no vocals] inside the Scene lyric map are timing/content markers, not additional song-section headings. Cover their visuals inside the nearest listed required section and never output those markers as separate headings.
* If only lyrics are provided, build the arc from the lyrics.
* If no lyrics are provided, build the arc from the theme or story idea.
* Do not ask follow-up questions unless absolutely necessary.
* Output only the story arc sections. No intro note, no markdown table, no JSON.

"#;

/// `_storyboard_story_arc_instruction`: the main Story Arc prompt (pass 1, one paragraph for each lyric section).
pub fn story_arc_instruction(i: &ArcInput) -> String {
    let structure = arc_structure_instruction(i.required_section_labels);
    let brief_block = if i.song_story_brief.is_empty() { String::new() } else { format!("Song story brief:\n{}\n\n", i.song_story_brief) };
    format!(
        "You are a music video story arc generator.\n\nYour job is to take song lyrics and turn them into a simple, short story arc for a music video.\n\nThe user may provide:\n* Song lyrics\n* Story idea (optional)\n* Style/theme (optional)\n* Character descriptions\n* Location descriptions\n\nAll inputs are optional. If something is missing, make a strong creative choice and continue.\n\nYour output should be clean, complete, and easy to use. Break it down by the actual song structure.\n\n{structure}\n\nFormat every section as its heading on one line ending in a colon, followed by one prose paragraph.\n\nEach section paragraph covers, in this order:\n1. Setting: the mapped location, with one light or weather detail taken from its description.\n2. Action: one continuous physical action by the main character.\n3. Blocking: where the character is in the space (foreground, midground, background) and how they move through it.\n4. Small movement: one visible movement such as breath, hands, gaze, or posture.\n5. Carry: the object, position, or light that passes into the next section.\n\nRules:\n* Fully summarize the visual story progression of each section in no more than {} words.\n{ARC_RULES}Creative variation seed: {}\nUse this seed as a reroll key. If the user regenerates the story arc with a different seed, choose a meaningfully different visual interpretation, section action pattern, and location usage while still respecting the same subjects, locations, lyrics, and style.\n\nScene default style settings:\n- Camera flow: {}\n- Camera motion speed: {}/10\n- Character motion speed: {}/10\n- Performance style: {}\n- Facial performance: {}\n\n{}\n\n{}\n\nStory idea:\n{}\n\n{brief_block}Previous generated story arc to avoid copying:\n{}\n\nIf a previous generated story arc is provided, do not preserve its specific locations, set pieces, or section actions. Use it only as a negative example of what should change on this reroll.\n\nStyle/theme:\n{}\n\nCharacter descriptions:\n{}\n\nLocation descriptions:\n{}\n\nAuthoritative lyric source: {}\nFull reference lyrics:\n{}\n\nScene lyric map:\n{}",
        i.section_word_limit,
        or_text(i.story_arc_seed, "[none]"),
        or_text(i.camera_flow, "[not provided]"),
        i.camera_motion_speed,
        i.character_motion_speed,
        or_text(i.performance_style, "[not provided]"),
        or_text(i.facial_performance, "[not provided]"),
        arc_motion_guidance(i.character_motion_speed),
        i.lyric_story_strength_guidance_text,
        or_text(i.story_idea, "[not provided]"),
        or_text(i.previous_story_arc, "[not provided]"),
        or_text(i.style_theme, "[not provided]"),
        i.subjects_json,
        i.locations_json,
        i.lyrics_source,
        or_text(i.lyrics, "[not provided]"),
        i.compact_scenes_json,
    )
}

/// The inputs of `scene_entry_instruction`.
pub struct EntryInput<'a> {
    pub section_label: &'a str,
    pub section_text: &'a str,
    pub scene_number: i64,
    pub location_name: &'a str,
    pub location_description: &'a str,
    pub cast_wall: &'a str,
    pub cast_names: &'a [String],
    pub lyric: &'a str,
    pub previous_entry: &'a str,
    pub camera_flow: &'a str,
    pub camera_motion_speed: i64,
    pub character_motion_speed: i64,
    pub word_limit: i64,
    pub story_arc_seed: &'a str,
}

/// `_storyboard_story_arc_scene_entry_instruction`: pass 2, one short note for one scene. The code adds the "Scene N (Location) \u{2014}" prefix.
pub fn scene_entry_instruction(i: &EntryInput) -> String {
    let who = if i.cast_names.is_empty() { "no one".to_string() } else { i.cast_names.join(", ") };
    format!(
        "You write one scene note for a music video shot list.\n\n{}\n\nSong section: {}\nScene number: {}\nLocation: {}\nLocation description: {}\nLyric line (context only, do not quote): {}\nSection story so far:\n{}\n\nPrevious scene note:\n{}\n\nCamera flow: {}\nCamera motion speed: {}/10\nCharacter motion speed: {}/10\n{}\n\nWrite 1 to 2 sentences, no more than {} words. Use this order:\n1. One camera idea (shot size and one move) that fits the motion speeds above.\n2. What the people listed above ({who}) physically do, in one continuous action, ending in a position the next scene can start from.\n3. One small visible movement (breath, hands, gaze, posture).\n4. One light or set detail from the location description.\n\nRules:\n- Describe what the camera sees. Do not describe feelings, memories, or symbolism.\n- Stay inside the location above. Do not invent another place.\n- Use names, not pronouns. Do not add any person who is not named in the people list above.\n- Do not repeat the previous scene note's action.\n- Output only the note text. No label, no scene number, no quotes.\nVariation seed: {}",
        i.cast_wall,
        i.section_label,
        i.scene_number,
        or_text(i.location_name, "[not provided]"),
        or_text(i.location_description, "[not provided]"),
        or_text(i.lyric, "[none]"),
        or_text(i.section_text, "[none]"),
        or_text(i.previous_entry, "[none]"),
        or_text(i.camera_flow, "[not provided]"),
        i.camera_motion_speed,
        i.character_motion_speed,
        arc_motion_guidance(i.character_motion_speed),
        i.word_limit,
        or_text(i.story_arc_seed, "[none]"),
    )
}

/// `_storyboard_story_arc_format_retry_instruction`: sent when the first reply loses the required headings.
pub fn arc_format_retry_instruction(required_section_labels: &[String], original_instruction: &str) -> String {
    let skeleton: Vec<String> = required_section_labels.iter().map(|label| format!("{label}:\n[one visual-story paragraph]")).collect();
    let first = required_section_labels.first().map(String::as_str).unwrap_or_default();
    format!(
        "CRITICAL FORMAT VALIDATION FAILURE:\nYour previous response violated the structural section heading contract.\n\nRULES FOR THIS REGENERATION:\n1. Your response must begin immediately on line 1 with: '{first}:'\n2. Include EVERY section heading listed below exactly once, in this exact sequence, followed by a colon.\n3. Write one prose paragraph under each heading.\n4. Absolutely no markdown lists, code fences, introductory sentences, or summary commentary.\n\nREQUIRED OUTPUT SKELETON:\n{}\n\nORIGINAL CONTEXT & PARAMETERS:\n{original_instruction}",
        skeleton.join("\n\n")
    )
}

/// `_storyboard_story_arc_schema`: one string for every required heading, in order.
pub fn arc_schema(required_section_labels: &[String]) -> Value {
    let properties: serde_json::Map<String, Value> = required_section_labels.iter().map(|label| (label.clone(), json!({"type": "string"}))).collect();
    json!({"type": "object", "properties": properties, "required": required_section_labels, "additionalProperties": false})
}

/// `_storyboard_story_arc_json_retry_instruction`: moves the heading structure into JSON keys.
pub fn arc_json_retry_instruction(original_instruction: &str) -> String {
    format!(
        "{original_instruction}\n\nOUTPUT FORMAT OVERRIDE: return a JSON object instead of heading lines. Use each required section heading as a key, in order, and write that section's visual-story paragraph as its value. Do not repeat the heading inside the value. Do not wrap the JSON in markdown code fences."
    )
}
