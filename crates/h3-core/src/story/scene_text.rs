//! Wording rules for the Storyboard's scene prompts: timed lyric cues, facial requirements, motion speed and reference openings. Port of
//! the pure helpers in `storyboard/scene_prompts.py`.

use fancy_regex::Regex;
use serde_json::Value;

use super::helpers::{normalize_performance_mode, prompt_mentions_visible_face, scene_has_visible_character};
use super::mode::{text, text_s};
use crate::pyval::{text_or_empty, to_float, truthy, Obj};

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn is_match(pattern: &str, text: &str) -> bool {
    rx(pattern).is_match(text).unwrap_or(false)
}

fn sub_all(pattern: &str, text: &str, with: &str) -> String {
    rx(pattern).replace_all(text, with).into_owned()
}

fn sub_once(pattern: &str, text: &str, with: &str) -> String {
    rx(pattern).replace(text, with).into_owned()
}

fn lowered(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_lowercase()
}

fn object<'a>(scene: &'a Value, key: &str) -> Option<&'a Obj> {
    scene.get(key).and_then(Value::as_object)
}

/// `_storyboard_timed_lyric_contract`: the Whisper timed cue rules for a scene's video prompt, or nothing.
pub fn timed_lyric_contract(scene: &Value) -> String {
    if !scene.is_object() {
        return String::new();
    }
    let vocal = object(scene, "vocal_status");
    let from_scene = scene.get("lyric_cue_map").filter(|v| truthy(v));
    let from_vocal = vocal.and_then(|v| v.get("lyric_cue_map")).filter(|v| truthy(v));
    let cues = match from_scene.or(from_vocal) {
        Some(Value::Array(cues)) => cues.clone(),
        _ => Vec::new(),
    };
    if cues.is_empty() {
        return contract_text_fallback(scene);
    }
    let mut rows: Vec<String> = Vec::new();
    let mut starts: Vec<f64> = Vec::new();
    for (index, cue) in cues.iter().enumerate() {
        let Some(cue) = cue.as_object() else { continue };
        let (Some(start), Some(end)) = (cue.get("start").and_then(to_float), cue.get("end").and_then(to_float)) else { continue };
        if !start.is_finite() || !end.is_finite() || end <= start {
            continue;
        }
        let number = index + 1;
        starts.push(start);
        if lowered(cue.get("type")) == "instrumental" {
            rows.push(format!("[Cue {number}] {start:.3}s-{end:.3}s: INSTRUMENTAL. No visible singing or lip-sync; mouths remain closed or naturally relaxed."));
            continue;
        }
        let lyric = text(cue.get("text"), 1200);
        if lyric.is_empty() {
            continue;
        }
        let singer = {
            let found = text(cue.get("singer_name"), 160);
            if found.is_empty() { "the assigned singer".to_string() } else { found }
        };
        rows.push(format!("[Cue {number}] {start:.3}s-{end:.3}s: {singer} sings only <d>[English] {lyric}</d>. Do not add words from any other cue."));
    }
    if rows.is_empty() {
        return String::new();
    }
    let cut_times = starts.iter().skip(1).map(|v| format!("{v:.3}s")).collect::<Vec<_>>().join(", ");
    let cut_rule = if cut_times.is_empty() { "Create one continuous shot interval for this cue.".to_string() } else { format!("Create exactly {} chronological shot intervals with cuts at {cut_times}.", rows.len()) };
    let mut all = vec!["AUTHORITATIVE WHISPER-TIMED LYRIC CUE CONTRACT \u{2014} this overrides the general cut-frequency plan and full scene lyric:".to_string(), cut_rule];
    all.extend(rows);
    all.push("Never repeat the complete scene lyric in every shot. Each shot may contain only the lyric assigned to its own timed cue; instrumental cues contain no visible singing.".into());
    all.join("\n")
}

