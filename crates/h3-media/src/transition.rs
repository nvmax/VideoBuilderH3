//! Transitions in a stitch.
//!
//! A transition is a block of the timeline: it starts `start` seconds in and lasts `seconds`, on the clock of the scene timing. It replaces the same
//! number of frames of the joined video, so the video keeps the timeline's length and the song stays in step. Where the block lies decides what it draws:
//!
//! - On a cut between two scene clips (the cut is inside the block or at one of its ends) it draws the outgoing clip into the incoming one. The clips
//!   have no frames beyond their own length, so the outgoing clip's last frame is held after the cut and the incoming clip's first frame before it.
//! - Inside one clip it draws the clip's own frames on both sides, so the transition acts as an effect on the footage (a flash, a zoom pulse, a glitch).
//!
//! A block that reaches over more than one cut is cut down to the cut nearest its middle, and a block that overlaps the one before it starts where that
//! one ends. `plan` is pure. `render_window` decodes the two sides with ffmpeg, draws every frame on the GPU and encodes the frames as one clip that goes
//! between the pieces of the scenes around it.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread::JoinHandle;

use h3_transitions::{params_from_json, transition, TransitionRenderer};
use serde_json::Value;

use crate::tools::{args, command, MediaError, Tools};
use crate::video::video_size;

/// A transition needs at least this many frames to be a transition.
pub const MIN_FRAMES: u32 = 2;
pub const DEFAULT_SECONDS: f64 = 0.5;
pub const SECONDS_RANGE: (f64, f64) = (0.1, 10.0);

/// One transition as the caller asks for it.
#[derive(Debug, Clone)]
pub struct TransitionItem {
    pub name: String,
    pub params: Value,
    /// Seconds from the start of the stitch's timeline (the clock of the scene timing).
    pub start: f64,
    pub seconds: f64,
}

impl TransitionItem {
    /// `{"transition": "Swirl", "start": 12.5, "seconds": 0.5, "params": {}}`. Only `transition` and `start` are required.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let map = value.as_object().ok_or("A transition must be an object.")?;
        let name = map.get("transition").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or("A transition needs `transition`, its name.")?;
        let entry = transition(name).map_err(|e| e.to_string())?;
        let start = map.get("start").and_then(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0).ok_or("A transition needs `start`, its time on the timeline in seconds.")?;
        let seconds = match map.get("seconds") {
            None | Some(Value::Null) => DEFAULT_SECONDS,
            Some(v) => v.as_f64().filter(|s| s.is_finite()).ok_or("`seconds` must be a number.")?,
        };
        let params = map.get("params").cloned().unwrap_or(Value::Null);
        params_from_json(entry, &params).map_err(|e| e.to_string())?;
        Ok(Self { name: name.to_string(), params, start, seconds: seconds.clamp(SECONDS_RANGE.0, SECONDS_RANGE.1) })
    }
}

/// The transitions of a stitch payload: absent or `null` means none.
pub fn items_from_json(value: Option<&Value>) -> Result<Vec<TransitionItem>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => list.iter().enumerate().map(|(i, item)| TransitionItem::from_json(item).map_err(|e| format!("Transition {}: {e}", i + 1))).collect(),
        Some(_) => Err("transitions must be a list.".to_string()),
    }
}

/// A transition as it will be drawn: the frames of the joined video it replaces.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub name: String,
    pub params: Value,
    /// The first frame of the joined video it replaces.
    pub first: i64,
    pub frames: u32,
    /// `Some(k)`: it sits on the cut between clips `k` and `k + 1`. `None`: it is inside one clip.
    pub cut: Option<usize>,
}

/// What was drawn, for the reply of a stitch.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedTransition {
    pub name: String,
    /// Where it starts in the joined video, in seconds.
    pub start: f64,
    pub frames: u32,
    pub cut: Option<usize>,
}

impl AppliedTransition {
    /// For the reply of a stitch. `after_scene_number` is the scene before the cut it sits on (1 = the first scene of the stitch), `null` inside a scene.
    pub fn to_json(&self) -> Value {
        serde_json::json!({"transition": self.name, "start": (self.start * 1000.0).round() / 1000.0, "frames": self.frames, "after_scene_number": self.cut.map(|cut| cut + 1)})
    }
}

/// The frames a clip takes on the timeline. The rounding is the stitch's own, so a clip has the same length here as in the whole project.
pub fn clip_frames(start: f64, end: f64, fps: u32) -> i64 {
    let start = start.max(0.0);
    let end = end.max(start);
    ((end * f64::from(fps) + 0.5) as i64 - (start * f64::from(fps) + 0.5) as i64).max(1)
}

