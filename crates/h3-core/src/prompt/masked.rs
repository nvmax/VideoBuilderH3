//! What a scene that continues the previous one with `latent_continuation_masked` needs to know. Port of
//! `continuation_start_limits`, `continuation_hold_seconds` and `masked_continuation_context` in `minimax/prompt_assembly.py`.

use serde_json::{json, Value};

use crate::pyval::{collapse_ws, fmt_g, round_to, text_or_empty, to_float, Obj};

pub const CONTINUATION_START_FIELD: &str = "minimax_h3_continuation_start_seconds";
const CONTINUATION_MIN_START_SECONDS: f64 = 0.0;

/// `(lowest, highest)` second of a continued scene where its own movement may begin: from the first frame up to half
/// of the scene, rounded down to 0.1 s.
pub fn continuation_start_limits(duration: f64) -> (f64, f64) {
    let low = CONTINUATION_MIN_START_SECONDS;
    let high = low.max(((duration.max(0.0) * 0.5 * 10.0 + 1e-9).floor()) / 10.0);
    (low, high)
}

/// Seconds a continued scene simply carries on before its own movement begins: the author's choice kept between the
/// limits, 0 s when nothing is set.
pub fn continuation_hold_seconds(duration: f64, requested: Option<&Value>) -> f64 {
    let (low, high) = continuation_start_limits(duration);
    let value = match requested {
        None | Some(Value::Null) => low,
        Some(Value::String(s)) if s.is_empty() => low,
        Some(v) => to_float(v).filter(|n| !n.is_nan()).unwrap_or(low),
    };
    round_to(high.min(low.max(value)), 1)
}

/// The rules for writing a scene that continues the previous scene's saved latent.
pub fn masked_continuation_context(segment: &Obj, duration: f64, shot_count: usize, has_vocals: bool) -> Obj {
    let hold = continuation_hold_seconds(duration, segment.get(CONTINUATION_START_FIELD));
    let scene_length = round_to(duration.max(0.0), 2);
    let seconds_left = round_to((scene_length - hold).max(0.0), 2);
    let (low, high) = continuation_start_limits(duration);
    let direction = collapse_ws(&text_or_empty(segment.get("minimax_h3_continuation_direction")));
    let (hold_g, length_g, left_g) = (fmt_g(hold), fmt_g(scene_length), fmt_g(seconds_left));
    let mut rules: Vec<String> = vec![
        "This scene continues the previous scene's saved latent. Its first frames are the previous scene's last moments, already rendered. Write it as the very next moment of one uninterrupted take, never as a new shot.".into(),
        if hold > 0.0 {
            format!("Write the timing into the shot text: 'For the first {hold_g} seconds, ...' carries on the previous scene's action with the same camera movement, framing and pace (no cut, reframe or restage), then 'At about {hold_g} seconds, ...' gives the one smooth movement this scene makes.")
        } else {
            "The scene's own movement starts from the first frame with no timed lead-in, carrying on from the previous scene's action with the same camera movement, framing and pace (no cut, reframe or restage).".into()
        },
        format!("The scene is {length_g} seconds long, so that movement has {left_g} seconds and must be completely finished before the scene ends. Keep every action of the author's direction, in their order, and perform them briskly enough to fit. Never leave one cut off or unfinished."),
        "Start from the body position and camera in the previous scene's final frame. If the movement needs a different position (standing up, walking, turning), describe the natural movement that gets there first.".into(),
        "No wipes, whip pans, portals, morphs or cuts. A change of location is one visible movement that carries the shot into the new place.".into(),
    ];
    if has_vocals {
        rules.push("The performer is mid-performance: keep singing (or speaking) the scene's lyrics through the whole movement, lips in sync with <Audio 1>, with the face kept in view.".into());
    }
    if shot_count > 1 {
        rules.push(format!("The cut plan has {shot_count} shots. A continued scene should be one continuous shot, set the cut frequency to 0."));
    }
    let mut out = Obj::new();
    out.insert("mode".into(), json!("latent_continuation_masked"));
    out.insert("hold_seconds".into(), json!(hold));
    out.insert("start_seconds".into(), json!(hold));
    out.insert("scene_seconds".into(), json!(scene_length));
    out.insert("seconds_left".into(), json!(seconds_left));
    out.insert("start_field".into(), json!(CONTINUATION_START_FIELD));
    out.insert("start_limits".into(), json!({"min": low, "max": high}));
    out.insert("direction".into(), json!(direction));
    out.insert("direction_field".into(), json!("minimax_h3_continuation_direction"));
    out.insert("rules".into(), json!(rules));
    out
}
