//! Window planning for the chunked MiniMax H3 second pass. Port of the pure part of `minimax/chunk_plan.py`.
//!
//! A long 2 Pass render is refined in overlapping temporal windows instead of one pass over the whole clip. H3 video
//! tokens run on a 1,4,4,4,4 temporal cycle (5 tokens = 17 pixel frames), so a window starts on a multiple of 5 tokens and
//! holds `5k + 2` tokens. The graph builder uses [`chunking_decision`] to decide whether a scene is long enough to chunk.

use serde_json::{json, Value};

use crate::pyval::{fmt_g, round_half_even, round_to, to_float, Obj};

pub const FPS: f64 = 24.0;
pub const DEFAULT_MIN_SECONDS: f64 = 8.0;
pub const MIN_SECONDS_RANGE: (f64, f64) = (1.0, 15.0);

/// What the Builder asks the chunk node for. These are the values the Heat A/B test used.
pub const CHUNK_SECONDS: f64 = 3.0;
pub const OVERLAP_SECONDS: f64 = 1.6;
pub const OVERLAP_MODE: &str = "locked";
pub const TAIL_TRIM_SECONDS: f64 = 0.7;
pub const UPSCALE_MODE: &str = "per_window";

/// Video tokens of a `17k + 5` frame clip (`5k + 2` tokens). `None` when the frame count is not on the H3 grid.
pub fn tokens_for_frames(frames: i64) -> Option<i64> {
    (frames >= 5 && (frames - 5) % 17 == 0).then(|| (frames - 5) / 17 * 5 + 2)
}

/// Phase-aligned windows as `(start, end, keep_end)`. Starts are multiples of 5 and lengths are `5k + 2`. `keep_end` is
/// where a window stops being final. The last window keeps everything. `None` when `total_tokens` is not `5k + 2`.
pub fn window_plan(total_tokens: i64, seconds: f64, overlap_seconds: f64, trim_seconds: f64) -> Option<Vec<(i64, i64, i64)>> {
    if total_tokens < 2 || total_tokens % 5 != 2 {
        return None;
    }
    let groups = round_half_even((seconds * FPS - 5.0) / 17.0).max(1);
    let overlap_groups = (groups - 1).min(round_half_even((overlap_seconds * FPS - 5.0) / 17.0).max(0));
    let trim_groups = (overlap_groups - 1).max(0).min(round_half_even(trim_seconds * FPS / 17.0).max(0));
    let window = groups * 5 + 2;
    let stride = (groups - overlap_groups) * 5;
    let mut plan = Vec::new();
    let mut start = 0;
    loop {
        let end = total_tokens.min(start + window);
        let last = end == total_tokens;
        plan.push((start, end, if last { end } else { end - trim_groups * 5 }));
        if last {
            return Some(plan);
        }
        start += stride;
    }
}

/// `auto` or `off`. Anything else, including a missing value, is `off`.
pub fn normalize_pass2_chunking(value: Option<&Value>) -> &'static str {
    let text = match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    if text.trim().eq_ignore_ascii_case("auto") { "auto" } else { "off" }
}

/// The length above which a render is chunked, clamped to 1 to 15 s.
pub fn normalize_min_seconds(value: Option<&Value>) -> f64 {
    let seconds = value.and_then(to_float).filter(|f| !f.is_nan()).unwrap_or(DEFAULT_MIN_SECONDS);
    seconds.max(MIN_SECONDS_RANGE.0).min(MIN_SECONDS_RANGE.1)
}