/// Where each clip starts in the joined video (entry `i` is the first frame of clip `i`), and its length as the last entry.
pub fn clip_bounds(lengths: &[i64]) -> Vec<i64> {
    let mut bounds = vec![0];
    for length in lengths {
        bounds.push(bounds[bounds.len() - 1] + length);
    }
    bounds
}

/// The index of the clip that holds `frame` of the joined video.
pub fn clip_at(bounds: &[i64], frame: i64) -> usize {
    bounds.partition_point(|b| *b <= frame).saturating_sub(1).min(bounds.len().saturating_sub(2))
}

/// A time of the timeline as a frame of the joined video. A time between two clips, before the first or after the last lands on the nearest clip edge.
fn frame_at(time: f64, clips: &[(f64, f64)], bounds: &[i64], fps: u32) -> i64 {
    let round = |t: f64| (t.max(0.0) * f64::from(fps) + 0.5) as i64;
    for (i, (start, end)) in clips.iter().enumerate() {
        let start = start.max(0.0);
        let end = end.max(start);
        if time < start {
            return bounds[i];
        }
        if time < end {
            return bounds[i] + (round(time) - round(start)).clamp(0, bounds[i + 1] - bounds[i]);
        }
    }
    bounds[bounds.len() - 1]
}

/// The windows to draw, in order, and a note for every transition that was shortened, moved or left out. `clips` are the start and end of every clip
/// on the timeline (the scene timing).
pub fn plan(items: &[TransitionItem], clips: &[(f64, f64)], fps: u32) -> (Vec<Window>, Vec<String>) {
    let lengths: Vec<i64> = clips.iter().map(|(start, end)| clip_frames(*start, *end, fps)).collect();
    let bounds = clip_bounds(&lengths);
    let mut spans: Vec<(usize, i64, i64)> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (i, frame_at(item.start, clips, &bounds, fps), frame_at(item.start + item.seconds, clips, &bounds, fps)))
        .collect();
    spans.sort_by_key(|(i, first, _)| (*first, *i));
    let (mut windows, mut notes) = (Vec::new(), Vec::new());
    // The frames before this one belong to the transitions before.
    let mut taken = 0i64;
    for (index, first, last) in spans {
        let item = &items[index];
        let label = format!("The transition {} at {:.2} s", item.name, item.start);
        let wanted = (item.seconds * f64::from(fps)).round().max(0.0) as i64;
        let left_out = |notes: &mut Vec<String>| notes.push(format!("{label} was left out: fewer than {MIN_FRAMES} frames of the scenes are free for it."));
        let first = first.max(taken);
        if last - first < i64::from(MIN_FRAMES) {
            left_out(&mut notes);
            continue;
        }
        // The cut nearest the middle of the block, among those it covers or touches.
        let nearest = (1..clips.len()).filter(|&b| first <= bounds[b] && bounds[b] <= last).min_by_key(|&b| ((2 * bounds[b] - (first + last)).abs(), std::cmp::Reverse(b)));
        let (first, last, cut) = match nearest {
            Some(b) => (first.max(bounds[b - 1]), last.min(bounds[b + 1]), Some(b - 1)),
            None => (first, last, None),
        };
        if last - first < i64::from(MIN_FRAMES) {
            left_out(&mut notes);
            continue;
        }
        let frames = last - first;
        if frames < wanted {
            notes.push(format!("{label} was shortened from {wanted} to {frames} frames to fit the scenes and the transitions around it."));
        }
        taken = last;
        windows.push(Window { name: item.name.clone(), params: item.params.clone(), first, frames: frames as u32, cut });
    }
    (windows, notes)
}

/// One piece of the joined video, in order: frames of a scene clip as they are, or a drawn transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    /// Frames `[from, to)` of clip `clip`.
    Scene { clip: usize, from: i64, to: i64 },
    /// The window with this index in the plan.
    Transition(usize),
}

/// The pieces that make up the joined video: the clips (`bounds` as `clip_bounds` made them) with the windows taken out and drawn instead.
pub fn pieces(bounds: &[i64], windows: &[Window]) -> Vec<Piece> {
    let total = bounds.last().copied().unwrap_or(0);
    let mut out = Vec::new();
    let scenes = |out: &mut Vec<Piece>, from: i64, to: i64| {
        for clip in 0..bounds.len().saturating_sub(1) {
            let (lo, hi) = (from.max(bounds[clip]), to.min(bounds[clip + 1]));
            if hi > lo {
                out.push(Piece::Scene { clip, from: lo - bounds[clip], to: hi - bounds[clip] });
            }
        }
    };
    let mut cursor = 0;
    for (index, window) in windows.iter().enumerate() {
        scenes(&mut out, cursor, window.first);
        out.push(Piece::Transition(index));
        cursor = window.first + i64::from(window.frames);
    }
    scenes(&mut out, cursor, total);
    out
}

// -- drawing ----------------------------------------------------------------------------------------------------------------

