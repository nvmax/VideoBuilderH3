//! The `MiniMax H3 shot-description task` text the creative LLM receives for a scene. Port of `build_shot_task` in
//! `minimax/shot_prompt.py`.

use serde::Deserialize;

use super::continuation::continuation_task_text;
use super::cues::{cue_shot_contract, CueRow};
use super::frame::{character_budget, RefLabel};
use super::lyrics::lyric_chunks;
use super::plan::{cut_plan_instruction, performance_craft_instruction, shot_plan};
use super::rx::{rx, sub};
use super::text::{literal_scene_instruction, motion_energy_text, scene_timing_instruction};
use crate::pyval::{fmt_g, round_to, take_chars, Obj};

fn default_target_limit() -> i64 {
    7000
}

/// Inputs of `build_shot_task`. Every field has the pack's default, so a caller sets only what differs.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ShotTaskArgs {
    pub mode_label: String,
    pub duration: f64,
    pub aspect_ratio: String,
    pub audio_mode: String,
    pub cut_plan: Obj,
    pub style: String,
    pub labels: Vec<RefLabel>,
    pub camera_speed: f64,
    pub character_speed: f64,
    pub camera_guidance: String,
    pub character_guidance: String,
    pub lyric_text: String,
    pub visual_only: bool,
    pub no_character: bool,
    pub story_beat: String,
    pub lyric_section: String,
    pub scene_notes: String,
    pub subject_text: String,
    pub location_text: String,
    pub seed: String,
    #[serde(default = "default_target_limit")]
    pub target_limit: i64,
    pub continuation: Option<Obj>,
    pub previous_shot: String,
    pub with_picture: bool,
    pub location_contract: String,
    pub motion_request: String,
    pub audio_direction: String,
    pub continuity: String,
    pub storyboard_context: String,
    pub cues: Option<Vec<CueRow>>,
}

impl Default for ShotTaskArgs {
    fn default() -> Self {
        Self {
            mode_label: String::new(),
            duration: 0.0,
            aspect_ratio: String::new(),
            audio_mode: String::new(),
            cut_plan: Obj::new(),
            style: String::new(),
            labels: Vec::new(),
            camera_speed: 0.0,
            character_speed: 0.0,
            camera_guidance: String::new(),
            character_guidance: String::new(),
            lyric_text: String::new(),
            visual_only: false,
            no_character: false,
            story_beat: String::new(),
            lyric_section: String::new(),
            scene_notes: String::new(),
            subject_text: String::new(),
            location_text: String::new(),
            seed: String::new(),
            target_limit: default_target_limit(),
            continuation: None,
            previous_shot: String::new(),
            with_picture: false,
            location_contract: String::new(),
            motion_request: String::new(),
            audio_direction: String::new(),
            continuity: String::new(),
            storyboard_context: String::new(),
            cues: None,
        }
    }
}

impl<'de> Deserialize<'de> for RefLabel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            label: String,
            picture: String,
            kind: String,
            #[serde(default)]
            name: String,
        }
        let r = Raw::deserialize(d)?;
        Ok(RefLabel { label: r.label, picture: r.picture, kind: r.kind, name: r.name })
    }
}