/// The contract text a scene already carries, for a scene with no cue list.
fn contract_text_fallback(scene: &Value) -> String {
    let vocal = object(scene, "vocal_status");
    let own = scene.get("timed_lyric_cue_contract").filter(|v| truthy(v));
    let theirs = vocal.and_then(|v| v.get("timed_lyric_cue_contract")).filter(|v| truthy(v));
    text(own.or(theirs), 12000)
}

/// `_storyboard_scene_is_visible_singing`.
pub fn scene_is_visible_singing(scene: &Value) -> bool {
    if !scene.is_object() || !scene_has_visible_character(scene) {
        return false;
    }
    let vocal = object(scene, "vocal_status").cloned().unwrap_or_default();
    let mode = [scene.get("performance_mode"), vocal.get("performance_mode"), scene.get("video_type"), scene.get("videoType")].into_iter().flatten().find(|v| truthy(v));
    if normalize_performance_mode(mode) != "singing" {
        return false;
    }
    if ["instrumental", "no_lip_sync", "no_character_present"].iter().any(|k| vocal.get(*k).is_some_and(truthy)) {
        return false;
    }
    if matches!(vocal.get("should_lip_sync"), Some(Value::Bool(false))) {
        return false;
    }
    let lyric = [vocal.get("lyric_text"), scene.get("lyrics"), scene.get("lyric_line")].into_iter().flatten().find(|v| truthy(v));
    !text(lyric, 1200).is_empty()
}

/// `_enforce_storyboard_video_facial_requirements`: a video prompt of a visible face mentions eye movement and blinking, and a singer is
/// never "quiet".
pub fn enforce_video_facial_requirements(prompt: &str, scene: &Value) -> String {
    let mut body = text_s(prompt, 100_000);
    if body.is_empty() {
        return body;
    }
    let vocal = object(scene, "vocal_status");
    let no_character = vocal.is_some_and(|v| v.get("no_character_present").is_some_and(truthy)) || (scene.is_object() && (scene.get("no_character_present").is_some_and(truthy) || scene.get("noCharacterPresent").is_some_and(truthy)));
    if no_character {
        return body;
    }
    if !(scene_has_visible_character(scene) || prompt_mentions_visible_face(&body)) {
        return body;
    }
    let prompt_says_singing = is_match(r"(?i)\b(?:sings?|singing|raps?|rapping)\b", &body);
    if scene_is_visible_singing(scene) || prompt_says_singing {
        let replacements: [(&str, &str); 8] = [
            (r"(?i)\bwith\s+a\s+quiet,\s*internal\s+intensity\b", "with controlled internal intensity"),
            (r"(?i)\bwith\s+quiet\s+internal\s+intensity\b", "with controlled internal intensity"),
            (r"(?i)\bquiet,\s*internal\s+intensity\b", "controlled internal intensity"),
            (r"(?i)\bquiet\s+internal\s+intensity\b", "controlled internal intensity"),
            (r"(?i)\bquiet\s+intensity\b", "controlled intensity"),
            (r"(?i)\bquiet\s+performance\b", "controlled performance"),
            (r"(?i)\bquiet\s+emotion\b", "restrained emotion"),
            (r"(?i)\bquiet\s+singing\b", "focused singing"),
        ];
        for (pattern, with) in replacements {
            body = sub_all(pattern, &body, with);
        }
    }
    let has_blink = is_match(r"(?i)\bblink\w*\b", &body);
    let has_eye_movement = is_match(r"(?i)\beye\s+movement\b|\beyes?\s+(?:shift|move|track|glance|flick|dart)\b", &body);
    let mut additions: Vec<&str> = Vec::new();
    if !has_eye_movement {
        additions.push("subtle natural eye movement");
    }
    if !has_blink {
        additions.push("occasional natural blinking");
    }
    if !additions.is_empty() {
        let insert = format!(", {}", additions.join(", "));
        let found = rx(r"(?i)([^.]*(?:face|eyes?|brows?|gaze|expression)[^.]*)(\.)").captures(&body).ok().flatten().and_then(|c| c.get(1).map(|m| (m.start(), m.end())));
        if let Some((start, end)) = found {
            let sentence = format!("{}{insert}", body[start..end].trim_end());
            body = format!("{}{}{}", &body[..start], sentence, &body[end..]);
        } else {
            body = format!("{} with {}.", body.trim_end().trim_end_matches('.'), additions.join(", "));
        }
    }
    text_s(sub_all(r"\s{2,}", &body, " ").trim(), 100_000)
}

