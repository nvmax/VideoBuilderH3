//! Effects and filters on the stitched video.
//!
//! A look is a block of the timeline: it starts `start` seconds in, lasts `seconds`, and changes the picture of the frames it covers, on the clock of the scene
//! timing. A filter grades the colours, an effect adds or bends something (`h3_transitions::looks`). Blocks may overlap: on a frame every filter that covers
//! it is applied first, then every effect, each group in the order of the timeline. `intensity` (0 to 1) mixes a block's change with the picture it got.
//! The frame count never changes, so the song stays in step.
//!
//! The pass runs on the joined video, after the transitions and before the overlays. `plan` and `segments` are pure. `apply_looks` cuts the video where blocks
//! start and end, draws the frames that have a look on the GPU and leaves the rest as they are, and joins the pieces.
//!
//! Frames without a look are not encoded again where that can be avoided: a stretch of them is copied from the video as it is, from one group of pictures to
//! the next (`plain_parts`), and only the few frames between the stretch and the nearest looks are encoded. What is encoded uses the stitch's own x264
//! settings (`scene_video_args`), so it is as good as the scene clips and the same preset, which the pieces need to be joined without being decoded.

use std::path::{Path, PathBuf};
use std::process::Stdio;

pub use h3_transitions::Kind;
use h3_transitions::{look, params_from_json, Params, TransitionRenderer};
use serde_json::{json, Value};

use crate::tools::{args, scene_video_args, MediaError, Tools};
use crate::transition::{decoder, to_canvas, Piped, UNTAGGED_YUV};
use crate::video::{probe_stream_fields, video_size};

pub const DEFAULT_INTENSITY: f64 = 1.0;
pub const SECONDS_RANGE: (f64, f64) = (0.05, 86400.0);

/// One effect or filter as the caller asks for it.
#[derive(Debug, Clone)]
pub struct LookItem {
    pub kind: Kind,
    pub name: String,
    pub params: Value,
    /// Seconds from the start of the stitch's timeline (the clock of the scene timing).
    pub start: f64,
    pub seconds: f64,
    pub intensity: f64,
}

impl LookItem {
    /// `{"look": "Glow", "start": 12.5, "seconds": 2, "intensity": 0.8, "params": {}}`. Only `look` and `start` are required.
    pub fn from_json(kind: Kind, value: &Value) -> Result<Self, String> {
        let map = value.as_object().ok_or_else(|| format!("An {} must be an object.", kind.name()))?;
        let name = map.get("look").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| format!("An {} needs `look`, its name.", kind.name()))?;
        let entry = look(kind, name).map_err(|e| e.to_string())?;
        let start = map.get("start").and_then(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0).ok_or("It needs `start`, its time on the timeline in seconds.")?;
        let seconds = match map.get("seconds") {
            None | Some(Value::Null) => 1.0,
            Some(v) => v.as_f64().filter(|s| s.is_finite()).ok_or("`seconds` must be a number.")?,
        };
        let intensity = match map.get("intensity") {
            None | Some(Value::Null) => DEFAULT_INTENSITY,
            Some(v) => v.as_f64().filter(|s| s.is_finite()).ok_or("`intensity` must be a number from 0 to 1.")?,
        };
        let params = map.get("params").cloned().unwrap_or(Value::Null);
        params_from_json(entry, &params).map_err(|e| e.to_string())?;
        Ok(Self { kind, name: name.to_string(), params, start, seconds: seconds.clamp(SECONDS_RANGE.0, SECONDS_RANGE.1), intensity: intensity.clamp(0.0, 1.0) })
    }
}

/// The effects or filters of a stitch payload: absent or `null` means none.
pub fn items_from_json(kind: Kind, value: Option<&Value>) -> Result<Vec<LookItem>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => list.iter().enumerate().map(|(i, item)| LookItem::from_json(kind, item).map_err(|e| format!("{} {}: {e}", kind.name(), i + 1))).collect(),
        Some(_) => Err(format!("{}s must be a list.", kind.name())),
    }
}

/// What was drawn, for the reply of a stitch.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedLook {
    pub kind: Kind,
    pub name: String,
    /// Where it starts in the joined video, in seconds.
    pub start: f64,
    pub frames: i64,
}

impl AppliedLook {
    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind.name(), "look": self.name, "start": (self.start * 1000.0).round() / 1000.0, "frames": self.frames})
    }
}

/// A look's frames of the joined video: `[first, last)`, and which item it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub item: usize,
    pub first: i64,
    pub last: i64,
}

