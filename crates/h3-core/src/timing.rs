//! Exact scene timing for MiniMax H3 renders. Port of `calculate_minimax_h3_timing` and its helpers in
//! `minimax/latent_manager.py`. Uses decimal arithmetic like the pack, so results match to the last digit.

use std::str::FromStr;

use rust_decimal::prelude::*;
use rust_decimal::RoundingStrategy;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

pub const H3_FPS: u32 = 24;
pub const H3_FRAME_STEP: i64 = 17;
pub const H3_FRAME_OFFSET: i64 = 5;
pub const H3_MIN_FRAME_COUNT: i64 = 5;
pub const H3_MAX_FRAME_COUNT: i64 = 362;
const SOURCE_AUDIO_TOLERANCE: &str = "0.001";

fn dec(value: f64, name: &str) -> Result<Decimal, CoreError> {
    let bad = || CoreError::value(format!("{name} must be a finite number."));
    if !value.is_finite() {
        return Err(bad());
    }
    // Same as Python `Decimal(str(value))`: the shortest round-trip text.
    Decimal::from_str(&format!("{value}")).map_err(|_| bad())
}

/// Python's decimal context keeps 28 significant digits and rounds every result. The pack relies on it: a context of
/// 4.04166...67 s times 24 comes out a hair above 97, so its ceiling is 98 frames. Match it after every operation.
fn q(value: Decimal) -> Decimal {
    let digits = value.mantissa().unsigned_abs().to_string().len() as i32;
    if digits <= 28 {
        return value;
    }
    let scale = value.scale() as i32 - (digits - 28);
    value.round_dp_with_strategy(scale.max(0) as u32, RoundingStrategy::MidpointNearestEven)
}

fn is_negative(value: Decimal) -> bool {
    value.is_sign_negative() && !value.is_zero()
}

fn non_negative_int(value: f64, name: &str) -> Result<i64, CoreError> {
    let bad = || CoreError::value(format!("{name} must be a non-negative whole number."));
    let number = dec(value, name)?;
    if is_negative(number) || number != number.trunc() {
        return Err(bad());
    }
    number.to_i64().ok_or_else(bad)
}

/// Seconds as a stable `f64`: nine decimals, ties to even.
fn seconds(value: Decimal) -> f64 {
    let rounded = value.round_dp_with_strategy(9, RoundingStrategy::MidpointNearestEven);
    rounded.to_string().parse().unwrap_or(0.0)
}

/// Round a frame count up to the 17n+5 grid.
pub fn align_h3_frame_count(frame_count: f64) -> Result<i64, CoreError> {
    let frames = H3_MIN_FRAME_COUNT.max(non_negative_int(frame_count, "frame_count")?);
    Ok(frames + (H3_FRAME_OFFSET - frames).rem_euclid(H3_FRAME_STEP))
}

/// Whole frames needed to cover a duration.
pub fn frames_covering_duration(duration_seconds: f64, fps: f64) -> Result<i64, CoreError> {
    let duration = dec(duration_seconds, "duration_seconds")?;
    let rate = non_negative_int(fps, "fps")?;
    if is_negative(duration) {
        return Err(CoreError::value("duration_seconds must not be negative."));
    }
    if rate <= 0 {
        return Err(CoreError::value("fps must be greater than zero."));
    }
    Ok((duration * Decimal::from(rate)).ceil().to_i64().unwrap_or(0))
}