/// `_storyboard_speed_value`.
pub fn storyboard_speed(value: Option<&Value>, fallback: f64) -> f64 {
    match value.and_then(to_float) {
        Some(n) if n.is_finite() => n.clamp(0.0, 10.0),
        _ => fallback,
    }
}

/// `_camera_motion_for_storyboard_speed`: slow camera wording becomes brisk at a high camera speed.
pub fn camera_motion_for_speed(value: &str, speed: f64) -> String {
    let mut motion = text_s(value, 500);
    if motion.is_empty() || speed < 7.0 {
        return motion;
    }
    let replacements: [(&str, &str); 18] = [
        (r"(?i)\bslow cinematic drift\b", "energetic cinematic tracking drift"),
        (r"(?i)\bslow orbit\b", "energetic orbit"),
        (r"(?i)\bslow (left|right) orbit\b", "energetic ${1} orbit"),
        (r"(?i)\bslow zoom out\b", "brisk pull-back reveal"),
        (r"(?i)\bslow (left|right|side|lateral) drift\b", "brisk ${1} tracking drift"),
        (r"(?i)\bslow (pan|tilt|track|tracking|pull[ -]?back|drift)\b", "brisk ${1}"),
        (r"(?i)\bgentle lateral drift\b", "energetic lateral tracking"),
        (r"(?i)\bgentle pan reveal\b", "brisk pan reveal"),
        (r"(?i)\bgentle (pan|tilt|orbit|drift|camera movement)\b", "brisk ${1}"),
        (r"(?i)\bsubtle handheld movement\b", "active handheld tracking"),
        (r"(?i)\bsubtle handheld camera\b", "active handheld camera"),
        (r"(?i)\bsubtle handheld follow\b", "energetic handheld follow"),
        (r"(?i)\bsubtle rack focus\b", "quick rack focus"),
        (r"(?i)\bsubtle energetic orbit\b", "energetic orbit"),
        (r"(?i)\bsubtle settling pause\b", "active reframing beat"),
        (r"(?i)\bsubtle orbit movement\b", "energetic orbit movement"),
        (r"(?i)\b(?:quiet handheld hold|locked-off reaction hold|locked-off shot)\b", "active handheld reaction tracking"),
        (r"(?i)\brestrained pan\b", "brisk pan"),
    ];
    for (pattern, with) in replacements {
        motion = sub_all(pattern, &motion, with);
    }
    text_s(sub_all(r"\s{2,}", &motion, " ").trim(), 500)
}