/// The blocks of the items that cover at least a frame of the video (`total` frames), in the order they are applied: filters, then effects, each by start. A
/// note for every item that was shortened to the end of the video or left out.
pub fn plan(items: &[LookItem], total: i64, fps: u32) -> (Vec<Block>, Vec<String>) {
    let round = |t: f64| (t.max(0.0) * f64::from(fps) + 0.5) as i64;
    let mut blocks = Vec::new();
    let mut notes = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let first = round(item.start);
        let wanted_last = round(item.start + item.seconds).max(first + 1);
        let last = wanted_last.min(total);
        if first >= total || last <= first {
            notes.push(format!("The {} {} at {:.2} s was left out: it starts after the end of the video.", item.kind.name(), item.name, item.start));
            continue;
        }
        if last < wanted_last {
            notes.push(format!("The {} {} at {:.2} s was shortened to the end of the video.", item.kind.name(), item.name, item.start));
        }
        blocks.push(Block { item: index, first, last });
    }
    let order = |b: &Block| (items[b.item].kind != Kind::Filter, b.first, b.item);
    blocks.sort_by_key(order);
    (blocks, notes)
}

/// A stretch of the video with the same blocks on every frame (`blocks` are indexes into the plan; none means the frames stay as they are).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub from: i64,
    pub to: i64,
    pub blocks: Vec<usize>,
}

/// The video cut at every place a block starts or ends, with the blocks that cover each piece in the order they are applied. Neighbours without a look are
/// one piece.
pub fn segments(blocks: &[Block], total: i64) -> Vec<Segment> {
    let mut cuts: Vec<i64> = vec![0, total];
    for b in blocks {
        cuts.push(b.first);
        cuts.push(b.last);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<Segment> = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let active: Vec<usize> = blocks.iter().enumerate().filter(|(_, b)| b.first <= from && to <= b.last).map(|(i, _)| i).collect();
        match out.last_mut() {
            Some(last) if last.blocks.is_empty() && active.is_empty() => last.to = to,
            _ => out.push(Segment { from, to, blocks: active }),
        }
    }
    out
}

/// A block's progress and time on frame `frame`: `progress` runs over (0, 1) across the block, `time` is the seconds since it started.
pub fn block_state(block: &Block, frame: i64, fps: u32) -> (f32, f32) {
    let progress = (frame - block.first) as f32 + 0.5;
    (progress / (block.last - block.first) as f32, (frame - block.first) as f32 / fps as f32)
}

/// A stretch of the video, frames `[from, to)`, written as it is (`copy`) or encoded again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub from: i64,
    pub to: i64,
    pub copy: bool,
}

/// How a stretch of frames without a look is written. `keyframes` are the frames a group of pictures starts at, in order, and `total` the frames of the
/// video. A stretch can be copied from one keyframe to the next, or to the end of the video, but not cut inside a group of pictures. So the part from the
/// first keyframe in the stretch to the last keyframe at or before its end is copied when it is at least `min_copy` frames long, and the frames on either
/// side of it are encoded again. A stretch with no such part is encoded whole.
pub fn plain_parts(from: i64, to: i64, total: i64, keyframes: &[i64], min_copy: i64) -> Vec<Part> {
    let start = keyframes.iter().copied().find(|k| *k >= from && *k < to);
    let end = start.and_then(|a| keyframes.iter().copied().chain(std::iter::once(total)).filter(|b| *b > a && *b <= to).max());
    match (start, end) {
        (Some(a), Some(b)) if b - a >= min_copy => {
            let mut parts = Vec::new();
            if from < a {
                parts.push(Part { from, to: a, copy: false });
            }
            parts.push(Part { from: a, to: b, copy: true });
            if b < to {
                parts.push(Part { from: b, to, copy: false });
            }
            parts
        }
        _ => vec![Part { from, to, copy: false }],
    }
}

/// The frames of `video` that a group of pictures starts at, as `(frame, seconds)` in order. `None` when the video does not have one packet for each of the
/// `total` frames, because then a packet's place in the video is not its frame number and nothing can be copied safely.
fn keyframes(tools: &Tools, video: &Path, total: i64) -> Option<Vec<(i64, f64)>> {
    let out = tools
        .ffprobe_checked(
            &args(&["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0", &video.display().to_string()]),
            "ffprobe could not list the packets of the video.",
        )
        .ok()?;
    let mut packets: Vec<(f64, bool)> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.trim().is_empty()) {
        let (time, flags) = line.trim().split_once(',')?;
        packets.push((time.parse().ok()?, flags.contains('K')));
    }
    if packets.len() as i64 != total {
        return None;
    }
    // Packets come in the order they are decoded, frames in the order they are shown.
    packets.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(packets.iter().enumerate().filter(|(_, packet)| packet.1).map(|(frame, packet)| (frame as i64, packet.0)).collect())
}