/// An ffmpeg process with its error text collected on the side, so a full stderr pipe cannot stop it.
pub(crate) struct Piped {
    pub(crate) child: Child,
    errors: Option<JoinHandle<String>>,
}

impl Piped {
    pub(crate) fn spawn(program: &Path, arguments: &[String], stdin: Stdio, stdout: Stdio) -> Result<Self, MediaError> {
        let mut child = command(program, arguments).stdin(stdin).stdout(stdout).stderr(Stdio::piped()).spawn()?;
        let errors = child.stderr.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut text = String::new();
                let _ = pipe.read_to_string(&mut text);
                text
            })
        });
        Ok(Self { child, errors })
    }

    /// Wait for the process. `Err` carries what it printed when it failed.
    pub(crate) fn finish(mut self, what: &str) -> Result<(), MediaError> {
        let status = self.child.wait()?;
        let text = self.errors.take().and_then(|h| h.join().ok()).unwrap_or_default();
        if status.success() {
            return Ok(());
        }
        let text = text.trim();
        Err(MediaError::Failed(if text.is_empty() { what.to_string() } else { format!("{what} {text}") }))
    }

    pub(crate) fn abandon(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.errors.take() {
            let _ = handle.join();
        }
    }
}

/// Scales a picture to the canvas the transition is drawn on (the outgoing clip's size) and turns it into RGBA with the matrix the encoder
/// below turns it back with, so the colours of the clips and of the transition between them agree.
pub(crate) fn to_canvas(width: u32, height: u32) -> String {
    format!("scale={width}:{height}:force_original_aspect_ratio=increase:in_color_matrix=bt709,crop={width}:{height},setsar=1,format=rgba")
}

/// RGBA back to YUV with the matrix `to_canvas` took it out with, then the colour tags `scale` set are cleared.
pub(crate) const UNTAGGED_YUV: &str = "scale=out_color_matrix=bt709,setparams=colorspace=unknown:range=unknown:color_primaries=unknown:color_trc=unknown,format=yuv420p";

pub(crate) fn decoder(tools: &Tools, clip: &Path, chain: &str, frames: u32) -> Result<Piped, MediaError> {
    let a = args(&[
        "-v", "error", "-i", &clip.display().to_string(), "-map", "0:v:0", "-an", "-vf", chain, "-fps_mode", "passthrough", "-frames:v", &frames.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1",
    ]);
    Piped::spawn(&tools.ffmpeg, &a, Stdio::null(), Stdio::piped())
}

/// What a window draws from.
#[derive(Debug, Clone, Copy)]
pub enum Source<'a> {
    /// A cut. The outgoing clip (`outgoing_frames` frames on the timeline) from its frame `from` on, held on its last frame once its time on the timeline is
    /// over, and the incoming clip held on its first frame for the `before` frames of the window that come before the cut, then playing from its start.
    Cut { outgoing: &'a Path, outgoing_frames: i64, from: i64, incoming: &'a Path, before: u32 },
    /// Inside one clip: its frames from `from` on, on both sides.
    Inside { clip: &'a Path, from: i64 },
}

/// The ffmpeg filter chains that read the two sides of a window, each ending in the canvas scaling.
fn side_chains(source: &Source, frames: u32, fps: u32, canvas: &str) -> (String, String) {
    match *source {
        Source::Cut { outgoing_frames, from, before, .. } => {
            let after = frames.saturating_sub(before);
            let from = from.max(0);
            // Outgoing side: its frames up to the cut, then the last frame of its time on the timeline held (a file longer than that is cut there first).
            let hold_seconds = f64::from(frames) / f64::from(fps) + 1.0;
            let outgoing = format!(
                "fps={fps},trim=end_frame={},setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration={hold_seconds:.3},trim=start_frame={from}:end_frame={},setpts=PTS-STARTPTS,{canvas}",
                outgoing_frames.max(1),
                from + i64::from(frames)
            );
            // Incoming side: its first frame held up to the cut, then its first frames.
            let incoming = format!("fps={fps},trim=start_frame=0:end_frame={},setpts=PTS-STARTPTS,tpad=start={before}:start_mode=clone,{canvas}", after.max(1));
            (outgoing, incoming)
        }
        Source::Inside { from, .. } => {
            let both = format!("fps={fps},trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS,{canvas}", from.max(0), from.max(0) + i64::from(frames));
            (both.clone(), both)
        }
    }
}