/// `_enforce_storyboard_high_motion_language`: a fast scene's prompt keeps the camera and the subject moving.
pub fn enforce_high_motion_language(prompt: &str, scene: &Value) -> String {
    let mut body = text_s(prompt, 100_000);
    if body.is_empty() || !scene.is_object() {
        return body;
    }
    let pick = |a: &str, b: &str| scene.get(a).filter(|v| truthy(v)).or_else(|| scene.get(b).filter(|v| truthy(v)));
    let camera_speed = storyboard_speed(pick("camera_motion_speed", "cameraMotionSpeed"), 4.0);
    let character_speed = storyboard_speed(pick("character_motion_speed", "characterMotionSpeed"), 4.0);
    if camera_speed >= 7.0 {
        body = camera_motion_for_speed(&body, camera_speed);
        let replacements: [(&str, &str); 6] = [
            (r"(?i)\bthen\s+holds?\s+on\b", "then continues moving across"),
            (r"(?i)\bthen\s+holds?\b", "then continues moving"),
            (r"(?i)\bsettles?\s+into\s+a\s+(?:static\s+|steady\s+)?hold\b", "flows into another coordinated camera move"),
            (r"(?i)\b(?:static|steady)\s+hold\b", "continued camera motion"),
            (r"(?i)\bholds?\s+on\s+her\s+steady,\s*powerful\s+gaze\b", "tracks her powerful gaze while the camera keeps moving"),
            (r"(?i)\bholds?\s+on\s+(his|her|their|the)\s+([^,.]+)\b", "keeps moving around ${1} ${2}"),
        ];
        for (pattern, with) in replacements {
            body = sub_all(pattern, &body, with);
        }
        if !is_match(r"(?i)\b(?:tracking|orbit|whip pan|pan|tilt|crane|pullback|pull-back|push|dolly|handheld|reveal)\b", &body) {
            body = format!("{}, with energetic camera tracking that keeps moving instead of settling into a static hold.", body.trim_end().trim_end_matches('.'));
        }
    }
    if character_speed >= 4.0 {
        let replacements: [(&str, &str); 8] = [
            (r"(?i)\bmoves?\s+with\s+a\s+quiet,\s*poised\s+authority\b", "moves with forceful, physically active authority"),
            (r"(?i)\bmoves?\s+with\s+quiet,\s*poised\s+authority\b", "moves with forceful, physically active authority"),
            (r"(?i)\bquiet,\s*poised\s+authority\b", "forceful, physically active authority"),
            (r"(?i)\bquiet\s+poised\s+authority\b", "forceful physical authority"),
            (r"(?i)\bpoised,\s*unyielding\s+head\s+position\b", "forward-driving head posture with sharp turns"),
            (r"(?i)\bpoised\s+posture\b", "active, commanding posture"),
            (r"(?i)\bsubtle\s+body\s+motion\b", "clear full-body movement"),
            (r"(?i)\bstands?\s+still\b", "moves through the space"),
        ];
        for (pattern, with) in replacements {
            body = sub_all(pattern, &body, with);
        }
        if !is_match(r"(?i)\b(?:walks?|steps?|strides?|runs?|sprints?|dances?|crosses?|lunges?|reaches?|pushes?|pulls?|climbs?|fights?|brushes?|sweeps?|gestures?|interacts?|grabs?|lifts?|paces?)\b", &body) {
            body = format!("{}, while the subject performs a clear physical action with the body, hands, or surrounding set instead of relying on facial movement alone.", body.trim_end().trim_end_matches('.'));
        }
    }
    text_s(sub_all(r"\s{2,}", &body, " ").trim(), 100_000)
}

/// `_storyboard_starting_shot_value`: the starting shot a scene requires, or nothing.
pub fn starting_shot_value(scene: &Value) -> String {
    let Some(requirement) = object(scene, "starting_shot") else { return String::new() };
    if !matches!(requirement.get("required"), Some(Value::Bool(true))) {
        return String::new();
    }
    let chosen = [requirement.get("selected_starting_shot"), requirement.get("shot_type"), scene.get("shot_type")].into_iter().flatten().find(|v| truthy(v));
    text(chosen, 240)
}

/// `_storyboard_starting_shot_subject`.
pub fn starting_shot_subject(scene: &Value) -> String {
    if !scene.is_object() {
        return "the subject".into();
    }
    if let Some(Value::Array(visible)) = scene.get("visible_subjects") {
        for value in visible {
            let name = text(Some(value), 160);
            if !name.is_empty() {
                return name;
            }
        }
    }
    for key in ["subject_refs", "subjects"] {
        let Some(Value::Array(subjects)) = scene.get(key) else { continue };
        for subject in subjects {
            let name = match subject {
                Value::Object(map) => text(map.get("name"), 160),
                other => text(Some(other), 160),
            };
            if !name.is_empty() {
                return name;
            }
        }
    }
    "the subject".into()
}

