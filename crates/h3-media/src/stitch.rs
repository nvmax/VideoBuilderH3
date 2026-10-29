//! Join scene clips into the final video. Port of `_stitch_scene_videos` and its embedded audio helpers in
//! `runner/video_files.py`.
//!
//! With timing items every clip is cut or padded to exactly `round(end * fps) - round(start * fps)` frames, so the
//! joined video has the timeline's frames. The audio is either the project song, the scenes' own audio, or (for
//! built-in audio) the scenes' audio joined into one track that lines up with the video at every scene start.

use std::fs;
use std::path::{Path, PathBuf};

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

use h3_transitions::TransitionRenderer;

use crate::tools::{args, run, MediaError, Tools};
use crate::looks::{apply_looks, AppliedLook, LookItem};
use crate::transition::{clip_at, clip_bounds, clip_frames, pieces, plan, render_window, AppliedTransition, Piece, Source, TransitionItem};
use crate::video::{clip_frame_length, is_video_file, probe_output_media, probe_stream_fields, video_size, Ratio};

/// Sample rate of the joined embedded audio track.
pub const EMBEDDED_SAMPLE_RATE: u32 = 48_000;
const CROSSFADE_SECONDS: f64 = 0.008;
const SONG_SEARCH_SECONDS: f64 = 0.025;
const SONG_MATCH: f64 = 0.8;
const SILENCE_LEVEL: f32 = 1e-4;
const PADDING_SAMPLES: usize = 2048;

