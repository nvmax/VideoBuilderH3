//! Location continuity and the continuation task. Port of the last part of `minimax/shot_prompt.py` before
//! `build_shot_task`. These texts keep a scene that continues the previous take on the same footing.

use serde_json::Value;

use super::rx::rx;
use crate::pyval::{collapse_ws, fmt_g, float_or_zero, take_chars, text_or_empty, truthy, Obj};

fn text(value: Option<&Value>) -> String {
    text_or_empty(value).trim().to_string()
}

fn location_identity(location: Option<&Obj>, fallback: &str) -> String {
    let name = location.map(|l| text(l.get("name"))).filter(|n| !n.is_empty()).unwrap_or_else(|| fallback.to_string());
    let description = collapse_ws(&location.map(|l| text_or_empty(l.get("description"))).unwrap_or_default());
    if description.is_empty() {
        return name;
    }
    let first = rx!(r"^.*?[.!?](?:\s|$)")
        .find(&description)
        .ok()
        .flatten()
        .map_or(description.clone(), |m| m.as_str().trim().to_string());
    let first = if first.chars().count() > 240 { format!("{}...", take_chars(&first, 237).trim()) } else { first };
    format!("{name} ({first})")
}

fn location_key(location: Option<&Obj>) -> String {
    let Some(l) = location else { return String::new() };
    let pick = ["id", "name", "description"].iter().filter_map(|k| l.get(*k)).find(|v| truthy(v));
    collapse_ws(&text_or_empty(pick).trim().to_lowercase())
}

/// The sentence that keeps a singing scene singing. `part` is `opening` (the first frame) or `transition` (a movement).
pub fn performance_line(has_vocals: bool, part: &str) -> String {
    if !has_vocals {
        return String::new();
    }
    if part == "transition" {
        return concat!(
            " The performer keeps singing the scene's lyrics on camera through the whole movement, lips and mouth in continuous ",
            "sync with <Audio 1>, and the movement keeps their face in view (it may travel around them but never hides the face ",
            "for more than a moment)."
        )
        .to_string();
    }
    concat!(
        " The performer is mid-performance, so keep singing the scene's lyrics without a pause from the first frame, lips and mouth ",
        "in continuous sync with <Audio 1>."
    )
    .to_string()
}