/// `_storyboard_starting_shot_sentence`.
pub fn starting_shot_sentence(scene: &Value) -> String {
    let shot = starting_shot_value(scene);
    if shot.is_empty() {
        return String::new();
    }
    let subject = starting_shot_subject(scene);
    let key = sub_all(r"[\s_-]+", &shot.to_lowercase(), " ").trim().to_string();
    match key.as_str() {
        "eyes shot" => return format!("The video begins with an extreme close-up of {subject}'s eyes."),
        "mouth shot" => return format!("The video begins with an extreme close-up of {subject}'s mouth."),
        "hands shot" => return format!("The video begins with a close-up of {subject}'s hands."),
        "feet shot" => return format!("The video begins with a close-up of {subject}'s feet."),
        _ => {}
    }
    let article = if key.chars().next().is_some_and(|c| "aeiou".contains(c)) { "an" } else { "a" };
    let target = if subject != "the subject" { subject.as_str() } else { "the scene" };
    format!("The video begins with {article} {shot} of {target}.")
}

/// `_ensure_storyboard_starting_shot`: the required starting shot opens the prompt.
pub fn ensure_starting_shot(prompt: &str, scene: &Value) -> String {
    let body = text_s(prompt, 100_000);
    let sentence = starting_shot_sentence(scene);
    if body.is_empty() || sentence.is_empty() {
        return body;
    }
    let opening: String = body.chars().take(500).collect();
    let has_marker = is_match(r"(?i)\b(?:the\s+video\s+)?(?:begins?|starts?|opens?)\s+with\b|\b(?:opening|first)\s+(?:shot|frame)\b", &opening);
    let shot_key = starting_shot_value(scene).to_lowercase();
    let has_framing = if shot_key == "eyes shot" {
        is_match(r"(?i)\beyes?\b", &opening)
    } else {
        let words: Vec<String> = rx(r"[a-z0-9]+").find_iter(&shot_key).flatten().map(|m| m.as_str().to_string()).filter(|w| w != "shot").collect();
        !words.is_empty() && words.iter().all(|w| is_match(&format!(r"(?i)\b{}\b", fancy_regex::escape(w)), &opening))
    };
    if has_marker && has_framing {
        return body;
    }
    format!("{sentence} {body}").trim().to_string()
}

fn has_image(item: &Obj) -> bool {
    let image = item.get("image").and_then(Value::as_object).unwrap_or(item);
    image.get("path").is_some_and(truthy) || image.get("data").is_some_and(truthy) || item.get("image_path").is_some_and(truthy) || item.get("image_data").is_some_and(truthy)
}

/// `_storyboard_reference_opening`: how an image prompt starts when the scene has reference pictures.
pub fn reference_opening(scene: &Value) -> String {
    let subject_count = if !scene.is_object() || scene.get("no_character_present").is_some_and(truthy) {
        0
    } else {
        match scene.get("subject_refs") {
            Some(Value::Array(refs)) => refs.iter().filter_map(Value::as_object).filter(|s| has_image(s)).count(),
            _ => 0,
        }
    };
    let has_location = object(scene, "location_ref").is_some_and(has_image);
    if subject_count == 0 && !has_location {
        return String::new();
    }
    let phrase = if subject_count > 1 { "character reference images" } else { "character reference image" };
    if subject_count > 0 && has_location {
        format!("Using the provided {phrase} and location reference image")
    } else if subject_count > 0 {
        format!("Using the provided {phrase}")
    } else {
        "Using the provided location reference image".into()
    }
}