/// Copies `count` frames of `input` into `output` without encoding them, starting at the keyframe that is shown at `seconds`. The copy is checked: when it
/// does not hold exactly `count` frames of the right length the file is removed and an error comes back, and the caller encodes those frames instead.
fn copy_piece(tools: &Tools, input: &Path, seconds: f64, count: i64, fps: u32, output: &Path) -> Result<(), MediaError> {
    // The time ffprobe gave is rounded to the microsecond, and a seek lands on the keyframe at or before it. One microsecond more is past the keyframe
    // whichever way it was rounded and far short of the next frame. Seeking further would shift the copy's timestamps by the difference.
    let start = format!("{:.6}", seconds + 0.000_001);
    let wrong = || MediaError::Failed("FFmpeg did not copy the frames of the video as they were.".into());
    tools.ffmpeg_checked(
        &args(&["-y", "-v", "error", "-ss", &start, "-i", &input.display().to_string(), "-map", "0:v:0", "-an", "-c:v", "copy", "-frames:v", &count.to_string(), &output.display().to_string()]),
        "FFmpeg could not copy a piece of the video for the effects.",
    )?;
    let checked = probe_stream_fields(tools, output, "v:0", &["nb_read_packets", "duration"], true).ok().filter(|f| {
        let frames = f.get("nb_read_packets").and_then(|v| v.parse::<i64>().ok());
        let seconds = f.get("duration").and_then(|v| v.parse::<f64>().ok());
        frames == Some(count) && seconds.is_some_and(|s| (s - count as f64 / f64::from(fps)).abs() < 1.5 / f64::from(fps))
    });
    if checked.is_none() {
        let _ = std::fs::remove_file(output);
        return Err(wrong());
    }
    Ok(())
}

/// Cuts frames `[from, to)` of `input` into `output` and encodes them as they are.
fn trim_piece(tools: &Tools, input: &Path, from: i64, to: i64, fps: u32, render_crf: Option<f64>, output: &Path) -> Result<(), MediaError> {
    let filter = format!("fps={fps},trim=start_frame={from}:end_frame={to},setpts=PTS-STARTPTS");
    let mut a = args(&["-y", "-v", "error", "-i", &input.display().to_string(), "-map", "0:v:0", "-an", "-vf", &filter, "-frames:v", &(to - from).to_string()]);
    a.extend(scene_video_args(render_crf));
    a.extend(args(&["-r", &fps.to_string(), &output.display().to_string()]));
    tools.ffmpeg_checked(&a, "FFmpeg could not cut a piece of the video for the effects.")?;
    Ok(())
}

/// Draws frames `[from, to)` of `input` with the blocks `active` applied to each, and encodes them to `output`.
#[allow(clippy::too_many_arguments)]
fn draw_piece(
    tools: &Tools,
    renderer: &mut TransitionRenderer,
    input: &Path,
    segment: &Segment,
    blocks: &[Block],
    items: &[LookItem],
    params: &[Params],
    fps: u32,
    render_crf: Option<f64>,
    output: &Path,
) -> Result<(), MediaError> {
    let (width, height) = video_size(tools, input)?;
    let size = width as usize * height as usize * 4;
    let frames = (segment.to - segment.from) as u32;
    let chain = format!("fps={fps},trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS,{}", segment.from, segment.to, to_canvas(width, height));
    let mut reader = decoder(tools, input, &chain, frames)?;
    let mut encode = args(&["-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{width}x{height}"), "-r", &fps.to_string(), "-i", "pipe:0", "-an", "-vf", UNTAGGED_YUV]);
    encode.extend(scene_video_args(render_crf));
    encode.extend(args(&["-r", &fps.to_string(), &output.display().to_string()]));
    let mut writer = match Piped::spawn(&tools.ffmpeg, &encode, Stdio::piped(), Stdio::null()) {
        Ok(p) => p,
        Err(e) => {
            reader.abandon();
            return Err(e);
        }
    };
    let drawn = (|| -> Result<(), MediaError> {
        let mut pipe = reader.child.stdout.take();
        let mut sink = writer.child.stdin.take();
        let (Some(pipe), Some(sink)) = (pipe.as_mut(), sink.as_mut()) else { return Err(MediaError::Failed("FFmpeg did not open its pipes.".into())) };
        let (mut current, mut next) = (vec![0u8; size], Vec::with_capacity(size));
        for frame in segment.from..segment.to {
            std::io::Read::read_exact(pipe, &mut current).map_err(|_| MediaError::Failed("FFmpeg returned too few frames of the video for the effects.".into()))?;
            for &b in &segment.blocks {
                let block = &blocks[b];
                let (progress, time) = block_state(block, frame, fps);
                renderer.render_look(&params[block.item], width, height, &current, progress, items[block.item].intensity as f32, time, &mut next).map_err(|e| MediaError::Failed(e.to_string()))?;
                std::mem::swap(&mut current, &mut next);
            }
            std::io::Write::write_all(sink, &current)?;
        }
        Ok(())
    })();
    drop(writer.child.stdin.take());
    match drawn {
        Ok(()) => {
            reader.finish("FFmpeg could not read the video for the effects.")?;
            writer.finish("FFmpeg could not encode the video with the effects.")
        }
        Err(e) => {
            reader.abandon();
            writer.abandon();
            Err(e)
        }
    }
}