fn compact(value: &str, limit: usize) -> String {
    let text = sub(rx!(r"\s+"), value, " ").trim().to_string();
    if text.chars().count() > limit {
        format!("{}...", take_chars(&text, limit).trim_end())
    } else {
        text
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub fn build_shot_task(a: &ShotTaskArgs) -> String {
    let plan = shot_plan(&a.cut_plan);
    let exact = fmt_g(round_to(a.duration, 3));
    let budget = character_budget(&a.cut_plan, &a.style, a.target_limit);
    let per_shot = (budget.shot_chars / (plan.len().max(1) as i64)).max(1);
    let cast: Vec<&RefLabel> = if a.no_character {
        Vec::new()
    } else {
        a.labels.iter().filter(|l| l.kind == "subject" || l.kind == "extra").collect()
    };
    let labelize = |value: &str| -> String {
        let mut text = value.to_string();
        for item in &cast {
            if !item.name.is_empty() {
                if let Ok(re) = fancy_regex::Regex::new(&format!(r"(?i)\b{}\b", fancy_regex::escape(&item.name))) {
                    text = re.replace_all(&text, fancy_regex::NoExpand(item.label.as_str())).into_owned();
                }
            }
        }
        text
    };
    let built_in = a.audio_mode == "built_in_audio";

    let mut parts: Vec<String> = vec![
        "MiniMax H3 shot-description task.".to_string(),
        format!("Return exactly {} JSON shot description{}: {{\"shots\":[{{\"description\":\"...\"}}]}}", plan.len(), plural(plan.len())),
        "Write only creative shot text. Do not include [Shot N] labels, cut times, markdown, or analysis.".to_string(),
        format!("Mode: {}", a.mode_label),
        format!("Duration: {exact}s, {}", a.aspect_ratio),
        format!("Audio mode: {}", if built_in { "Built-in MiniMax audio" } else { "Input Audio 1 preserved by Builder" }),
        format!("Shot count: {}", plan.len()),
        cut_plan_instruction(&a.cut_plan),
        format!(
            "MANDATORY CHARACTER BUDGET: The combined text inside all {} JSON description values must not exceed {} characters total (about {} per shot). Stay within this combined limit; be concise without omitting required subjects, actions, or camera direction.",
            plan.len(), budget.shot_chars, per_shot
        ),
        concat!(
            "SHOT FORMAT — MANDATORY: Write concise complete sentences, with length and action complexity suited to the available shot time, in this order. ",
            "1) Camera: the opening framing, one named camera move with its direction and speed, and the ending framing. ",
            "2) Subject action: each person in the cast does their own continuous physical action, written by what the body does. ",
            "3) Acting detail: each visible face's concrete expression for this moment, as the PERFORMANCE CRAFT rules below describe. ",
            "4) Light and set: one short line using only the mapped location's own light and objects. ",
            "Describe only what the camera sees. Show emotion only as visible movement. Do not use feeling words such as feel, grief, longing, memory, soul, or emotional. ",
            "Write complete grammatical prose, not notes, labels, or fragments. ",
            "Character appearance is already carried by the reference images and the Builder. Do not list clothing, hair, accessories, jewelry, or facial features. ",
            "Mention a garment or feature only when it moves or reacts in the action, in one brief clause at most."
        )
        .to_string(),
        format!(
            "MOTION ENERGY — MANDATORY: {} This scene is {exact} seconds long. Apply this energy to the requested action within the available time; speed does not require additional actions.",
            motion_energy_text(a.camera_speed, a.character_speed)
        ),
        scene_timing_instruction(a.duration, &a.cut_plan),
    ];
    let cut_list: Vec<String> = plan.iter().skip(1).map(|s| s.timecode.clone()).collect();
    parts.push(if cut_list.is_empty() {
        "Continuous shot: return one description only.".to_string()
    } else {
        format!("Builder cut times for your planning only: {}. Do not write these times.", cut_list.join(", "))
    });
    if !cast.is_empty() {
        let mut lines = vec![concat!(
            "CAST FOR THIS SCENE — MANDATORY: Character names in the scene text below have been replaced by these labels. ",
            "Identify each character by its subject label on first mention in each shot, then use natural pronouns and possessives when the actor is unambiguous. ",
            "For a single character, continue with she/he/they and her/his/their rather than repeating the label for every action. ",
            "With multiple characters, repeat a label when the actor or speaker changes or a pronoun would be ambiguous. ",
            "Do not append character names or picture origins in parentheses."
        )
        .to_string()];
        for item in &cast {
            let role = if a.visual_only || a.lyric_text.is_empty() {
                "does not sing, speak, or lip sync; acts and reacts silently with a relaxed closed mouth"
            } else if built_in {
                "SINGS and lip-syncs the exact lyric line with visible mouth, lip, and jaw movement"
            } else {
                // Supplied audio drives the mouth, so the prose describes the face and body (SUPPLIED-AUDIO SINGING below).
                "SINGS the exact lyric line in sync with <Audio 1>, acting it with the face and body"
            };
            let name = if item.name.is_empty() { "mapped subject" } else { item.name.as_str() };
            lines.push(format!("- {} ({}): {}", item.label, name, role));
        }
        let only = cast
            .iter()
            .map(|i| format!("{} ({})", i.label, if i.name.is_empty() { "mapped subject" } else { i.name.as_str() }))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "ONLY THESE PEOPLE APPEAR — MANDATORY: {only}. No other person, hand, arm, shadow, reflection, silhouette, or crowd appears in any shot."
        ));
        parts.push(lines.join("\n"));
    }
    parts.push(format!(
        "Camera speed: {}/10{}.",
        fmt_g(a.camera_speed),
        if a.camera_guidance.is_empty() { String::new() } else { format!(" - {}", compact(&a.camera_guidance, 240)) }
    ));
    parts.push(format!(
        "Character speed: {}/10{}.",
        fmt_g(a.character_speed),
        if a.character_guidance.is_empty() { String::new() } else { format!(" - {}", compact(&a.character_guidance, 240)) }
    ));
    if a.camera_speed >= 7.0 {
        parts.push("Camera rule: use energetic, visibly active camera movement; avoid slow/static/locked-off language.".to_string());
    }
    let cues = a.cues.as_ref().filter(|c| !c.is_empty());
    if a.no_character {
        parts.push("Vocal performance: no visible character / no lip sync. Do not invent a visible singer or speaker.".to_string());
    } else if a.visual_only || a.lyric_text.is_empty() {
        parts.push("Vocal performance: no exact lyric is assigned to this scene. Do not add singing, speaking, or lip sync.".to_string());
    } else if let Some(cues) = cues {
        parts.push(
            concat!(
                "TIMED VOCAL CUES ONLY: Follow the timed singer/shot contract exactly. Never place, anticipate, continue, or repeat a lyric in an ",
                "instrumental shot. A performer sings only inside the specifically assigned vocal cue shot."
            )
            .to_string(),
        );
        parts.push(format!("Timed singer / shot contract:\n{}", cue_shot_contract(cues)));
    } else {
        let chunks = lyric_chunks(&a.lyric_text, plan.len() as i64);
        let singer = match cast.first() {
            Some(first) if !first.name.is_empty() => format!("{} ({})", first.label, first.name),
            Some(first) => first.label.clone(),
            None => "the singer".to_string(),
        };
        let mouth = if built_in {
            "Show clear, natural mouth, lip, jaw, and facial movement synchronized to the audible vocal. "
        } else {
            "Audio 1 drives the singer's mouth, so act the line with the face and body. "
        };
        parts.push(format!(
            "MANDATORY VOCAL PERFORMANCE: The assigned subject is visibly singing the exact supplied lyric/audio during this scene. Use the stable visible subject label. {mouth}LYRIC IN THE SHOT — MANDATORY: write the exact lyric words, unchanged, inside <d>[English] exact words with final punctuation.</d>, introduced like this: {singer} sings <d>[English] the exact words.</d> in sync with <Audio 1>. Every lyric line given below must appear in a shot, in order."
        ));
        if chunks.len() == 1 {
            parts.push(format!("Exact lyric line:\n{}", compact(&chunks[0], 900)));
        } else {
            let lines: Vec<String> = chunks
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.is_empty())
                .map(|(i, c)| format!("Shot {}: {}", i + 1, compact(c, 900)))
                .collect();
            parts.push(format!("Exact lyric lines, one group per shot:\n{}", lines.join("\n")));
        }
    }
    let subject_block = if a.no_character { "No main character is visible in this scene.".to_string() } else { a.subject_text.clone() };
    let sections: [(&str, String); 8] = [
        ("Story beat", labelize(&a.story_beat)),
        ("Lyric section", a.lyric_section.clone()),
        ("Scene notes", labelize(&a.scene_notes)),
        ("Motion/camera request", labelize(&a.motion_request)),
        ("Subject (identity context only; do not restate appearance or clothing in the shot text)", subject_block),
        ("Location", a.location_text.clone()),
        ("Manual audio direction for staging only", a.audio_direction.clone()),
        ("Continuity notes for staging only", labelize(&a.continuity)),
    ];
    for (label, value) in sections {
        let text = compact(&value, 900);
        if !text.is_empty() {
            parts.push(format!("{label}:\n{text}"));
        }
    }
    if !a.storyboard_context.trim().is_empty() {
        parts.push(format!("Storyboard Builder context:\n{}", a.storyboard_context.trim()));
    }
    if !a.labels.is_empty() {
        let pictures = a.labels.iter().map(|l| l.picture.clone()).collect::<Vec<_>>().join(", ");
        parts.push(format!("Available renderer reference labels: {pictures}. Do not define labels in the shot text."));
        let lines = a
            .labels
            .iter()
            .map(|i| format!("{}: {} ({}), {} reference.", i.picture, i.label, i.name, i.kind))
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("Renderer picture assignments for planning only:\n{lines}"));
    }
    if a.mode_label == "Reference to Video" {
        parts.push(REFERENCE_SCENE_GROUNDING.to_string());
    }
    parts.push(performance_craft_instruction(
        &a.style,
        !cast.is_empty(),
        !a.lyric_text.is_empty() && !a.visual_only && !a.no_character,
        a.continuation.is_some(),
    ));
    parts.push(literal_scene_instruction());
    if !built_in && !a.visual_only && !a.no_character {
        parts.push(
            concat!(
                "SUPPLIED-AUDIO SINGING: Include the assigned singer and Audio 1 synchronization cue once, only during audible vocals. ",
                "Do not describe mouth, lip or jaw movement or ask for singing from the first frame. Preserve any supplied vocal timing; introductory silence remains non-vocal."
            )
            .to_string(),
        );
    }
    if let Some(continuation) = &a.continuation {
        // Last, where the model weighs it most. The vocal rule applies only when the scene sings.
        let has_vocals = !(a.visual_only || a.no_character || a.lyric_text.is_empty());
        parts.push(continuation_task_text(continuation, &a.previous_shot, has_vocals, a.with_picture, &a.location_contract));
    }
    parts.join("\n\n")
}