/// Stay in the established location, or at a change of mapped location perform the chosen transition once. Empty when
/// the scene has no mapped location.
pub fn location_continuity_contract(
    current: Option<&Obj>,
    previous: Option<&Obj>,
    preset: &str,
    custom: &str,
    hold_seconds: f64,
    has_vocals: bool,
) -> String {
    let current_key = location_key(current);
    if current_key.is_empty() {
        return String::new();
    }
    let current_name = location_identity(current, "the current mapped location");
    let previous_key = location_key(previous);
    if previous_key.is_empty() || previous_key == current_key {
        return format!(
            "LOCATION PHASE — ESTABLISHED CURRENT LOCATION: {current_name} is now the complete established environment. Continue forward through the exact visible opening state and deeper into its mapped geography. Build every newly revealed environmental feature from {current_name}, preserve its established spatial logic, and let the camera trajectory create the next composition. End looking deeper into {current_name}, fully grounded in that location."
        );
    }
    let prev = location_identity(previous, "the preceding mapped location");
    let cur = current_name;
    let hold = fmt_g(hold_seconds);
    let hold_value: f64 = hold.parse().unwrap_or(0.0);
    let common_ending = format!(
        "Complete the transition inside this uninterrupted scene and end looking deeper into {cur}, with its mapped geography filling the image as the location inherited by the following scene."
    );
    let normal = format!(
        "LOCATION PHASE — PHYSICAL THRESHOLD TURN: This scene performs the single physical passage from {prev} into {cur}. Track the subject to a visible doorway, gateway, solid foreground edge, or natural threshold already present in the opening frame. After the subject crosses its plane, let that foreground edge sweep fully across the image as a natural full-frame occlusion. During that continuous occlusion, cross the threshold and execute a clear camera arc around the subject toward {cur}. As the occluding edge clears, the camera faces forward into {cur}; {prev} has passed fully beyond the rear camera plane while the subject and destination reappear through coherent motion and parallax. Describe the threshold crossing, full-frame occlusion, camera arc, and final viewing direction explicitly."
    );
    let direction = match preset {
        "surreal" => format!(
            "LOCATION PHASE — SURREAL MATERIAL TRANSFORMATION: Transform {prev} progressively into {cur} through imaginative, visually connected material changes that travel across the frame. Use forms, textures, weather, particles, light, and movement visible in the actual opening image as the transformation source. Preserve the subject's continuous identity, action, position, and camera momentum while each physical element evolves into a corresponding element of {cur}. Make the transformation richly creative, spatially progressive, and complete."
        ),
        "cinematic" => format!(
            "LOCATION PHASE — CINEMATIC CONCEAL AND REVEAL: Choose a visually suitable detail present in the actual opening image, such as the subject's eye, hair, clothing, a shadow, bright light, fog, doorway, or foreground object. Move the camera into that detail until it fills the complete frame, carry the camera continuously through the concealed moment, then pull or glide outward to reveal the subject physically present in {cur}. Preserve the subject's action and camera momentum across the full-frame concealment."
        ),
        "inner_world" => format!(
            "LOCATION PHASE — INNER WORLD PORTAL: Reveal {cur} living inside a visually suitable surface already present in the opening image, such as an eye reflection, mirror, window, pool, pendant, crystal, smoke formation, or luminous opening. Let the destination become visibly dimensional inside that surface, move the camera continuously through it, and emerge with the subject inside the full-scale geography of {cur}. Treat the portal as one coherent passage with continuous scale, perspective, light, and motion."
        ),
        "match" => format!(
            "LOCATION PHASE — VISUAL MATCH TRANSITION: Inspect the actual opening image and find a strong shared shape, color, texture, light pattern, or motion that can connect {prev} to {cur}. Track that matching element as it fills or commands the composition, then let the same visual form resolve seamlessly as a real element in {cur}. Carry the subject's movement and camera trajectory through the visual correspondence with precise composition and spatial flow."
        ),
        "motion" => format!(
            "LOCATION PHASE — MOTION-DRIVEN TRANSITION: Use an energetic camera action suited to the actual opening frame, such as a whip pan, rapid orbit, fast push, foreground sweep, or close pass around the subject. Let directional motion and natural motion blur carry the complete image across the location boundary, then resolve the same movement and screen direction clearly inside {cur}. Preserve the subject's action, rhythm, and camera momentum throughout."
        ),
        "creative_auto" => format!(
            "LOCATION PHASE — CREATIVE IMAGE-AWARE TRANSITION: Inspect the actual opening image, {prev}, and {cur}, then choose the most visually convincing imaginative transition for their specific forms, materials, lighting, subject action, and camera trajectory. Build one explicit on-screen mechanism with readable progression and continuous movement, using a physical passage, material transformation, cinematic concealment, portal, visual match, motion bridge, or an equally coherent original idea. Make the chosen mechanism concrete in the shot description."
        ),
        "masked" => {
            let lead = if hold_value > 0.0 {
                format!(
                    "so for the first {hold} seconds simply continue the opening frame's action in {prev} with the same subject, framing, pace, and camera motion. No cut, no change of angle, and no new setup. At about {hold} seconds, "
                )
            } else {
                format!(
                    "so from the first frame carry on the opening frame's action in {prev} with the same subject, framing, pace, and camera motion. No cut, no change of angle, and no new setup. From the first frame, "
                )
            };
            format!(
                "LOCATION PHASE — MASKED CONTINUATION TRANSITION: The renderer already holds the previous scene's last moments, {lead}begin ONE smooth, motivated movement that carries the shot into {cur}. Choose the movement that best suits the opening frame: a camera move (push-in, pull-back, pan, tilt, track, or arc around the subject) or a natural subject move (turning, stepping through, walking on) that the camera follows. Let the surroundings change progressively through that movement and its parallax, with no wipe, flash, portal, morph, or cut. Describe the move and where the shot has arrived when it finishes.{}",
                performance_line(has_vocals, "transition")
            )
        }
        "custom" => {
            let custom_text = collapse_ws(custom);
            if custom_text.is_empty() {
                format!(
                    "LOCATION PHASE — CUSTOM TRANSITION FALLBACK: Inspect the actual opening image, {prev}, and {cur}, then create one highly imaginative, visually readable transition whose on-screen mechanism follows the subject and camera momentum into the destination."
                )
            } else {
                format!(
                    "LOCATION PHASE — CUSTOM TRANSITION: Apply this scene's authored transition direction: {custom_text} Translate it into explicit positive visual action that carries the actual opening image from {prev} into {cur} through one coherent continuous camera experience."
                )
            }
        }
        _ => normal,
    };
    format!("{direction} {common_ending}")
}

