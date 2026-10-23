//! Instruction texts and shot description cleanup. Port of the middle part of `minimax/shot_prompt.py`.

use serde_json::Value;

use super::plan::{shot_plan, Shot, CONTINUOUS_CUTS_OPENING, FALLBACK_SHOT};
use super::rx::{clean_sentence_start, group, is_match, rx, split_sentences, sub, sub_first, sub_with};
use crate::error::CoreError;
use crate::pyval::{fmt_g, round_to, take_chars, truthy, Obj};

const LIFE_SUBTLE: [&str; 12] = [
    "breath catching before a held note", "weight shifting from one foot to the other", "fingers tightening then loosening on an object",
    "a glance away and back", "shoulders dropping after a held pose", "a slow blink", "a head tilt on a sustained note",
    "a hand brushing hair or collar", "a small smile that fades", "jaw tension then release", "a lean toward or away from the camera",
    "an unfinished gesture",
];
const LIFE_ACTIVE: [&str; 12] = [
    "a snap turn that whips the hair", "a quick step then a hard stop", "shoulders driving on the downbeat", "a spin that ends in a pose",
    "a fast reach and grab", "a head throw back on the high note", "a jump landing with bent knees", "a quick glance at the other person mid-stride",
    "hands slicing the air with the rhythm", "a lean into the camera then a push away", "a stumble that turns into a dance step",
    "a laugh breaking out mid-motion",
];

/// Natural action-led prose with clear actors and placement.
pub fn literal_scene_instruction() -> String {
    concat!(
        "LITERAL SCENE CLARITY — MANDATORY: Write natural, fluent shot prose that H3 can follow literally. Lead ",
        "physical action with the person doing it; use their possessive for a body part or worn item and clear ",
        "pronouns afterward. For example: '<Subject 1> steps past the ground-level camera, her boot briefly crossing ",
        "the foreground.' Describe how the camera films that same action instead of independently placing a body part ",
        "in the foreground and its owner elsewhere. A close-up identifies whose body part is framed; describe ",
        "cropping only when needed to understand the shot. Keep ownership and spatial checks in your reasoning; ",
        "finished sentences describe visible action, not explanations of anatomy or routine clothing attachment. ",
        "Rewrite ambiguous scene-card or image-prompt wording into ordinary action-led sentences rather than copying ",
        "it. Keep character, camera and prop positions consistent from the opening through the ending. Place loose ",
        "props explicitly and use only objects supported by this scene's directions or mapped references. Carry an ",
        "object from an adjacent scene only when the current scene explicitly does so. Before returning, read the ",
        "shot by itself: every action has an actor, every item has a clear owner or placement, and the camera ",
        "movement fits the same physical action. Resolve uncertainty with clear verbs and possessives, not ",
        "explanatory qualifiers. Return only the requested shot JSON with natural visual prose."
    )
    .to_string()
}

/// The real scene and per-shot time budgets for the creative LLM.
pub fn scene_timing_instruction(duration: f64, cut_plan: &Obj) -> String {
    let exact = round_to(duration, 3);
    let plan = shot_plan(cut_plan);
    let mut windows: Vec<String> = Vec::new();
    for (index, shot) in plan.iter().enumerate() {
        let start = shot.time;
        let end = plan.get(index + 1).map_or(exact, |next| next.time);
        let available = round_to((end - start).max(0.0), 3);
        windows.push(format!("Shot {}: {}–{}s ({} seconds available).", index + 1, fmt_g(start), fmt_g(end), fmt_g(available)));
    }
    let mut lines = vec![format!("SCENE TIMING — MANDATORY: The complete scene lasts exactly {} seconds.", fmt_g(exact))];
    lines.extend(windows);
    lines.push(
        concat!(
            "Use the scene beat and storyboard directions together to stage only what can physically finish within each ",
            "shot's available time. Reserve time for the required singing or speaking and any supplied entrance, reveal, ",
            "or transition. For a brief shot, express the essential beat through one economical continuous action and the ",
            "requested camera move; let performance and camera movement happen together where physically possible. Keep ",
            "optional gestures and reactions only when time remains. Do not invent sequential steps, pivots, glances, or ",
            "framing changes to fill a checklist. Preserve explicitly requested action and exact vocal words; simplify ",
            "optional choreography rather than rushing required performance. Motion speed sets the energy of the chosen ",
            "movement, not the number of actions. Before returning, check that the opening, action, camera travel, and ",
            "ending fit the available time without an extra cut, rushed final beat, or action continuing beyond the shot."
        )
        .to_string(),
    );
    lines.join("\n")
}

pub fn motion_energy_text(camera_speed: f64, character_speed: f64) -> String {
    let camera = if camera_speed >= 7.0 {
        "The requested camera movement is fast and energetic."
    } else if camera_speed >= 4.0 {
        "The camera moves at a steady pace with a clear direction."
    } else {
        "The camera moves slowly, or holds a locked frame."
    };
    let character = if character_speed >= 7.0 {
        "The requested performer action has high energy and decisive physical movement."
    } else if character_speed >= 4.0 {
        "The requested performer action has steady physical energy."
    } else {
        "The performers use small movements and held poses with life detail."
    };
    format!("{camera} {character}")
}