/// Whether a render of `h3_frame_count` frames gets the chunked second pass.
///
/// The length is what H3 renders (the scene plus warm-up and alignment padding), because that is what costs memory. A
/// render longer than the threshold is chunked, one at or under it keeps the single whole-clip pass.
pub fn chunking_decision(mode: Option<&Value>, min_seconds: Option<&Value>, h3_frame_count: Option<i64>) -> Obj {
    let mode = normalize_pass2_chunking(mode);
    let threshold = normalize_min_seconds(min_seconds);
    let frames = h3_frame_count.unwrap_or(0);
    let seconds = frames as f64 / FPS;
    let mut decision = Obj::new();
    decision.insert("mode".into(), json!(mode));
    decision.insert("min_seconds".into(), json!(threshold));
    decision.insert("rendered_seconds".into(), json!(round_to(seconds, 3)));
    decision.insert("enabled".into(), json!(false));
    let reason = if mode == "off" {
        "Pass 2 chunking is off.".to_string()
    } else if frames <= 0 {
        "The render length is unknown.".to_string()
    } else if seconds <= threshold {
        format!("{} s is not longer than {} s, so pass 2 runs in one piece.", two_places(seconds), fmt_g(threshold))
    } else {
        decision.insert("enabled".into(), json!(true));
        let windows = tokens_for_frames(frames)
            .and_then(|tokens| window_plan(tokens, CHUNK_SECONDS, OVERLAP_SECONDS, TAIL_TRIM_SECONDS))
            .map(|plan| plan.len());
        decision.insert("windows".into(), json!(windows));
        format!("{} s is longer than {} s, so pass 2 runs in windows.", two_places(seconds), fmt_g(threshold))
    };
    decision.insert("reason".into(), json!(reason));
    decision
}

/// `f"{seconds:.2f}"`. Python rounds an exact tie (1.625) to the even digit.
fn two_places(seconds: f64) -> String {
    format!("{:.2}", round_to(seconds, 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_settings_plan_windows_on_the_token_grid() {
        // 328 frames (13.67 s) is 97 tokens. 3 s windows hold 22 tokens and the stride is 10, the overlap is 12.
        let plan = window_plan(tokens_for_frames(328).unwrap(), CHUNK_SECONDS, OVERLAP_SECONDS, TAIL_TRIM_SECONDS).unwrap();
        assert_eq!(plan.first(), Some(&(0, 22, 17)));
        assert!(plan.iter().all(|(start, _, _)| start % 5 == 0));
        assert_eq!(plan.last().map(|w| w.1), Some(97));
        assert_eq!(chunking_decision(Some(&json!("auto")), None, Some(328))["windows"], json!(plan.len()));
    }

    #[test]
    fn rejects_frame_counts_off_the_h3_grid() {
        assert_eq!(tokens_for_frames(100), None);
        assert_eq!(tokens_for_frames(107), Some(32));
        assert!(window_plan(33, 3.0, 1.6, 0.7).is_none());
    }

    #[test]
    fn threshold_decides_and_is_clamped() {
        let auto = json!("auto");
        let frames = Some(193); // 8.04 s
        assert_eq!(chunking_decision(Some(&auto), Some(&json!(8)), frames)["enabled"], json!(true));
        assert_eq!(chunking_decision(Some(&auto), Some(&json!(9)), frames)["enabled"], json!(false));
        assert_eq!(chunking_decision(Some(&json!("off")), Some(&json!(1)), frames)["enabled"], json!(false));
        assert_eq!(chunking_decision(None, None, frames)["mode"], json!("off"));
        assert_eq!(normalize_min_seconds(Some(&json!(0.2))), 1.0);
        assert_eq!(normalize_min_seconds(Some(&json!("40"))), 15.0);
        assert_eq!(normalize_min_seconds(Some(&json!("junk"))), 8.0);
    }

    #[test]
    fn reason_text_matches_the_pack() {
        let auto = json!("auto");
        let short = chunking_decision(Some(&auto), Some(&json!(8)), Some(124));
        assert_eq!(short["reason"], json!("5.17 s is not longer than 8 s, so pass 2 runs in one piece."));
        let tie = chunking_decision(Some(&auto), Some(&json!(8)), Some(39)); // 1.625 s
        assert_eq!(tie["reason"], json!("1.62 s is not longer than 8 s, so pass 2 runs in one piece."));
        let long = chunking_decision(Some(&auto), Some(&json!(3.5)), Some(124));
        assert_eq!(long["reason"], json!("5.17 s is longer than 3.5 s, so pass 2 runs in windows."));
        assert_eq!(long["rendered_seconds"], json!(5.167));
    }
}