/// The rules an LLM needs to write a scene that continues the previous one with `latent_continuation_masked`. With
/// `with_picture` the LLM is shown the previous scene's final frame as Attached Picture 1. Without it the previous
/// scene's last shot description stands in for the picture.
pub fn continuation_task_text(
    continuation: &Obj,
    previous_shot: &str,
    has_vocals: bool,
    with_picture: bool,
    location_contract: &str,
) -> String {
    let hold_value = match continuation.get("hold_seconds") {
        None | Some(Value::Null) => 0.0,
        some => float_or_zero(some).unwrap_or(0.0),
    };
    let hold = fmt_g(hold_value);
    let hold_f: f64 = hold.parse().unwrap_or(0.0);
    let scene_seconds = float_or_zero(continuation.get("scene_seconds")).unwrap_or(0.0);
    let left_value = float_or_zero(continuation.get("seconds_left")).unwrap_or(0.0);
    let seconds_left = if left_value != 0.0 { left_value } else { (scene_seconds - hold_f).max(0.0) };
    let direction = text(continuation.get("direction"));
    let follow_up = if direction.is_empty() {
        "then advance them one small natural step at a time. "
    } else {
        "then carry on exactly as the AUTHOR'S DIRECTION at the end of this scene concept says. "
    };
    let mut parts: Vec<String> = Vec::new();
    if with_picture {
        let next = if direction.is_empty() {
            "The current story beat, lyrics/audio timing, and mapped location decide where the action goes next, reached through the continuing motion."
        } else {
            "The author's direction decides what happens next."
        };
        parts.push(format!(
            "FRAME-TO-FRAME CONTINUITY — HIGHEST PRIORITY:\nAttached Picture 1 is the previous rendered scene's actual final frame. The renderer already holds the last moments of the previous scene's motion and audio as the start of this render, so this scene is the very next moment of the same uninterrupted take, not a new shot. Begin the returned description with exactly: ‘Continuing seamlessly from the previous shot, the camera maintains its established course as’ and immediately name the same camera movement continuing at the same speed. Keep the subject's action, pace, pose, framing, camera angle, lighting, wardrobe, and environment exactly as Attached Picture 1 shows them, {follow_up}Do not restage, reset, re-establish, change framing, or cut. {next}{} Write every finished shot sentence as a positive description of the desired visible result. Attached Picture 1 remains an LLM-only observation source; finished prose uses direct visual description and the documented renderer labels.",
            performance_line(has_vocals, "opening")
        ));
        parts.push(
            concat!(
                "OPENING SUBJECT VISIBILITY — IMAGE-AWARE: Inspect Attached Picture 1 and inventory the human or character subjects actually visible in its opening composition. ",
                "Visible subjects continue from their exact observed position and state. Every currently mapped subject absent from Attached Picture 1 begins physically offscreen. ",
                "Bring an offscreen subject into view through a clearly described continuous screen-space event: an entrance through a frame edge, doorway, path, foreground layer, ",
                "or the selected transition mechanism, or a camera pan, track, or orbit that reaches and reveals them in a connected position. ",
                "State the observed empty or partially occupied opening composition first, then the exact entrance or camera-reveal route, then that subject's performance action. ",
                "Their first visible moment occurs through that physical entrance or reveal."
            )
            .to_string(),
        );
    } else {
        let follow = follow_up.replace("at the end of this scene concept", "below");
        parts.push(format!(
            "CONTINUATION — HIGHEST PRIORITY:\nThis scene is the very next moment of the same uninterrupted take the previous scene ended on. The renderer already holds the previous scene's last moments as the start of this render, so this is not a new shot. Begin the returned description with exactly: ‘Continuing seamlessly from the previous shot, the camera maintains its established course as’ and immediately name the same camera movement continuing at the same speed. Keep the subject's action, pace, pose, framing, camera angle, lighting, and environment as the previous scene's last shot leaves them, {follow}Do not restage, reset, re-establish, change framing, or cut. The camera line of Shot 1 continues the previous scene's camera and framing, it does not choose a new opening framing.{}",
            performance_line(has_vocals, "opening")
        ));
        if !previous_shot.is_empty() {
            parts.push(format!("PREVIOUS SCENE'S LAST SHOT (what is on screen when this scene starts):\n{previous_shot}"));
        }
    }
    if !location_contract.is_empty() {
        parts.push(location_contract.to_string());
    }
    if !direction.is_empty() {
        let timing = if scene_seconds > 0.0 {
            format!(
                "SCENE TIMING: This scene is {} seconds long. The direction starts at {hold} seconds and has to be completely finished before the scene ends, which leaves {} seconds for it. Perform every action of the direction in the author's order at a brisk pace that fits those {} seconds, and end the shot with the last action fully done, never cut off or left unfinished. ",
                fmt_g(scene_seconds),
                fmt_g(seconds_left),
                fmt_g(seconds_left)
            )
        } else {
            String::new()
        };
        let timed = if hold_f > 0.0 {
            format!(
                "Write the one shot description in two timed parts and put the timing in the text itself. First: \"For the first {hold} seconds, ...\" continuing the opening frame's action with the same camera motion, framing, and pace. Then: \"At about {hold} seconds, ...\" performing every action in the direction above, in the author's order and with the author's own verbs and objects, as one smooth continuous movement in the same take. "
            )
        } else {
            "Write the one shot description so the direction starts from the first frame, with no timed lead-in: perform every action in the direction above, in the author's order and with the author's own verbs and objects, as one smooth continuous movement in the same take. ".to_string()
        };
        let anchor = if with_picture { "Attached Picture 1" } else { "the previous scene's last shot" };
        parts.push(format!(
            "AUTHOR'S DIRECTION FOR THIS SCENE — MANDATORY, THE FINISHED DESCRIPTION MUST CONTAIN IT: \"{direction}\"\n{timing}{timed}If the direction needs a body position or facing different from {anchor} (for example standing up, walking, or turning), first describe the natural movement that gets the subject there, inside the same take. Do not skip or replace any action in the direction. If it is a lot for the time left, perform its actions in quicker succession rather than leaving any out. Never cut, change shot, or restart the action to reach it. Write it as the shot's one movement: if the mapped location differs from the previous scene's, let that same movement carry the shot into the new location instead of adding a second one. Finish by stating where the shot ends.{}",
            performance_line(has_vocals, "transition")
        ));
    }
    parts.join("\n\n")
}