const REFERENCE_SCENE_GROUNDING: &str = concat!(
    "REFERENCE SCENE GROUNDING — MANDATORY: Generate a complete new scene from the assigned character and environment pictures. ",
    "Character pictures supply identity and appearance; environment pictures supply the set. The prompt determines opening framing, camera angle, staging, pose, composition, and action. Never copy a reference picture's composition, framing, camera angle, or pose as the opening shot. A separately enabled continuation task follows its own previous-frame rules. ",
    "A saved image prompt is a proposed scene idea, not proof that its props, poses, or layout exist in a supplied picture. ",
    "Use props supported by the mapped references or current explicit scene directions; omit unsupported carryover props. ",
    "When a requested action needs a new prop, introduce its appearance and physical placement before using it, rather than an unexplained 'the rusted worktable'. ",
    "COHERENT SHOT WRITING: Treat the story beat and scene card as story context to translate into a self-contained, physically coherent shot, ",
    "not prose to splice into a camera template. Introduce each prop, its owner or containing object, and its placement before any action or camera instruction refers to it. ",
    "For a coat-and-zipper beat, establish 'a battered coat lies across a rusted worktable, its zipper caught half-open' before referring to 'the zipper'. ",
    "Establish any character's position relative to those objects before describing interaction. Write the opening setup, action, camera movement, ",
    "and final framing in chronological order; do not describe the camera's ending before establishing its target. ",
    "Ensure the final framing is physically possible from the stated character and prop positions. Preserve the beat's intended visual emphasis and endpoint; ",
    "do not replace a prop-focused ending with a generic singing close-up or invent prop handling merely because a singer is present. ",
    "Integrate a required performance only through staging consistent with the scene directions. Before returning the shot, reread it independently ",
    "of the beat and resolve every unexplained object, gesture, spatial relationship, and camera target. ",
    "STAGING AND ACTION OWNERSHIP: Establish where each character stands and where any handled prop is relative to them. ",
    "Make the character the actor: '<Subject 1> grips the zipper pull with her gloved right hand', not 'a studded glove grips the zipper'. ",
    "Attribute every gesture and facial reaction to that character; use '<Subject 1> looks down at the zipper, then back toward the camera', not unowned 'eyes flick down and back'. ",
    "Do not invent gloves or accessories. Describe the environment as a physical setting around the character, with concrete spatial and lighting relationships; ",
    "a picture label identifies the environment reference, not a moving background object. State each singing/audio synchronization direction once, integrated into the action. ",
    "LOCATION REFERENCE: Establish the physical setting and bind the location picture to that setting. ",
    "Then describe the camera independently. Derive framing, focus, camera motion, staging, and how much of the location is visible ",
    "from this scene's story beat, storyboard details, scene-card directions, and selected camera settings. ",
    "The location picture supplies the environment's visual appearance; the scene directions determine how it is filmed. ",
    "Identify each character by its subject label on first mention in each shot, then use natural pronouns and possessives when the actor is unambiguous. ",
    "For a single character, continue with she/he/they and her/his/their rather than repeating the label for every action. ",
    "With multiple characters, repeat a label when the actor or speaker changes or a pronoun would be ambiguous. ",
    "Do not append character names or picture origins in parentheses. ",
    "Do not add a standalone reference-definition paragraph to the shot description."
);