/// Audio of one scene.
#[derive(Debug, Clone)]
pub struct SceneAudioItem {
    pub path: PathBuf,
    pub start: f64,
    pub duration: f64,
    /// The clip's own audio track.
    pub embedded: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct TimingItem {
    pub start: f64,
    pub end: f64,
}

/// A clip laid over the joined video between `start` and `end`.
#[derive(Debug, Clone)]
pub struct OverlayItem {
    pub path: PathBuf,
    pub start: f64,
    pub end: f64,
    pub source_start: f64,
}

#[derive(Debug, Clone, Default)]
pub struct StitchRequest {
    pub project: PathBuf,
    pub scene_paths: Vec<PathBuf>,
    pub audio_path: Option<PathBuf>,
    pub song_path: Option<PathBuf>,
    pub scene_audio_items: Vec<SceneAudioItem>,
    pub scene_timing: Vec<TimingItem>,
    pub timeline_fps: u32,
    pub use_embedded_scene_audio: bool,
    pub overlays: Vec<OverlayItem>,
    /// Transitions on the cuts between scenes. They need `scene_timing`: without it the clips are joined as they are.
    pub transitions: Vec<TransitionItem>,
    /// The filters and the effects on the finished video, applied after the transitions (filters first).
    pub looks: Vec<LookItem>,
    /// Output canvas. Zero keeps the clips' size.
    pub width: u32,
    pub height: u32,
    pub audio_start: f64,
    pub audio_duration: f64,
    pub output_prefix: String,
}

#[derive(Debug, Clone)]
pub struct StitchResult {
    pub final_video_path: PathBuf,
    pub video_folder: PathBuf,
    pub scene_count: usize,
    pub insert_count: usize,
    pub used_scene_audio: bool,
    pub embedded_audio_mode: String,
    pub normalized_canvas: bool,
    pub timeline_frame_sync: bool,
    pub timeline_frame_count: i64,
    pub output_width: u32,
    pub output_height: u32,
    pub output_duration: f64,
    pub output_audio_duration: f64,
    /// The transitions that were drawn, after being fitted to the scenes next to them.
    pub transitions: Vec<AppliedTransition>,
    /// Why a transition was shortened or left out.
    pub transition_notes: Vec<String>,
    /// The filters and effects that were drawn, in the order they were applied.
    pub looks: Vec<AppliedLook>,
    /// Why a filter or an effect was shortened or left out.
    pub look_notes: Vec<String>,
}

type Frames = Vec<[f32; 2]>;

// -- helpers ----------------------------------------------------------------------------------------------------------------

fn s(path: &Path) -> String {
    path.display().to_string()
}

/// `file '<path>'` escaping for ffmpeg's concat list.
fn concat_path(path: &Path) -> String {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string().replace('\\', "/").replace('\'', "'\\''")
}

pub(crate) fn write_concat_list(list: &Path, paths: &[PathBuf]) -> Result<(), MediaError> {
    let mut text = String::new();
    for path in paths {
        text.push_str(&format!("file '{}'\n", concat_path(path)));
    }
    fs::write(list, text)?;
    Ok(())
}

pub(crate) fn remove_quietly(path: &Path) {
    let _ = fs::remove_file(path);
}

/// `FINAL_VIDEO.mp4`, or `FINAL_VIDEO_2.mp4`, `_3`, ... in the project folder.
pub fn unique_final_video_path(project: &Path, prefix: &str) -> PathBuf {
    let safe: String = prefix.chars().map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let safe = safe.trim_matches('_');
    let safe = if safe.is_empty() { "FINAL_VIDEO" } else { safe };
    let first = project.join(format!("{safe}.mp4"));
    if !first.exists() {
        return first;
    }
    let mut index = 2;
    loop {
        let candidate = project.join(format!("{safe}_{index}.mp4"));
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}

/// Scale and crop a clip to the canvas. Returns false when it already has that size.
fn normalize_canvas(tools: &Tools, source: &Path, target: &Path, width: u32, height: u32) -> Result<bool, MediaError> {
    if width == 0 || height == 0 {
        return Ok(false);
    }
    if let Ok((w, h)) = video_size(tools, source) {
        if w == width && h == height {
            return Ok(false);
        }
    }
    let vf = format!("scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1");
    tools.ffmpeg_checked(&args(&["-y", "-i", &s(source), "-an", "-vf", &vf, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", &s(target)]), "FFmpeg could not normalize the canvas.")?;
    Ok(true)
}

// -- embedded audio ---------------------------------------------------------------------------------------------------------

/// Decode a file's audio to 48 kHz float stereo, with optional extra filters.
pub fn decode_audio_samples(tools: &Tools, path: &Path, filters: &[String], stream: &str) -> Result<Frames, MediaError> {
    let mut chain = vec![format!("aresample={EMBEDDED_SAMPLE_RATE}"), "aformat=sample_fmts=flt:channel_layouts=stereo".to_string()];
    chain.extend(filters.iter().cloned());
    let a = args(&["-v", "error", "-i", &s(path), "-map", stream, "-vn", "-af", &chain.join(","), "-f", "f32le", "-acodec", "pcm_f32le", "-"]);
    let out = run(&tools.ffmpeg, &a)?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(MediaError::Failed(if text.is_empty() { format!("FFmpeg could not decode audio: {}", path.display()) } else { text }));
    }
    let (words, _) = out.stdout.as_chunks::<4>();
    let floats: Vec<f32> = words.iter().map(|b| f32::from_le_bytes(*b)).collect();
    let (pairs, _) = floats.as_chunks::<2>();
    Ok(pairs.to_vec())
}

fn probe_float(values: &std::collections::HashMap<String, String>, key: &str) -> f64 {
    values.get(key).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// A clip's audio placed against its first frame, or `None` when the clip has no audio stream.
fn clip_audio_samples(tools: &Tools, path: &Path) -> Result<Option<Frames>, MediaError> {
    let audio = probe_stream_fields(tools, path, "a:0", &["index", "start_time"], false)?;
    if audio.get("index").is_none_or(|v| v.is_empty()) {
        return Ok(None);
    }
    // Keep the audio where it sits against the clip's first frame (normally both start at 0). No input -ss: a seek, even
    // to 0, moves AAC audio by its priming.
    let video = probe_stream_fields(tools, path, "v:0", &["start_time"], false)?;
    let lead = probe_float(&audio, "start_time") - probe_float(&video, "start_time");
    let mut filters = Vec::new();
    if lead > 0.0005 {
        filters.push(format!("adelay={:.3}:all=1", lead * 1000.0));
    } else if lead < -0.0005 {
        filters.push(format!("atrim=start={:.6},asetpts=PTS-STARTPTS", -lead));
    }
    let mut samples = decode_audio_samples(tools, path, &filters, "0:a:0")?;
    // An AAC stream can decode with up to one frame of encoder padding (near digital silence) past the real audio. Drop
    // such a short silent tail so it is filled like any other shortfall. Longer silence is content.
    let end = samples.iter().rposition(|f| f[0].abs().max(f[1].abs()) >= SILENCE_LEVEL).map_or(0, |i| i + 1);
    if samples.len() - end <= PADDING_SAMPLES {
        samples.truncate(end);
    }
    Ok(Some(samples))
}

/// `samples[start .. start + count]` with zeros wherever that range falls outside `samples`.
fn slice_padded(samples: Option<&Frames>, start: i64, count: i64) -> Frames {
    let count = count.max(0) as usize;
    let mut out = vec![[0f32; 2]; count];
    let Some(samples) = samples else { return out };
    let lo = start.max(0);
    let hi = (samples.len() as i64).min(start + count as i64);
    if hi > lo {
        for i in lo..hi {
            out[(i - start) as usize] = samples[i as usize];
        }
    }
    out
}

/// True when `clip` is the project song from `song_start` (a sample index), give or take a few milliseconds.
fn matches_song(clip: &Frames, song: &Frames, song_start: i64) -> bool {
    let sr = i64::from(EMBEDDED_SAMPLE_RATE);
    let mono: Vec<f32> = clip.iter().map(|f| (f[0] + f[1]) / 2.0).collect();
    let lo = (0.1 * sr as f64) as i64;
    let hi = (mono.len() as i64 - (0.05 * sr as f64) as i64).min(lo + 2 * sr);
    if hi - lo < (0.25 * sr as f64) as i64 {
        return false;
    }
    let block: Vec<f64> = mono[lo as usize..hi as usize].iter().map(|v| f64::from(*v)).collect();
    if block.iter().all(|v| *v == 0.0) {
        return false;
    }
    let search = (SONG_SEARCH_SECONDS * sr as f64) as i64;
    let seg_lo = song_start + lo - search;
    let seg: Vec<f64> = slice_padded(Some(song), seg_lo, block.len() as i64 + 2 * search).iter().map(|f| f64::from((f[0] + f[1]) / 2.0)).collect();
    let size = (seg.len() + block.len()).next_power_of_two();
    let mut planner = FftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(size);
    let inverse = planner.plan_fft_inverse(size);
    let to_spectrum = |data: &[f64]| -> Vec<Complex<f64>> {
        let mut buffer: Vec<Complex<f64>> = data.iter().map(|v| Complex::new(*v, 0.0)).collect();
        buffer.resize(size, Complex::new(0.0, 0.0));
        forward.process(&mut buffer);
        buffer
    };
    let (seg_f, block_f) = (to_spectrum(&seg), to_spectrum(&block));
    let mut product: Vec<Complex<f64>> = seg_f.iter().zip(&block_f).map(|(a, b)| a * b.conj()).collect();
    inverse.process(&mut product);
    let lags = seg.len() - block.len() + 1;
    let mut energy = vec![0f64; seg.len() + 1];
    for (i, v) in seg.iter().enumerate() {
        energy[i + 1] = energy[i] + v * v;
    }
    let block_norm = block.iter().map(|v| v * v).sum::<f64>().sqrt();
    let best = (0..lags)
        .map(|k| {
            let corr = product[k].re / size as f64;
            let window = (energy[k + block.len()] - energy[k]).max(0.0);
            corr / (window.sqrt() * block_norm + 1e-12)
        })
        .fold(f64::NEG_INFINITY, f64::max);
    best >= SONG_MATCH
}

fn write_pcm_wav(path: &Path, samples: &Frames) -> Result<(), MediaError> {
    let data_len = samples.len() as u32 * 4;
    let mut out: Vec<u8> = Vec::with_capacity(44 + data_len as usize);
    out.extend(b"RIFF");
    out.extend((36 + data_len).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(EMBEDDED_SAMPLE_RATE.to_le_bytes());
    out.extend((EMBEDDED_SAMPLE_RATE * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data_len.to_le_bytes());
    for frame in samples {
        for v in frame {
            let q = (v.clamp(-1.0, 1.0) * 32767.0).round_ties_even() as i16;
            out.extend(q.to_le_bytes());
        }
    }
    fs::write(path, out)?;
    Ok(())
}

/// Join the scenes' own audio as one PCM track that lines up with the joined video at every scene start. Returns the WAV
/// and the mode used. Each scene gets exactly its clip's `frames / fps` of audio (sample counts come from the running
/// total, so 44.1 kHz rounding cannot build up).
///
/// * `continuous_song`: the scenes sit back to back in the song and every clip's audio is the song at its scene's song
///   time. The track is one continuous read of the song from `song_origin`. Joining the clips' audio would jump or
///   repeat the music by up to half a frame at every cut, one read cannot.
/// * `per_scene`: each scene plays its own audio cut to its length. Audio shorter than the picture is continued from the
///   song instead of silence, a clip with no audio is the song for its length, and each join gets a short crossfade.
#[allow(clippy::too_many_arguments)]
pub fn embedded_scene_audio_track(
    tools: &Tools,
    scene_paths: &[PathBuf],
    frame_lengths: &[(i64, Ratio)],
    target_dir: &Path,
    temp_files: &mut Vec<PathBuf>,
    song_path: Option<&Path>,
    song_starts: &[f64],
    song_origin: Option<f64>,
) -> Result<(PathBuf, String), MediaError> {
    let sr = i128::from(EMBEDDED_SAMPLE_RATE);
    // Frame exact boundaries as sample counts, from exact fractions.
    let (mut num, mut den): (i128, i128) = (0, 1);
    let mut bounds: Vec<i64> = vec![0];
    for (frames, fps) in frame_lengths {
        // elapsed += frames / fps  (fps = n/d)  =>  frames * d / n
        let (add_n, add_d) = (i128::from(*frames) * fps.den, fps.num);
        num = num * add_d + add_n * den;
        den *= add_d;
        let g = gcd_i128(num.abs(), den.abs()).max(1);
        num /= g;
        den /= g;
        let rounded = ((2 * num * sr + den) / (2 * den)) as i64;
        let last = *bounds.last().unwrap_or(&0);
        bounds.push((last + 1).max(rounded));
    }
    let total = *bounds.last().unwrap_or(&0);

    let song: Option<Frames> = match song_path.filter(|p| p.is_file()) {
        Some(p) => Some(decode_audio_samples(tools, p, &[], "0:a:0")?),
        None => None,
    };
    let mut clips: Vec<Option<Frames>> = Vec::new();
    for path in scene_paths {
        clips.push(clip_audio_samples(tools, path)?);
    }
    let starts: Vec<i64> = song_starts.iter().map(|v| h3_core::pyval::round_half_even(v * sr as f64)).collect();
    let song = if song.is_some() && starts.len() != scene_paths.len() { None } else { song };

    let mut mode = "per_scene".to_string();
    let all_match = match (&song, song_origin) {
        (Some(song), Some(_)) => clips.iter().zip(&starts).all(|(clip, start)| clip.as_ref().is_none_or(|c| matches_song(c, song, *start))),
        _ => false,
    };
    let track: Frames = if all_match {
        mode = "continuous_song".to_string();
        let origin = h3_core::pyval::round_half_even(song_origin.unwrap_or(0.0) * sr as f64);
        slice_padded(song.as_ref(), origin, total)
    } else {
        let fade = (h3_core::pyval::round_half_even(CROSSFADE_SECONDS * sr as f64)).max(1);
        let ramp: Vec<f32> = (0..fade).map(|i| (i as f32 + 0.5) / fade as f32).collect();
        let mut track = vec![[0f32; 2]; total as usize];
        let mut tails: Vec<Frames> = Vec::new();
        for (index, clip) in clips.iter().enumerate() {
            let count = bounds[index + 1] - bounds[index];
            let start = if song.is_some() { starts[index] } else { 0 };
            let part: Frames = match clip {
                None => match &song {
                    Some(_) => slice_padded(song.as_ref(), start, count + fade),
                    None => vec![[0f32; 2]; (count + fade) as usize],
                },
                Some(clip) => {
                    let mut part = slice_padded(Some(clip), 0, count + fade);
                    let have = clip.len() as i64;
                    if have < count + fade && song.is_some() {
                        // Continue the music from the song where the clip's audio runs out, with a short blend.
                        let blend = fade.min(have);
                        let rest = slice_padded(song.as_ref(), start + have, count + fade - have);
                        for (i, f) in rest.into_iter().enumerate() {
                            part[have as usize + i] = f;
                        }
                        if blend > 0 {
                            let lo = have - blend;
                            let song_part = slice_padded(song.as_ref(), start + lo, blend);
                            for i in 0..blend as usize {
                                let w = (i as f32 + 0.5) / blend as f32;
                                let c = clip[lo as usize + i];
                                part[lo as usize + i] = [c[0] * (1.0 - w) + song_part[i][0] * w, c[1] * (1.0 - w) + song_part[i][1] * w];
                            }
                        }
                    }
                    part
                }
            };
            let count_usize = count as usize;
            track[bounds[index] as usize..bounds[index + 1] as usize].copy_from_slice(&part[..count_usize]);
            tails.push(part[count_usize..].to_vec());
        }
        for index in 1..clips.len() {
            let tail = &tails[index - 1];
            if tail.iter().map(|f| f[0].abs().max(f[1].abs())).fold(0f32, f32::max) < 1e-3 {
                continue; // nothing was playing on: keep the next scene's first sample exact
            }
            let cut = bounds[index] as usize;
            let width = (fade.min(bounds[index + 1] - bounds[index])) as usize;
            for i in 0..width.min(tail.len()) {
                let w = ramp[i];
                for ch in 0..2 {
                    track[cut + i][ch] = track[cut + i][ch] * w + tail[i][ch] * (1.0 - w);
                }
            }
        }
        track
    };

    let joined = target_dir.join("_temp_scene_audio_joined.wav");
    temp_files.push(joined.clone());
    write_pcm_wav(&joined, &track)?;
    Ok((joined, mode))
}

fn gcd_i128(a: i128, b: i128) -> i128 {
    if b == 0 { a } else { gcd_i128(b, a % b) }
}

// -- the stitch ---------------------------------------------------------------------------------------------------------------

/// Join the scene clips and the audio into one file in the project folder.
pub fn stitch_scene_videos(tools: &Tools, req: &StitchRequest) -> Result<StitchResult, MediaError> {
    if req.scene_paths.is_empty() {
        return Err(MediaError::Failed("No scene video paths were provided.".into()));
    }
    let target_dir = req.project.join("rendered_scene_videos");
    fs::create_dir_all(&target_dir)?;
    for (i, path) in req.scene_paths.iter().enumerate() {
        if !path.is_file() {
            return Err(MediaError::Failed(format!("Scene {} video was not found: {}", i + 1, path.display())));
        }
        if !is_video_file(path) {
            return Err(MediaError::Failed(format!("Scene {} media is not a supported video file: {}", i + 1, path.display())));
        }
    }
    let mut audio_items: Vec<SceneAudioItem> = req.scene_audio_items.clone();
    let audio_path = req.audio_path.clone().filter(|p| p.is_file());
    if !audio_items.is_empty() {
        if audio_items.len() != req.scene_paths.len() {
            return Err(MediaError::Failed("Scene audio item count does not match scene video count.".into()));
        }
        for (i, item) in audio_items.iter().enumerate() {
            if !item.embedded && !item.path.is_file() {
                return Err(MediaError::Failed(format!("Scene {} audio was not found: {}", i + 1, item.path.display())));
            }
        }
    } else if req.use_embedded_scene_audio {
        audio_items = req.scene_paths.iter().map(|p| SceneAudioItem { path: p.clone(), start: 0.0, duration: 0.0, embedded: true }).collect();
    } else if audio_path.is_none() {
        return Err(MediaError::Failed(format!("Audio file was not found: {}", req.audio_path.as_ref().map(|p| s(p)).unwrap_or_default())));
    }
    // The project song fills scene audio that runs short and plays continuously when the clips' audio is the song.
    let song_path: Option<PathBuf> = req.song_path.clone().filter(|p| p.is_file()).or_else(|| audio_path.clone());

    let mut timeline_sync: Vec<PathBuf> = Vec::new();
    let mut timeline_frames: i64 = 0;
    let mut timeline_lengths: Vec<(i64, Ratio)> = Vec::new();
    let mut windows: Vec<(f64, f64)> = Vec::new();
    let mut transition_files: Vec<PathBuf> = Vec::new();
    let mut applied_transitions: Vec<AppliedTransition> = Vec::new();
    let mut transition_notes: Vec<String> = Vec::new();
    let mut applied_looks: Vec<AppliedLook> = Vec::new();
    let mut look_notes: Vec<String> = Vec::new();
    let mut concat_paths: Vec<PathBuf> = req.scene_paths.clone();
    if !req.scene_timing.is_empty() {
        if req.timeline_fps == 0 {
            return Err(MediaError::Failed("Timeline FPS is required when scene timing items are provided.".into()));
        }
        if req.scene_timing.len() != req.scene_paths.len() {
            return Err(MediaError::Failed("Scene timing item count does not match scene video count.".into()));
        }
        let fps = req.timeline_fps;
        concat_paths = Vec::new();
        let clips: Vec<(f64, f64)> = req
            .scene_timing
            .iter()
            .map(|item| {
                let start = item.start.max(0.0);
                (start, item.end.max(start))
            })
            .collect();
        let frame_counts: Vec<i64> = clips.iter().map(|(start, end)| clip_frames(*start, *end, fps)).collect();
        let bounds = clip_bounds(&frame_counts);
        let (drawn, notes) = plan(&req.transitions, &clips, fps);
        transition_notes = notes;
        timeline_frames = bounds[bounds.len() - 1];
        timeline_lengths = frame_counts.iter().map(|frames| (*frames, Ratio::new(i128::from(fps), 1))).collect();
        windows = clips.clone();
        let mut renderer: Option<TransitionRenderer> = None;
        // The frames a transition draws are left out of the clips: each clip is cut into the pieces between the transitions.
        for (number, piece) in pieces(&bounds, &drawn).into_iter().enumerate() {
            match piece {
                Piece::Scene { clip, from, to } => {
                    let sync_path = target_dir.join(format!("_temp_timeline_scene_{:04}.mp4", number + 1));
                    let filter = format!("fps={fps},tpad=stop_mode=clone:stop_duration=1,trim=start_frame={from}:end_frame={to},setpts=PTS-STARTPTS");
                    let result = run(
                        &tools.ffmpeg,
                        &args(&["-y", "-i", &s(&req.scene_paths[clip]), "-map", "0:v:0", "-an", "-vf", &filter, "-frames:v", &(to - from).to_string(), "-r", &fps.to_string(), "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", &s(&sync_path)]),
                    )?;
                    if !result.status.success() || !sync_path.is_file() {
                        return Err(MediaError::Failed(crate::tools::failure_text(&result, &format!("FFmpeg failed to align scene {} to the timeline.", clip + 1))));
                    }
                    timeline_sync.push(sync_path.clone());
                    concat_paths.push(sync_path);
                }
                Piece::Transition(index) => {
                    let window = &drawn[index];
                    let source = match window.cut {
                        Some(k) => Source::Cut { outgoing: &req.scene_paths[k], outgoing_frames: frame_counts[k], from: window.first - bounds[k], incoming: &req.scene_paths[k + 1], before: (bounds[k + 1] - window.first) as u32 },
                        None => {
                            let clip = clip_at(&bounds, window.first);
                            Source::Inside { clip: &req.scene_paths[clip], from: window.first - bounds[clip] }
                        }
                    };
                    let cut_path = target_dir.join(format!("_temp_timeline_transition_{:04}.mp4", number + 1));
                    transition_files.push(cut_path.clone());
                    render_window(tools, &mut renderer, source, &window.name, &window.params, window.frames, fps, &cut_path)?;
                    concat_paths.push(cut_path);
                    applied_transitions.push(AppliedTransition { name: window.name.clone(), start: window.first as f64 / f64::from(fps), frames: window.frames, cut: window.cut });
                }
            }
        }
    } else if !req.transitions.is_empty() {
        transition_notes.push("Transitions need the timeline's scene timing and this stitch has none, so no transition was drawn.".to_string());
    }

    let concat_file = target_dir.join("concat_list.txt");
    write_concat_list(&concat_file, &concat_paths)?;
    let mut temp_video = target_dir.join("_temp_video_no_audio.mp4");
    let normalized_video = target_dir.join("_temp_video_normalized_canvas.mp4");
    let temp_audio = target_dir.join("_temp_scene_audio.m4a");
    let temp_global_audio = target_dir.join("_temp_global_audio.m4a");
    let audio_concat_file = target_dir.join("audio_concat_list.txt");
    let mut temp_audio_parts: Vec<PathBuf> = Vec::new();
    let final_output = unique_final_video_path(&req.project, if req.output_prefix.is_empty() { "FINAL_VIDEO" } else { &req.output_prefix });
    let mut normalized_canvas = false;

    let mut run_all = || -> Result<(String, bool), MediaError> {
        tools.ffmpeg_checked(&args(&["-y", "-f", "concat", "-safe", "0", "-i", &s(&concat_file), "-an", "-c:v", "copy", &s(&temp_video)]), "FFmpeg could not join the scene clips.")?;

        let mut overlays = req.overlays.clone();
        for (i, item) in overlays.iter().enumerate() {
            if !item.path.is_file() {
                return Err(MediaError::Failed(format!("Insert {} video was not found: {}", i + 1, item.path.display())));
            }
            if !is_video_file(&item.path) {
                return Err(MediaError::Failed(format!("Insert {} media is not a supported video file: {}", i + 1, item.path.display())));
            }
        }
        if !overlays.is_empty() {
            overlays.sort_by(|a, b| (a.start, a.end).partial_cmp(&(b.start, b.end)).unwrap_or(std::cmp::Ordering::Equal));
            let flattened = target_dir.join("_temp_video_with_inserts.mp4");
            let flatten_list = target_dir.join("flatten_concat_list.txt");
            let mut parts: Vec<PathBuf> = Vec::new();
            let mut cursor = 0.0f64;
            let mut part_index = 1;
            let mut add_part = |source: &Path, start: Option<f64>, duration: Option<f64>, parts: &mut Vec<PathBuf>| -> Result<(), MediaError> {
                let part = target_dir.join(format!("_temp_flatten_part_{part_index:04}.mp4"));
                part_index += 1;
                let mut a = args(&["-y"]);
                if let Some(start) = start {
                    a.extend(args(&["-ss", &format!("{:.6}", start.max(0.0))]));
                }
                a.extend(args(&["-i", &s(source)]));
                if let Some(duration) = duration {
                    a.extend(args(&["-t", &format!("{:.6}", duration.max(0.05))]));
                }
                a.extend(args(&["-an", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", &s(&part)]));
                tools.ffmpeg_checked(&a, "FFmpeg could not cut an insert part.")?;
                parts.push(part);
                Ok(())
            };
            for item in &overlays {
                if item.start > cursor + 0.01 {
                    add_part(&temp_video, Some(cursor), Some(item.start - cursor), &mut parts)?;
                }
                add_part(&item.path, Some(item.source_start), Some(item.end - item.start), &mut parts)?;
                cursor = cursor.max(item.end);
            }
            add_part(&temp_video, Some(cursor), None, &mut parts)?;
            write_concat_list(&flatten_list, &parts)?;
            tools.ffmpeg_checked(&args(&["-y", "-f", "concat", "-safe", "0", "-i", &s(&flatten_list), "-an", "-c:v", "copy", &s(&flattened)]), "FFmpeg could not join the inserts.")?;
            remove_quietly(&temp_video);
            remove_quietly(&flatten_list);
            for p in &parts {
                remove_quietly(p);
            }
            temp_video = flattened;
        }

        // The filters and effects go on the joined video, after the transitions are in it and before the canvas is normalized and the audio is put on.
        if !req.looks.is_empty() {
            if req.timeline_fps == 0 || timeline_frames <= 0 {
                look_notes.push("Effects and filters need the timeline's scene timing and this stitch has none, so none was drawn.".to_string());
            } else {
                let styled = target_dir.join("_temp_video_with_looks.mp4");
                let (applied, notes) = apply_looks(tools, &req.looks, &temp_video, &styled, req.timeline_fps, timeline_frames, &target_dir)?;
                look_notes.extend(notes);
                if let Some(applied) = applied {
                    applied_looks = applied;
                    remove_quietly(&temp_video);
                    temp_video = styled;
                }
            }
        }

        if req.width > 0 && req.height > 0 {
            normalized_canvas = normalize_canvas(tools, &temp_video, &normalized_video, req.width, req.height)?;
            if normalized_canvas {
                remove_quietly(&temp_video);
                temp_video = normalized_video.clone();
            }
        }

        let mut mux_audio: PathBuf = req.audio_path.clone().unwrap_or_default();
        let mut audio_matches_video = false;
        let mut mode = String::new();
        if !audio_items.is_empty() && audio_items.iter().all(|i| i.embedded) {
            // Each clip's own audio, cut or padded to the clip's exact length: the timeline frame count when the clips
            // were synced to the timeline, otherwise the clip's own frames / fps.
            let frame_lengths: Vec<(i64, Ratio)> = if !timeline_sync.is_empty() {
                timeline_lengths.clone()
            } else {
                req.scene_paths.iter().map(|p| clip_frame_length(tools, p)).collect::<Result<Vec<_>, _>>()?
            };
            // Where each scene sits in the song. Timing items are relative to the preview start shifted to a whole frame.
            let (song_origin, song_starts): (Option<f64>, Vec<f64>) = if !timeline_sync.is_empty() {
                let fps = f64::from(req.timeline_fps);
                let origin = (req.audio_start * fps + 0.5).floor() / fps;
                let starts = windows.iter().map(|(start, _)| origin + start).collect();
                let gap = windows.windows(2).any(|w| (w[1].0 - w[0].1).abs() > 0.001);
                (if gap { None } else { Some(origin) }, starts)
            } else {
                let mut starts = Vec::new();
                let (mut n, mut d) = (0i128, 1i128);
                for (frames, fps) in &frame_lengths {
                    starts.push(req.audio_start + n as f64 / d as f64);
                    let (add_n, add_d) = (i128::from(*frames) * fps.den, fps.num);
                    n = n * add_d + add_n * d;
                    d *= add_d;
                    let g = gcd_i128(n.abs(), d.abs()).max(1);
                    n /= g;
                    d /= g;
                }
                (Some(req.audio_start), starts)
            };
            let (path, m) = embedded_scene_audio_track(tools, &req.scene_paths, &frame_lengths, &target_dir, &mut temp_audio_parts, song_path.as_deref(), &song_starts, song_origin)?;
            mux_audio = path;
            mode = m;
            audio_matches_video = true;
        } else if !audio_items.is_empty() {
            let mut list = String::new();
            for (i, item) in audio_items.iter().enumerate() {
                let mut path = item.path.clone();
                if item.embedded || item.start != 0.0 || item.duration != 0.0 {
                    let part = target_dir.join(format!("_temp_scene_audio_{:04}.m4a", i + 1));
                    let mut a = args(&["-y", "-ss", &item.start.to_string(), "-i", &s(&item.path)]);
                    if item.duration != 0.0 {
                        a.extend(args(&["-t", &item.duration.to_string()]));
                    }
                    a.extend(args(&["-vn", "-c:a", "aac", &s(&part)]));
                    tools.ffmpeg_checked(&a, "FFmpeg could not cut scene audio.")?;
                    temp_audio_parts.push(part.clone());
                    path = part;
                }
                list.push_str(&format!("file '{}'\n", concat_path(&path)));
            }
            fs::write(&audio_concat_file, list)?;
            tools.ffmpeg_checked(&args(&["-y", "-f", "concat", "-safe", "0", "-i", &s(&audio_concat_file), "-vn", "-c:a", "aac", &s(&temp_audio)]), "FFmpeg could not join scene audio.")?;
            mux_audio = temp_audio.clone();
        } else if req.audio_start != 0.0 || req.audio_duration != 0.0 {
            let mut a = args(&["-y"]);
            if req.audio_start != 0.0 {
                a.extend(args(&["-ss", &format!("{:.6}", req.audio_start)]));
            }
            a.extend(args(&["-i", &s(&mux_audio)]));
            if req.audio_duration != 0.0 {
                a.extend(args(&["-t", &format!("{:.6}", req.audio_duration)]));
            }
            a.extend(args(&["-vn", "-c:a", "aac", &s(&temp_global_audio)]));
            tools.ffmpeg_checked(&a, "FFmpeg could not cut the song.")?;
            mux_audio = temp_global_audio.clone();
        }

        let mut mux = args(&["-y", "-i", &s(&temp_video), "-i", &s(&mux_audio), "-c:v", "copy", "-c:a", "aac"]);
        if timeline_sync.is_empty() && !audio_matches_video {
            // -shortest ends the file at the shorter stream. The joined embedded audio is already exactly the video's
            // length, and -shortest would clip its last few ms with the last frame's duration.
            mux.push("-shortest".into());
        }
        mux.push(s(&final_output));
        tools.ffmpeg_checked(&mux, "FFmpeg could not mux the final video.")?;
        Ok((mode, audio_matches_video))
    };
    let outcome = run_all();
    for p in [&temp_video, &normalized_video, &concat_file, &audio_concat_file, &temp_audio, &temp_global_audio] {
        remove_quietly(p);
    }
    for p in temp_audio_parts.iter().chain(timeline_sync.iter()).chain(transition_files.iter()) {
        remove_quietly(p);
    }
    let (mode, _) = outcome?;
    // Describe the file that was written, not the request.
    let media = probe_output_media(tools, &final_output);
    Ok(StitchResult {
        final_video_path: final_output,
        video_folder: target_dir,
        scene_count: req.scene_paths.len(),
        insert_count: req.overlays.len(),
        used_scene_audio: !audio_items.is_empty(),
        embedded_audio_mode: mode,
        normalized_canvas,
        timeline_frame_sync: !timeline_sync.is_empty(),
        timeline_frame_count: timeline_frames,
        output_width: if media.width > 0 { media.width } else { req.width },
        output_height: if media.height > 0 { media.height } else { req.height },
        output_duration: media.video_duration,
        output_audio_duration: media.audio_duration,
        transitions: applied_transitions,
        transition_notes,
        looks: applied_looks,
        look_notes,
    })
}