/// Applies the looks to `input` and writes the result to `output`. Returns what was drawn and a note for each item that was cut short or left out. When no
/// item covers a frame nothing is written (`None` for the applied list) and the caller keeps `input`. `render_crf` is the quality the scene clips were
/// rendered at (`source_crf`), which what is encoded here is never coarser than.
#[allow(clippy::too_many_arguments)]
pub fn apply_looks(
    tools: &Tools,
    items: &[LookItem],
    input: &Path,
    output: &Path,
    fps: u32,
    total: i64,
    scratch: &Path,
    render_crf: Option<f64>,
) -> Result<(Option<Vec<AppliedLook>>, Vec<String>), MediaError> {
    let (blocks, notes) = plan(items, total, fps);
    if blocks.is_empty() {
        return Ok((None, notes));
    }
    let mut params: Vec<Params> = Vec::with_capacity(items.len());
    for item in items {
        let entry = look(item.kind, &item.name).map_err(|e| MediaError::Failed(e.to_string()))?;
        params.push(params_from_json(entry, &item.params).map_err(|e| MediaError::Failed(e.to_string()))?);
    }
    let mut renderer = TransitionRenderer::new().map_err(|e| MediaError::Failed(e.to_string()))?;
    let mut pieces: Vec<PathBuf> = Vec::new();
    // Where a group of pictures starts, so the frames between the looks can be copied.
    let keys = keyframes(tools, input, total).unwrap_or_default();
    let key_frames: Vec<i64> = keys.iter().map(|(frame, _)| *frame).collect();
    let result = (|| -> Result<(), MediaError> {
        for segment in segments(&blocks, total).iter() {
            if segment.blocks.is_empty() {
                for part in plain_parts(segment.from, segment.to, total, &key_frames, i64::from(fps)) {
                    let path = scratch.join(format!("_temp_look_piece_{:04}.mp4", pieces.len() + 1));
                    pieces.push(path.clone());
                    let seconds = keys.iter().find(|(frame, _)| *frame == part.from).map(|(_, seconds)| *seconds);
                    // A copy that does not check out is encoded instead: the frames matter more than the speed.
                    let copied = part.copy && seconds.is_some_and(|s| copy_piece(tools, input, s, part.to - part.from, fps, &path).is_ok());
                    if !copied {
                        trim_piece(tools, input, part.from, part.to, fps, render_crf, &path)?;
                    }
                }
            } else {
                let path = scratch.join(format!("_temp_look_piece_{:04}.mp4", pieces.len() + 1));
                pieces.push(path.clone());
                draw_piece(tools, &mut renderer, input, segment, &blocks, items, &params, fps, render_crf, &path)?;
            }
        }
        let list = scratch.join("_temp_look_pieces.txt");
        crate::stitch::write_concat_list(&list, &pieces)?;
        let joined = tools.ffmpeg_checked(&args(&["-y", "-v", "error", "-f", "concat", "-safe", "0", "-i", &list.display().to_string(), "-an", "-c:v", "copy", &output.display().to_string()]), "FFmpeg could not join the pieces of the video.");
        crate::stitch::remove_quietly(&list);
        joined.map(|_| ())
    })();
    for piece in &pieces {
        crate::stitch::remove_quietly(piece);
    }
    result?;
    let applied = blocks.iter().map(|b| AppliedLook { kind: items[b.item].kind, name: items[b.item].name.clone(), start: b.first as f64 / f64::from(fps), frames: b.last - b.first }).collect();
    Ok((Some(applied), notes))
}
