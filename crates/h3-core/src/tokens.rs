//! MiniMax H3 token and frame math, and the masked-continuation context plan.
//! Port of `minimax/latent_manager.py` (`_tokens_to_frames`, `_frames_to_tokens`, `plan_masked_context`).

use serde::Serialize;

use crate::error::CoreError;

const FRAME_PER_TOKEN: [i64; 5] = [1, 4, 4, 4, 4];

/// Video frames covered by a temporal latent of `tokens` tokens.
pub fn tokens_to_frames(tokens: i64) -> i64 {
    if tokens <= 0 {
        return 0;
    }
    if tokens <= 2 {
        return 5;
    }
    (0..tokens).map(|k| FRAME_PER_TOKEN[(k % 5) as usize]).sum()
}

/// Temporal latent tokens needed for `frames` video frames.
pub fn frames_to_tokens(frames: i64) -> i64 {
    if frames <= 5 {
        return 2;
    }
    if frames == 16 || frames == 17 {
        return 5;
    }
    let mut tokens: i64 = 2;
    let mut total: i64 = 5;
    loop {
        let next = total + FRAME_PER_TOKEN[(tokens % 5) as usize];
        if next > frames {
            break;
        }
        total = next;
        tokens += 1;
    }
    tokens.max(2)
}

/// Context sizes of the masked continuation: 39 + 51k frames.
pub const MASKED_CONTEXT_FRAMES: [i64; 4] = [39, 90, 141, 192];
const MASKED_AUDIO_TICKS_PER_FRAME: f64 = 40.0 / 24.0;

/// Snap a requested context size to a valid one (39 when unknown).
pub fn normalize_masked_context_frames(value: f64) -> i64 {
    if !value.is_finite() {
        return MASKED_CONTEXT_FRAMES[0];
    }
    let frames = value.trunc() as i64;
    if MASKED_CONTEXT_FRAMES.contains(&frames) {
        frames
    } else {
        MASKED_CONTEXT_FRAMES[0]
    }
}

/// Predecessor frames copied unchanged into the head of the next scene's latent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaskedContext {
    pub start_token: i64,
    pub end_token: i64,
    pub tokens: i64,
    pub context_frames: i64,
    pub warmup_frames: i64,
    pub start_frame: i64,
    pub end_frame: i64,
    pub start_audio_tick: i64,
    pub end_audio_tick: i64,
    pub lost_tail_frames: i64,
    pub head_tail_frames: i64,
    pub tail_padding_frames: i64,
}

/// Python `round()`: ties go to the even integer.
fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        (r - x.signum()) as i64
    } else {
        r as i64
    }
}

/// Pick the predecessor window. Fails when the predecessor is too short for `context_frames`.
pub fn plan_masked_context(
    total_tokens: i64,
    tail_padding_frames: Option<i64>,
    context_frames: f64,
) -> Result<MaskedContext, CoreError> {
    let frames = normalize_masked_context_frames(context_frames);
    let wanted = 2 + 5 * ((frames - 5) / 17);
    let total = total_tokens.max(0);
    let pad = tail_padding_frames.unwrap_or(0).max(0);
    let visible = (tokens_to_frames(total) - pad).max(0);

    let mut end = (wanted..=total).find(|t| t % 5 == 2 && tokens_to_frames(*t) >= visible);
    if end.is_none() {
        let mut e = total;
        while e >= wanted && (e % 5 != 2 || tokens_to_frames(e) > visible) {
            e -= 1;
        }
        end = Some(e);
    }
    let end = end.unwrap_or(total);
    if end < wanted {
        return Err(CoreError::value(format!(
            "The predecessor latent ({total} tokens, {visible} visible frames) is too short for a {frames}-frame masked context. Pick a smaller context size or re-render the predecessor longer."
        )));
    }
    let start = end - wanted;
    let start_frame = tokens_to_frames(start);
    let end_frame = tokens_to_frames(end);
    let head_frames = end_frame - start_frame;
    let head_tail = (end_frame - visible).max(0);
    let lost_tail = (visible - end_frame).max(0);
    Ok(MaskedContext {
        start_token: start,
        end_token: end,
        tokens: wanted,
        context_frames: head_frames,
        warmup_frames: head_frames - head_tail + lost_tail,
        start_frame,
        end_frame,
        start_audio_tick: round_half_even(start_frame as f64 * MASKED_AUDIO_TICKS_PER_FRAME),
        end_audio_tick: round_half_even(end_frame as f64 * MASKED_AUDIO_TICKS_PER_FRAME),
        lost_tail_frames: lost_tail,
        head_tail_frames: head_tail,
        tail_padding_frames: pad,
    })
}