/// Six small life details, rotated by a seed so scenes differ.
pub fn life_movements(character_speed: f64, seed: &str) -> Vec<String> {
    let bank: Vec<&str> = if character_speed >= 7.0 {
        LIFE_ACTIVE.to_vec()
    } else if character_speed >= 4.0 {
        LIFE_SUBTLE[..6].iter().chain(LIFE_ACTIVE[..6].iter()).copied().collect()
    } else {
        LIFE_SUBTLE.to_vec()
    };
    let offset = seed.chars().map(|c| c as u64).sum::<u64>() as usize % bank.len();
    (0..6).map(|i| bank[(offset + i) % bank.len()].to_string()).collect()
}

/// The opening of `detailed_description`: the style name and its look, then the continuity sentence.
pub fn opening_wrapper(style: &str, continuous: bool, guidance: &str) -> String {
    let continuity = if continuous { format!(" {CONTINUOUS_CUTS_OPENING}") } else { String::new() };
    let look = if guidance.trim().is_empty() { ".".to_string() } else { format!(": {}", guidance.trim()) };
    let style = if style.is_empty() { "photorealistic cinematic" } else { style };
    format!("detailed_description:\nThe target video is in a {style} music-video style{look}{continuity}\n\n")
}

/// Keep only positive visual sentences. Quoted lyric words are sung text, never a reason to drop a sentence.
pub fn strip_negative_sentences(description: &str) -> String {
    let text = description.trim();
    if text.is_empty() {
        return String::new();
    }
    let negative = rx!(r"(?i)\b(?:do\s+not|don['’]t|never|without|avoid|must\s+not|cannot|can['’]t|not|no)\b");
    let quote_pattern = rx!(r#"(?i)<d>[\s\S]*?</d>|["“][^"”]*["”]"#);
    let mut quotes: Vec<String> = Vec::new();
    let mut masked = String::new();
    let mut last = 0;
    for m in quote_pattern.find_iter(text).filter_map(Result::ok) {
        masked.push_str(&text[last..m.start()]);
        masked.push_str(&format!("VRGDGQUOTE{}TOKEN", quotes.len()));
        quotes.push(m.as_str().to_string());
        last = m.end();
    }
    masked.push_str(&text[last..]);
    // A decimal point (1.5 seconds) is not a sentence end.
    let protected = sub(rx!(r"(\d)\.(?=\d)"), &masked, "${1}VRGDGDECIMALTOKEN");
    let mut sentences: Vec<String> =
        split_sentences(&protected).into_iter().map(|s| s.replace("VRGDGDECIMALTOKEN", ".")).collect();
    if sentences.is_empty() {
        sentences.push(masked.clone());
    }
    let token = rx!(r"VRGDGQUOTE\d+TOKEN");
    let kept = sentences
        .iter()
        .filter(|s| !is_match(negative, &sub(token, s, " ")))
        .map(|s| s.trim().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let restored = sub_with(rx!(r"VRGDGQUOTE(\d+)TOKEN"), &kept, |c| {
        group(c, 1).parse::<usize>().ok().and_then(|n| quotes.get(n).cloned()).unwrap_or_default()
    });
    sub(rx!(r"\s{2,}"), &restored, " ").trim().to_string()
}

/// Port of `normalizeMiniMaxH3ShotDescription` for the parts that apply without dialogue tags.
pub fn normalize_description(text: &str) -> String {
    let mut clean = sub(rx!(r"<\|[^<>]*\|>"), text, "");
    clean = sub(rx!(r"(?i)<\s*(Subject\s+\d+)\s*\(\s*([^)<>]+)\s*>"), &clean, "<$1> ($2)");
    clean = sub(rx!(r"(?i)(?<!<)\bSubject\s+(\d+)\b(?!\s*>)"), &clean, "<Subject $1>");
    clean = sub(rx!(r"\bAudio\s+1\b"), &clean, "<Audio 1>");
    clean = sub(rx!(r"<+Audio 1>+"), &clean, "<Audio 1>");
    clean = sub(rx!(r"(?i)\bImage\s+(\d+)\b"), &clean, "<Picture $1>");
    if clean.matches('"').count() % 2 == 1 {
        clean = clean.replace('"', ""); // an unpaired quote is LLM noise, not a quotation
    }
    sub(rx!(r"\s+"), &clean, " ").trim().to_string()
}

fn strip_cut_directive(text: &str) -> String {
    let mut clean = normalize_description(text);
    let mut previous: Option<String> = None;
    while !clean.is_empty() && previous.as_deref() != Some(clean.as_str()) {
        previous = Some(clean.clone());
        clean = sub(rx!(r"(?i)^(?:(?:the\s+camera\s+)?cuts?\s+to|cut\s+to)\s*(?::|[-–—.]|\s)*"), &clean, "").trim().to_string();
    }
    clean
}

/// A later shot of a continuous-moment scene says what it carries on from.
pub fn continuous_cut_text(text: &str, shot_number: i64) -> String {
    if is_match(rx!(r"(?i)\bcontinu"), text) {
        return text.to_string();
    }
    let carried = format!("the camera cuts, continuing directly from the final moment of Shot {}.", shot_number - 1);
    if is_match(rx!(r"(?i)^the camera cuts\."), text) {
        return sub_first(rx!(r"(?i)^the camera cuts\."), text, &carried);
    }
    format!("{carried} {text}")
}

/// The text after `[Shot N] At MM:SS.mmm,`.
pub fn post_cut_text(text: &str) -> String {
    format!("the camera cuts. {}", clean_sentence_start(&strip_cut_directive(text)))
}

fn leading_json(source: &str) -> String {
    let source = source.trim();
    let Some(start) = source.find('{') else { return source.to_string() };
    let (mut depth, mut in_string, mut escaped) = (0i32, false, false);
    for (offset, ch) in source[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..start + offset + 1].to_string();
                }
            }
            _ => {}
        }
    }
    source.to_string()
}