/// `_ensure_storyboard_reference_opening`.
pub fn ensure_reference_opening(prompt: &str, scene: &Value) -> String {
    let body = text_or_empty(Some(&Value::String(prompt.to_string()))).trim().to_string();
    let opening = reference_opening(scene);
    if opening.is_empty() || body.is_empty() {
        return body;
    }
    let mut body = sub_once(
        r"(?i)^Using the provided\s+(?:(?:character|location|scene|reference)\s+)*(?:images?|references?)(?:\s+and\s+(?:(?:character|location|scene|reference)\s+)*(?:images?|references?))*\s*,?\s*(?:create\s+)?",
        &body,
        "",
    )
    .trim()
    .to_string();
    body = sub_once(r"(?i)^and\s+(?:(?:character|location|scene|reference)\s+)*(?:images?|references?)\s*,?\s*(?:create\s+)?", &body, "").trim().to_string();
    body = sub_once(r"(?i)^(?:create|make|generate)\b\s*", &body, "").trim().to_string();
    if body.is_empty() {
        return format!("{opening}, create a cinematic still image.");
    }
    let mut chars = body.chars();
    let first: String = chars.next().map(|c| c.to_lowercase().collect()).unwrap_or_default();
    format!("{opening}, create {first}{}", chars.as_str()).trim().to_string()
}

/// `_storyboard_image_mode_uses_reference_opening`.
pub fn image_mode_uses_reference_opening(bundle: &Value) -> bool {
    let mode = [bundle.get("image_model_mode"), bundle.get("imageMode")].into_iter().flatten().find(|v| truthy(v)).map(|v| text_or_empty(Some(v))).unwrap_or_default().trim().to_lowercase();
    matches!(mode.as_str(), "nano_banana" | "flux_klein" | "flow_gpt")
}

// -- pronouns for a scene with one subject -----------------------------------------------------------------------------------

/// `_selected_storyboard_scene`: the scene a scene card bundle points at (the first when none matches).
pub fn selected_scene(bundle: &Value) -> Result<Value, String> {
    let scenes = match bundle.get("scenes") {
        Some(Value::Array(scenes)) if !scenes.is_empty() => scenes,
        _ => return Err("Storyboard scene-card payload has no scenes.".into()),
    };
    let number = |value: Option<&Value>| value.filter(|v| truthy(v)).and_then(to_float).map(|f| f.trunc() as i64);
    let selected = number(bundle.get("selected_scene_number")).or_else(|| number(scenes[0].get("scene_number"))).unwrap_or(1);
    Ok(scenes.iter().find(|scene| number(scene.get("scene_number")).unwrap_or(0) == selected).unwrap_or(&scenes[0]).clone())
}

/// The words that stand for the one subject of a scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Pronouns {
    pub subject: String,
    pub they: String,
    pub them: String,
    pub their: String,
    pub theirs: String,
    pub are: &'static str,
    pub sing: &'static str,
    pub perform: &'static str,
    pub say: &'static str,
}

/// `_single_subject_pronouns`: she, he or the name, for a scene that has exactly one subject.
pub fn single_subject_pronouns(scene: &Value) -> Option<Pronouns> {
    let map = scene.as_object()?;
    let subject_refs = map.get("subject_refs");
    let subjects = map.get("subjects");
    let count = match map.get("subject_count") {
        Some(Value::Null) | None => match (subject_refs, subjects) {
            (Some(Value::Array(refs)), _) if !refs.is_empty() => Some(json_count(refs.len())),
            (_, Some(Value::Array(list))) => Some(json_count(list.len())),
            _ => None,
        },
        other => other.cloned(),
    };
    let count = count.as_ref().filter(|v| truthy(v)).map_or(Some(0), |v| to_float(v).filter(|f| f.is_finite()).map(|f| f.trunc() as i64));
    if count != Some(1) {
        return None;
    }
    let subject = match (subject_refs, subjects) {
        (Some(Value::Array(refs)), _) if !refs.is_empty() => Some(&refs[0]),
        (_, Some(Value::Array(list))) if !list.is_empty() => Some(&list[0]),
        _ => None,
    };
    let (name, description) = match subject {
        Some(Value::Object(s)) => (text(s.get("name").filter(|v| truthy(v)).or(Some(&Value::String("the subject".into()))), 160), text(s.get("description"), 1200)),
        Some(other) => (text(Some(other).filter(|v| truthy(v)).or(Some(&Value::String("the subject".into()))), 160), String::new()),
        None => ("the subject".to_string(), String::new()),
    };
    let probe = format!("{name}\n{description}").to_lowercase();
    let make = |they: &str, them: &str, their: &str, theirs: &str| Pronouns { subject: name.clone(), they: they.into(), them: them.into(), their: their.into(), theirs: theirs.into(), are: "is", sing: "sings", perform: "performs", say: "says" };
    if is_match(r"\b(woman|girl|female|feminine|she|her)\b", &probe) {
        return Some(make("she", "her", "her", "hers"));
    }
    if is_match(r"\b(man|boy|male|masculine|he|him|his)\b", &probe) {
        return Some(make("he", "him", "his", "his"));
    }
    let shown = if name.is_empty() { "the subject".to_string() } else { name.clone() };
    let mut named = make(&shown, &shown, &format!("{shown}'s"), &format!("{shown}'s"));
    named.subject = shown;
    Some(named)
}