/// Draw `frames` frames of the transition `name` from `source`, and encode them to `output`. The renderer is created on the first call and kept for the
/// next ones.
#[allow(clippy::too_many_arguments)]
pub fn render_window(
    tools: &Tools,
    renderer: &mut Option<TransitionRenderer>,
    source: Source,
    name: &str,
    params: &Value,
    frames: u32,
    fps: u32,
    output: &Path,
) -> Result<(), MediaError> {
    let failed = |e: h3_transitions::TransitionError| MediaError::Failed(e.to_string());
    let params = params_from_json(transition(name).map_err(failed)?, params).map_err(failed)?;
    if renderer.is_none() {
        *renderer = Some(TransitionRenderer::new().map_err(failed)?);
    }
    let renderer = renderer.as_mut().ok_or_else(|| MediaError::Failed("The transition renderer did not start.".into()))?;

    let (first_clip, second_clip) = match source {
        Source::Cut { outgoing, incoming, .. } => (outgoing, incoming),
        Source::Inside { clip, .. } => (clip, clip),
    };
    let (width, height) = video_size(tools, first_clip)?;
    let size = width as usize * height as usize * 4;
    let canvas = to_canvas(width, height);
    let (from_chain, to_chain) = side_chains(&source, frames, fps, &canvas);

    let mut from = decoder(tools, first_clip, &from_chain, frames)?;
    let mut to = match decoder(tools, second_clip, &to_chain, frames) {
        Ok(p) => p,
        Err(e) => {
            from.abandon();
            return Err(e);
        }
    };
    // The scene clips carry no colour tags, so neither does this one: a tag appearing in the middle of the joined video would change how a player shows
    // everything after it.
    let encode = args(&[
        "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{width}x{height}"), "-r", &fps.to_string(), "-i", "pipe:0", "-an", "-vf", UNTAGGED_YUV, "-c:v",
        "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-r", &fps.to_string(), &output.display().to_string(),
    ]);
    let mut encoder = match Piped::spawn(&tools.ffmpeg, &encode, Stdio::piped(), Stdio::null()) {
        Ok(p) => p,
        Err(e) => {
            from.abandon();
            to.abandon();
            return Err(e);
        }
    };

    let drawn = (|| -> Result<(), MediaError> {
        let (mut from_pipe, mut to_pipe) = (from.child.stdout.take(), to.child.stdout.take());
        let mut sink = encoder.child.stdin.take();
        let (from_pipe, to_pipe, sink) = match (from_pipe.as_mut(), to_pipe.as_mut(), sink.as_mut()) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            _ => return Err(MediaError::Failed("FFmpeg did not open its pipes.".into())),
        };
        let (mut a, mut b, mut frame) = (vec![0u8; size], vec![0u8; size], Vec::with_capacity(size));
        for k in 0..frames {
            let short = |side: &str| MediaError::Failed(format!("FFmpeg returned too few frames of the {side} scene for the transition."));
            from_pipe.read_exact(&mut a).map_err(|_| short("outgoing"))?;
            to_pipe.read_exact(&mut b).map_err(|_| short("incoming"))?;
            // Never exactly 0 or 1: those frames are the clips' own.
            let progress = (k + 1) as f32 / (frames + 1) as f32;
            renderer.render(&params, width, height, &a, &b, progress, &mut frame).map_err(|e| MediaError::Failed(e.to_string()))?;
            sink.write_all(&frame)?;
        }
        Ok(())
    })();
    // Closing the encoder's input is what lets it finish.
    drop(encoder.child.stdin.take());
    match drawn {
        Ok(()) => {
            from.finish("FFmpeg could not read the scene for the transition.")?;
            to.finish("FFmpeg could not read the scene for the transition.")?;
            encoder.finish("FFmpeg could not encode the transition.")
        }
        Err(e) => {
            from.abandon();
            to.abandon();
            encoder.abandon();
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outgoing_side_holds_the_last_frame_of_its_time_on_the_timeline() {
        let clip = Path::new("scene.mp4");
        let (outgoing, incoming) = side_chains(&Source::Cut { outgoing: clip, outgoing_frames: 24, from: 18, incoming: clip, before: 6 }, 12, 24, "CANVAS");
        // A file longer than its 24 frames on the timeline is cut there before the last frame is held.
        assert!(outgoing.starts_with("fps=24,trim=end_frame=24,setpts=PTS-STARTPTS,tpad=stop_mode=clone"), "{outgoing}");
        assert!(outgoing.ends_with("trim=start_frame=18:end_frame=30,setpts=PTS-STARTPTS,CANVAS"), "{outgoing}");
        assert_eq!(incoming, "fps=24,trim=start_frame=0:end_frame=6,setpts=PTS-STARTPTS,tpad=start=6:start_mode=clone,CANVAS");
    }

    #[test]
    fn a_window_inside_a_clip_reads_the_same_frames_on_both_sides() {
        let (a, b) = side_chains(&Source::Inside { clip: Path::new("scene.mp4"), from: 12 }, 12, 24, "CANVAS");
        assert_eq!(a, "fps=24,trim=start_frame=12:end_frame=24,setpts=PTS-STARTPTS,CANVAS");
        assert_eq!(a, b);
    }
}