/// The creative shot descriptions from the LLM's JSON, exactly `expected` of them.
pub fn parse_shot_descriptions(raw: &str, expected: usize) -> Result<Vec<String>, CoreError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(CoreError::shot("The LLM returned an empty MiniMax shot-description payload.", "EMPTY_SHOTS"));
    }
    let parsed: Value = serde_json::from_str(&leading_json(text)).map_err(|_| {
        CoreError::shot(
            format!("The LLM did not return valid JSON shot descriptions. Raw output:\n{}", take_chars(text, 600)),
            "BAD_JSON",
        )
    })?;
    let mut shots: Vec<Value> = parsed.get("shots").and_then(Value::as_array).cloned().unwrap_or_default();
    if shots.len() > expected {
        return Err(CoreError::shot(
            format!("The LLM returned {} shot descriptions, but {} were expected.", shots.len(), expected),
            "WRONG_SHOT_COUNT",
        ));
    }
    shots.resize(expected, Value::Object(Obj::new()));
    let mut result = Vec::new();
    for (index, item) in shots.iter().enumerate() {
        let description = match item {
            Value::String(s) => s.clone(),
            Value::Object(o) => {
                let pick = ["description", "text", "shot"].iter().filter_map(|k| o.get(*k)).find(|v| truthy(v));
                crate::pyval::text_or_empty(pick).trim().to_string()
            }
            _ => String::new(),
        };
        if description.is_empty() {
            result.push(FALLBACK_SHOT.to_string());
            continue;
        }
        if is_match(rx!(r"(?i)\[\s*Shot\s+\d+\s*\]"), &description)
            || is_match(rx!(r"(?i)\bAt\s+\d{1,2}:\d{2}(?:\.\d{1,3})?\b"), &description)
        {
            return Err(CoreError::shot(
                format!("The LLM included shot labels or cut times inside shot {}.", index + 1),
                "LABELS_IN_SHOT",
            ));
        }
        let positive = strip_negative_sentences(&description);
        result.push(if positive.is_empty() { FALLBACK_SHOT.to_string() } else { normalize_description(&positive) });
    }
    Ok(result)
}

/// The `[Shot N]` blocks, with cut times, for already written descriptions.
pub fn shot_body(descriptions: &[String], cut_plan: &Obj) -> Result<String, CoreError> {
    let plan: Vec<Shot> = shot_plan(cut_plan);
    if descriptions.len() != plan.len() {
        return Err(CoreError::shot(
            format!("Cannot assemble MiniMax shots: expected {} descriptions, got {}.", plan.len(), descriptions.len()),
            "WRONG_SHOT_COUNT",
        ));
    }
    let continuous = cut_plan.get("continuous").is_some_and(truthy);
    let mut blocks = Vec::new();
    for (shot, description) in plan.iter().zip(descriptions) {
        if shot.number == 1 {
            blocks.push(format!("[Shot 1] {description}").trim().to_string());
        } else {
            let mut text = post_cut_text(description);
            if continuous {
                text = continuous_cut_text(&text, shot.number);
            }
            blocks.push(format!("[Shot {}] At {}, {}", shot.number, shot.timecode, text).trim().to_string());
        }
    }
    Ok(blocks.join("\n\n"))
}

/// The saved prompt for a reference-to-video scene. `guidance` is the video style's look for the opening line.
pub fn assemble_prompt(descriptions: &[String], cut_plan: &Obj, style: &str, guidance: &str) -> Result<String, CoreError> {
    let continuous = cut_plan.get("continuous").is_some_and(truthy);
    let body = shot_body(descriptions, cut_plan)?;
    Ok(format!("{}{}", opening_wrapper(style, continuous, guidance), body).trim().to_string())
}