fn json_count(n: usize) -> Value {
    Value::from(n as i64)
}

fn match_case(replacement: &str, original: &str) -> String {
    let cased: Vec<char> = original.chars().filter(|c| c.is_lowercase() || c.is_uppercase()).collect();
    if !cased.is_empty() && cased.iter().all(|c| c.is_uppercase()) {
        return replacement.to_uppercase();
    }
    if original.chars().next().is_some_and(char::is_uppercase) {
        let mut chars = replacement.chars();
        return chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
    }
    replacement.to_string()
}

/// `_fix_single_subject_prompt_pronouns`: "they" in a prompt about one subject becomes she, he or the name.
pub fn fix_single_subject_prompt_pronouns(prompt: &str, bundle: &Value) -> Result<String, String> {
    let scene = selected_scene(bundle)?;
    let Some(p) = single_subject_pronouns(&scene) else { return Ok(prompt.to_string()) };
    let mut body = prompt.to_string();
    let phrases: Vec<(&str, String)> = vec![
        (r"(?i)\bthey\s+are\b", format!("{} {}", p.they, p.are)),
        (r"(?i)\bthey\s+sing\b", format!("{} {}", p.they, p.sing)),
        (r"(?i)\bthey\s+say\b", format!("{} {}", p.they, p.say)),
        (r"(?i)\bthey\s+perform\b", format!("{} {}", p.they, p.perform)),
        (r"(?i)\bthey\s+stand\b", format!("{} stands", p.they)),
        (r"(?i)\bthey\s+move\b", format!("{} moves", p.they)),
        (r"(?i)\bthey\s+walk\b", format!("{} walks", p.they)),
        (r"(?i)\bthey\s+glide\b", format!("{} glides", p.they)),
        (r"(?i)\bthey\s+turn\b", format!("{} turns", p.they)),
        (r"(?i)\bthey\s+look\b", format!("{} looks", p.they)),
        (r"(?i)\bthey\s+hold\b", format!("{} holds", p.they)),
        (r"(?i)\bthey\s+raise\b", format!("{} raises", p.they)),
        (r"(?i)\bthey\s+tilt\b", format!("{} tilts", p.they)),
        (r"(?i)\bthey\s+lean\b", format!("{} leans", p.they)),
    ];
    for (pattern, replacement) in &phrases {
        body = rx(pattern).replace_all(&body, |caps: &fancy_regex::Captures| match_case(replacement, &caps[0])).into_owned();
    }
    body = rx(r"(?i)\b(they|them|their|theirs)\b")
        .replace_all(&body, |caps: &fancy_regex::Captures| {
            let word = match caps[1].to_lowercase().as_str() {
                "they" => p.they.clone(),
                "them" => p.them.clone(),
                "their" => p.their.clone(),
                _ => p.theirs.clone(),
            };
            match_case(&word, &caps[0])
        })
        .into_owned();
    Ok(body)
}