/// Inputs of one scene's timing plan.
#[derive(Debug, Clone, Default)]
pub struct TimingInput {
    pub timeline_start_seconds: f64,
    pub timeline_end_seconds: f64,
    pub warmup_frames: f64,
    pub cooldown_frames: f64,
    pub source_start_seconds: Option<f64>,
    pub source_duration_seconds: Option<f64>,
    pub max_frame_count: Option<f64>,
    pub pad_warmup: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimingPlan {
    pub timeline_start_seconds: f64,
    pub timeline_end_seconds: f64,
    pub scene_duration_seconds: f64,
    pub source_start_seconds: f64,
    pub source_duration_seconds: Option<f64>,
    pub requested_warmup_frames: i64,
    pub requested_cooldown_frames: i64,
    pub actual_warmup_seconds: f64,
    pub actual_cooldown_seconds: f64,
    pub audio_trim_start_seconds: f64,
    pub audio_trim_duration_seconds: f64,
    pub context_duration_seconds: f64,
    pub context_frame_count: i64,
    pub workflow_duration_input_seconds: f64,
    pub h3_frame_count: i64,
    pub h3_render_duration_seconds: f64,
    pub alignment_padding_seconds: f64,
    pub final_trim_start_seconds: f64,
    pub final_trim_duration_seconds: f64,
    pub discard_after_scene_seconds: f64,
    pub final_frame_count: i64,
    pub audio_leading_padding_seconds: f64,
}

/// The complete render and trim timing plan for one Builder scene at 24 fps.
pub fn calculate_timing(input: &TimingInput) -> Result<TimingPlan, CoreError> {
    let rate = Decimal::from(H3_FPS);
    let timeline_start = dec(input.timeline_start_seconds, "timeline_start_seconds")?;
    let timeline_end = dec(input.timeline_end_seconds, "timeline_end_seconds")?;
    if is_negative(timeline_start) {
        return Err(CoreError::value("timeline_start_seconds must not be negative."));
    }
    if timeline_end <= timeline_start {
        return Err(CoreError::value("timeline_end_seconds must be greater than timeline_start_seconds."));
    }
    let scene_duration = q(timeline_end - timeline_start);

    let warm_frames = non_negative_int(input.warmup_frames, "warmup_frames")?;
    let cool_frames = non_negative_int(input.cooldown_frames, "cooldown_frames")?;
    let requested_warmup = q(Decimal::from(warm_frames) / rate);
    let requested_cooldown = q(Decimal::from(cool_frames) / rate);

    let source_start = match input.source_start_seconds {
        None => timeline_start,
        Some(v) => dec(v, "source_start_seconds")?,
    };
    if is_negative(source_start) {
        return Err(CoreError::value("source_start_seconds must not be negative."));
    }

    let mut source_duration = None;
    if let Some(v) = input.source_duration_seconds {
        let d = dec(v, "source_duration_seconds")?;
        if is_negative(d) {
            return Err(CoreError::value("source_duration_seconds must not be negative."));
        }
        let tolerance = Decimal::from_str(SOURCE_AUDIO_TOLERANCE).unwrap_or_default();
        if source_start + scene_duration > d + tolerance {
            return Err(CoreError::value("The selected scene extends beyond the available source audio."));
        }
        source_duration = Some(d);
    }

    let available_warmup = requested_warmup.min(source_start);
    let actual_warmup = if input.pad_warmup { requested_warmup } else { available_warmup };
    let audio_leading_padding = q(actual_warmup - available_warmup);
    let mut actual_cooldown = requested_cooldown;
    if let Some(d) = source_duration {
        let after_scene = q(d - q(source_start + scene_duration));
        actual_cooldown = requested_cooldown.min(after_scene.max(Decimal::ZERO));
    }

    let audio_trim_start = q(source_start - available_warmup);
    let context_duration = q(q(actual_warmup + scene_duration) + actual_cooldown);
    let context_frames = q(context_duration * rate).ceil().to_i64().unwrap_or(0);
    let h3_frames = align_h3_frame_count(context_frames as f64)?;
    let maximum = non_negative_int(input.max_frame_count.unwrap_or(H3_MAX_FRAME_COUNT as f64), "max_frame_count")?;
    if h3_frames > maximum {
        return Err(CoreError::value(format!(
            "The scene plus available warm-up/cool-down requires {h3_frames} H3 frames, exceeding the configured maximum of {maximum}."
        )));
    }

    let render_duration = q(Decimal::from(h3_frames) / rate);
    let workflow_duration = q(Decimal::from(context_frames) / rate);
    let alignment_padding = q(render_duration - context_duration);
    let mut audio_tail = Decimal::ZERO;
    if let Some(d) = source_duration {
        let after_context = q(d - q(q(source_start + scene_duration) + actual_cooldown));
        audio_tail = alignment_padding.min(after_context.max(Decimal::ZERO));
    }
    let discard_after_scene = q(render_duration - q(actual_warmup + scene_duration));
    let half_up = |v: Decimal| {
        q(v * rate).round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero).to_i64().unwrap_or(0)
    };
    let final_frame_count = (half_up(timeline_end) - half_up(timeline_start)).max(1);

    Ok(TimingPlan {
        timeline_start_seconds: seconds(timeline_start),
        timeline_end_seconds: seconds(timeline_end),
        scene_duration_seconds: seconds(scene_duration),
        source_start_seconds: seconds(source_start),
        source_duration_seconds: source_duration.map(seconds),
        requested_warmup_frames: warm_frames,
        requested_cooldown_frames: cool_frames,
        actual_warmup_seconds: seconds(actual_warmup),
        actual_cooldown_seconds: seconds(actual_cooldown),
        audio_trim_start_seconds: seconds(audio_trim_start),
        audio_trim_duration_seconds: seconds(q(context_duration + audio_tail)),
        context_duration_seconds: seconds(context_duration),
        context_frame_count: context_frames,
        workflow_duration_input_seconds: seconds(workflow_duration),
        h3_frame_count: h3_frames,
        h3_render_duration_seconds: seconds(render_duration),
        alignment_padding_seconds: seconds(alignment_padding),
        final_trim_start_seconds: seconds(actual_warmup),
        final_trim_duration_seconds: seconds(scene_duration),
        discard_after_scene_seconds: seconds(discard_after_scene),
        final_frame_count,
        audio_leading_padding_seconds: seconds(audio_leading_padding),
    })
}
