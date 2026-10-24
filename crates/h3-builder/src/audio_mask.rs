//! Per scene audio stems and the masked mix the video model hears (the Audio Mask feature). Port of
//! `builder/audio_stems.py`, with the same files so a project works in both apps.
//!
//! A scene's audio is split into stems with Demucs. For every stem the user sets whether it is masked, the regions that
//! stay audible, a level and mute. The masked mix is the sum of the stems. It is what the video model is rendered with,
//! so the character only lip-syncs the wanted vocal parts. The finished clip gets the original audio back.
//!
//! Files live in `<project>/audio_masks/<scene_id>/`: `original.wav`, one wav per stem, `masked_mix.wav`, `mask.json` and
//! `peaks.json`. Every wav is 44.1 kHz stereo 16 bit and exactly the scene length.
//!
//! The split itself runs in ComfyUI (its Get Stems node), so [`separate_scene_stems`] takes the part that does it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use h3_core::pyval::{round_to, text_or_empty, to_float, truthy, Obj};
use h3_project::atomic::atomic_write_json;
use serde_json::{json, Map, Value};

use crate::files::resolve_existing_file;
use crate::Env;

pub const SAMPLE_RATE: u32 = 44_100;
const FOUR_STEMS: [&str; 4] = ["vocals", "drums", "bass", "other"];
const SIX_STEMS: [&str; 6] = ["vocals", "drums", "bass", "guitar", "piano", "other"];
const MODEL_NAMES: [&str; 4] = ["htdemucs", "htdemucs_ft", "mdx_extra", "htdemucs_6s"];
const DEVICES: [&str; 3] = ["auto", "cuda", "cpu"];
const META_NAME: &str = "mask.json";
const PEAKS_NAME: &str = "peaks.json";
const PEAK_COUNT: usize = 900;
const DEFAULT_FADE_MS: f64 = 30.0;
const MIN_REGION_SECONDS: f64 = 0.005;
const MAX_BATCH_SCENES: usize = 100;

/// The stems a Demucs model returns.
pub fn stems_for_model(model: &str) -> &'static [&'static str] {
    if model == "htdemucs_6s" {
        &SIX_STEMS
    } else {
        &FOUR_STEMS
    }
}

/// Stereo audio as two channels of samples between -1 and 1.
pub type Audio = [Vec<f32>; 2];

/// What a split gave: the stems and the model that made them. A separator may use another model than the one asked for.
pub struct Separated {
    pub model: String,
    pub stems: Vec<(String, Audio)>,
}

/// What splits a cut of the song into stems: the context wav, the model and the device in, the stems out.
pub type Separator<'a> = &'a dyn Fn(&Path, &str, &str) -> Result<Separated, String>;

type Reply = Result<Value, String>;

fn number(payload: &Obj, key: &str, default: f64, min: f64, max: f64) -> f64 {
    let value = match payload.get(key) {
        None => default,
        Some(v) => to_float(v).filter(|f| f.is_finite()).unwrap_or(default),
    };
    value.clamp(min, max)
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

/// An error that names the file it is about.
fn at(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

fn project_of(project_folder: &Value) -> Result<PathBuf, String> {
    let raw = text_or_empty(Some(project_folder)).trim().trim_matches('"').to_string();
    let project = if raw.is_empty() { PathBuf::new() } else { absolute(Path::new(&raw)) };
    if project.as_os_str().is_empty() || !project.is_dir() {
        return Err("Project folder is empty or does not exist.".into());
    }
    Ok(project)
}

/// The folder that holds one scene's stems, inside the project folder.
pub fn scene_folder(project_folder: &Value, scene_id: &Value, create: bool) -> Result<PathBuf, String> {
    let project = project_of(project_folder)?;
    let name = text_or_empty(Some(scene_id)).trim().to_string();
    let valid = !name.is_empty() && name.len() <= 80 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) && name != "." && name != "..";
    if !valid {
        return Err("Scene id is missing or not valid.".into());
    }
    let folder = project.join("audio_masks").join(&name);
    if create {
        fs::create_dir_all(&folder).map_err(|e| at(&folder, e))?;
    }
    Ok(folder)
}

// -- wav files ----------------------------------------------------------------------------------------------------------

/// A 16 bit wav as stereo floats. A mono file is doubled.
pub fn read_wav(path: &Path) -> Result<Audio, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err(format!("Expected 16 bit audio in {}.", path.display()));
    }
    let channels = spec.channels.max(1) as usize;
    let samples: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    let frames = samples.len() / channels;
    let channel = |index: usize| -> Vec<f32> { (0..frames).map(|f| f32::from(samples[f * channels + index.min(channels - 1)]) / 32768.0).collect() };
    Ok([channel(0), channel(1)])
}

/// Write stereo floats as a 44.1 kHz 16 bit wav, replacing the file in one step.
pub fn write_wav(path: &Path, audio: &Audio) -> Result<(), String> {
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let spec = hound::WavSpec { channels: 2, sample_rate: SAMPLE_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut writer = hound::WavWriter::create(&temporary, spec).map_err(|e| at(&temporary, e))?;
        for i in 0..audio[0].len() {
            for channel in audio {
                let value = (channel[i].clamp(-1.0, 1.0) * 32767.0).round_ties_even() as i16;
                writer.write_sample(value).map_err(|e| at(&temporary, e))?;
            }
        }
        writer.finalize().map_err(|e| at(&temporary, e))?;
    }
    fs::rename(&temporary, path).map_err(|e| at(path, e))
}

/// Cut or zero pad to exactly `samples`.
fn fit(audio: &Audio, samples: usize) -> Audio {
    let one = |channel: &Vec<f32>| -> Vec<f32> {
        let mut out: Vec<f32> = channel.iter().take(samples).copied().collect();
        out.resize(samples, 0.0);
        out
    };
    [one(&audio[0]), one(&audio[1])]
}

/// Largest absolute level in each of `count` equal slices, for drawing a waveform.
pub fn peaks(audio: &Audio, count: usize) -> Vec<f64> {
    let length = audio[0].len().max(audio[1].len()).max(1);
    let level = |i: usize| -> f32 { audio[0].get(i).copied().unwrap_or(0.0).abs().max(audio[1].get(i).copied().unwrap_or(0.0).abs()) };
    let bucket = length.div_ceil(count).max(1);
    (0..length.div_ceil(bucket))
        .map(|b| {
            let top = (b * bucket..((b + 1) * bucket).min(length)).map(level).fold(0.0f32, f32::max);
            round_to(f64::from(top), 4)
        })
        .collect()
}

fn wav_samples(path: &Path) -> Result<usize, String> {
    hound::WavReader::open(path).map(|r| r.duration() as usize).map_err(|e| e.to_string())
}

// -- settings and mixing ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub start: f64,
    pub end: f64,
    pub fade_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StemSettings {
    pub mask: bool,
    pub regions: Vec<Region>,
    pub db: f64,
    pub mute: bool,
}

impl StemSettings {
    fn as_json(&self) -> Value {
        json!({
            "mask": self.mask,
            "regions": self.regions.iter().map(|r| json!({"start": r.start, "end": r.end, "fade_ms": r.fade_ms})).collect::<Vec<_>>(),
            "db": self.db,
            "mute": self.mute,
        })
    }
}

/// Regions as sorted `{start, end, fade_ms}` in scene seconds, clamped to the scene.
pub fn normalize_regions(raw: Option<&Value>, duration: f64) -> Vec<Region> {
    let mut regions: Vec<Region> = raw
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let item = item.as_object()?;
            let start = item.get("start").and_then(to_float)?;
            let end = item.get("end").and_then(to_float)?;
            let fade = match item.get("fade_ms") {
                None => DEFAULT_FADE_MS,
                Some(v) => to_float(v)?,
            };
            if !(start.is_finite() && end.is_finite() && fade.is_finite()) {
                return None;
            }
            let (start, end) = (start.clamp(0.0, duration), end.clamp(0.0, duration));
            (end - start >= MIN_REGION_SECONDS).then(|| Region { start: round_to(start, 4), end: round_to(end, 4), fade_ms: fade.clamp(0.0, 1000.0) })
        })
        .collect();
    regions.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    regions
}

/// Per stem: `mask` (only the regions are audible), `regions`, `db` (level) and `mute`. A missing stem plays as it is.
pub fn normalize_stem_settings(raw: Option<&Value>, names: &[String], duration: f64) -> Vec<(String, StemSettings)> {
    let empty = Map::new();
    let source = raw.and_then(Value::as_object).unwrap_or(&empty);
    names
        .iter()
        .map(|name| {
            let item = source.get(name).and_then(Value::as_object).cloned().unwrap_or_default();
            let settings = StemSettings {
                mask: item.get("mask").is_some_and(truthy),
                regions: normalize_regions(item.get("regions"), duration),
                db: number(&item, "db", 0.0, -100.0, 24.0),
                mute: item.get("mute").is_some_and(truthy),
            };
            (name.clone(), settings)
        })
        .collect()
}

/// 1 where a stem is kept, 0 where it is silenced, with a smooth fade inside each region's edges.
pub fn keep_envelope(regions: &[Region], samples: usize) -> Vec<f32> {
    let mut envelope = vec![0.0f32; samples];
    for region in regions {
        let first = ((region.start * f64::from(SAMPLE_RATE)).round_ties_even().max(0.0)) as usize;
        let last = (((region.end * f64::from(SAMPLE_RATE)).round_ties_even()) as usize).min(samples);
        if last <= first {
            continue;
        }
        let mut piece = vec![1.0f32; last - first];
        let fade = ((region.fade_ms / 1000.0 * f64::from(SAMPLE_RATE)) as usize).min((last - first) / 2);
        if fade > 0 {
            let ramp: Vec<f32> = (0..fade).map(|i| 0.5 - 0.5 * (std::f32::consts::PI * i as f32 / fade as f32).cos()).collect();
            for (i, r) in ramp.iter().enumerate() {
                piece[i] *= r;
                let end = piece.len() - 1 - i;
                piece[end] *= r;
            }
        }
        for (slot, value) in envelope[first..last].iter_mut().zip(piece) {
            *slot = slot.max(value);
        }
    }
    envelope
}

/// The masked mix: every stem with its mask, level and mute applied, added together and kept below clipping.
pub fn mix_stems(stems: &[(String, Audio)], settings: &[(String, StemSettings)]) -> Audio {
    let samples = stems.first().map_or(0, |(_, a)| a[0].len());
    let mut mix: Audio = [vec![0.0; samples], vec![0.0; samples]];
    for (name, audio) in stems {
        let item = settings.iter().find(|(n, _)| n == name).map(|(_, s)| s.clone()).unwrap_or(StemSettings { mask: false, regions: vec![], db: 0.0, mute: false });
        if item.mute {
            continue;
        }
        let gain = 10.0f32.powf(item.db as f32 / 20.0);
        let envelope = item.mask.then(|| keep_envelope(&item.regions, samples));
        for channel in 0..2 {
            for i in 0..samples {
                let mut value = audio[channel][i] * gain;
                if let Some(envelope) = &envelope {
                    value *= envelope[i];
                }
                mix[channel][i] += value;
            }
        }
    }
    let top = mix.iter().flatten().fold(0.0f32, |t, v| t.max(v.abs()));
    if top > 0.98 {
        let scale = 0.98 / top;
        mix.iter_mut().flatten().for_each(|v| *v *= scale);
    }
    mix
}

// -- what is on disk ----------------------------------------------------------------------------------------------------

fn meta_path(folder: &Path) -> PathBuf {
    folder.join(META_NAME)
}

fn load_meta(folder: &Path) -> Obj {
    match fs::read_to_string(meta_path(folder)).ok().and_then(|t| h3_core::pyjson::parse(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Obj::new(),
    }
}

/// The stems a scene was split into. A scene split before the 6 stem model existed has the four basic ones.
fn stem_names(meta: &Obj) -> Vec<String> {
    let saved = meta.get("separation").and_then(|s| s.get("stem_names")).and_then(Value::as_array);
    let names: Vec<String> = saved.into_iter().flatten().filter_map(Value::as_str).filter(|n| SIX_STEMS.contains(n)).map(str::to_string).collect();
    if names.is_empty() {
        FOUR_STEMS.iter().map(|s| s.to_string()).collect()
    } else {
        names
    }
}

fn modified_seconds(path: &Path) -> f64 {
    fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64())
}

fn now_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Waveform peaks for each wav, kept in `peaks.json` so reading a scene's state does not analyse every stem again. An
/// entry is reused while its wav has the same size and modified time.
fn lane_peaks(folder: &Path, files: &[(String, PathBuf)]) -> Result<Obj, String> {
    let cache_path = folder.join(PEAKS_NAME);
    let mut saved = match fs::read_to_string(&cache_path).ok().and_then(|t| h3_core::pyjson::parse(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Obj::new(),
    };
    let mut lanes = Obj::new();
    let mut changed = false;
    for (name, path) in files {
        let size = fs::metadata(path).map_or(0, |m| m.len());
        let stamp = format!("{:.3}:{size}", modified_seconds(path));
        if let Some(entry) = saved.get(name).and_then(Value::as_object) {
            if entry.get("stamp").and_then(Value::as_str) == Some(stamp.as_str()) && entry.get("peaks").is_some_and(Value::is_array) {
                lanes.insert(name.clone(), entry["peaks"].clone());
                continue;
            }
        }
        let computed = json!(peaks(&read_wav(path)?, PEAK_COUNT));
        lanes.insert(name.clone(), computed.clone());
        saved.insert(name.clone(), json!({"stamp": stamp, "peaks": computed}));
        changed = true;
    }
    let stale: Vec<String> = saved.keys().filter(|k| !files.iter().any(|(n, _)| n == *k)).cloned().collect();
    for key in stale {
        saved.shift_remove(&key);
        changed = true;
    }
    if changed {
        atomic_write_json(&cache_path, &Value::Object(saved)).map_err(|e| e.to_string())?;
    }
    Ok(lanes)
}

/// `POST .../audio_mask/state`: settings used, file paths and waveform peaks. `exists` is false before the first split.
pub fn scene_state(payload: &Obj) -> Reply {
    let null = Value::Null;
    let folder = scene_folder(payload.get("project_folder").unwrap_or(&null), payload.get("scene_id").unwrap_or(&null), false)?;
    let meta = load_meta(&folder);
    let names = stem_names(&meta);
    let stem_files: Vec<PathBuf> = names.iter().map(|n| folder.join(format!("{n}.wav"))).collect();
    let scene_id = text_or_empty(payload.get("scene_id"));
    if meta.is_empty() || !stem_files.iter().all(|p| p.is_file()) {
        return Ok(json!({"ok": true, "exists": false, "scene_id": scene_id}));
    }
    let samples = wav_samples(&stem_files[0])?;
    let candidates: Vec<String> = std::iter::once("original".to_string()).chain(names.iter().cloned()).chain(std::iter::once("masked_mix".to_string())).collect();
    let files: Vec<(String, PathBuf)> = candidates.into_iter().map(|n| (n.clone(), folder.join(format!("{n}.wav")))).filter(|(_, p)| p.is_file()).collect();
    let mix = meta.get("mix").filter(|m| m.is_object() && files.iter().any(|(n, _)| n == "masked_mix")).cloned();
    let wanted: Vec<(String, PathBuf)> = files.iter().filter(|(n, _)| n != "masked_mix" || mix.is_some()).cloned().collect();
    let lanes = lane_peaks(&folder, &wanted)?;
    let file_map: Obj = files.iter().map(|(n, p)| (n.clone(), json!(text(p)))).collect();
    Ok(json!({
        "ok": true,
        "exists": true,
        "scene_id": scene_id,
        "duration": round_to(samples as f64 / f64::from(SAMPLE_RATE), 4),
        "stem_names": names,
        "separation": meta.get("separation").cloned().unwrap_or(json!({})),
        "mix": mix.unwrap_or(Value::Null),
        "files": file_map,
        "peaks": lanes,
    }))
}

/// `POST .../audio_mask/states`: the state of many scenes at once, so a project opens with one request. A scene whose
/// state cannot be read is reported as having no stems.
pub fn scene_states(payload: &Obj) -> Reply {
    let Some(ids) = payload.get("scene_ids").and_then(Value::as_array) else { return Err("scene_ids must be a list.".into()) };
    let mut states = Obj::new();
    for id in ids.iter().take(MAX_BATCH_SCENES) {
        let scene_id = text_or_empty(Some(id));
        let mut request = Obj::new();
        request.insert("project_folder".into(), payload.get("project_folder").cloned().unwrap_or(Value::Null));
        request.insert("scene_id".into(), json!(scene_id));
        let state = scene_state(&request).unwrap_or_else(|_| json!({"ok": true, "exists": false, "scene_id": scene_id}));
        states.insert(scene_id, state);
    }
    Ok(json!({"ok": true, "states": states}))
}

fn ffmpeg_run(env: &Env, args: Vec<String>, message: &str) -> Result<(), String> {
    let tools = env.tools()?;
    tools.ffmpeg_checked(&args, message).map(|_| ()).map_err(|e| {
        let full = e.to_string();
        let tail: String = full.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
        if tail.trim().is_empty() { message.to_string() } else { tail.trim().to_string() }
    })
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// `POST .../audio_mask/separate`: cut the scene's audio (with some context on both sides), apply the input gain, split
/// it into stems and save them. The context makes Demucs more accurate at the scene edges and only the scene is kept.
/// The old masked mix is removed because it no longer matches.
pub fn separate_scene_stems(env: &Env, payload: &Obj, separator: Separator) -> Reply {
    let null = Value::Null;
    let folder = scene_folder(payload.get("project_folder").unwrap_or(&null), payload.get("scene_id").unwrap_or(&null), true)?;
    let source = resolve_existing_file(&text_or_empty(payload.get("audio_path")), "Scene audio")?;
    let start = number(payload, "start_seconds", 0.0, 0.0, 86_400.0);
    let duration = number(payload, "duration_seconds", 0.0, 0.0, 300.0);
    if duration < 0.05 {
        return Err("The scene needs a length before its audio can be split.".into());
    }
    let model = match text_or_empty(payload.get("model_name")) {
        m if m.is_empty() => "htdemucs".to_string(),
        m => m,
    };
    if !MODEL_NAMES.contains(&model.as_str()) {
        return Err(format!("Unknown Demucs model: {model}"));
    }
    let device = match text_or_empty(payload.get("device")) {
        d if d.is_empty() => "auto".to_string(),
        d => d,
    };
    if !DEVICES.contains(&device.as_str()) {
        return Err(format!("Unknown device: {device}"));
    }
    let gain_db = number(payload, "input_gain_db", 0.0, -24.0, 24.0);
    let context = number(payload, "context_seconds", 3.0, 0.0, 10.0);
    let lead = context.min(start);
    let samples = (duration * f64::from(SAMPLE_RATE)).round_ties_even() as usize;
    let context_path = folder.join("_context.wav");
    let original_path = folder.join("original.wav");
    let mut filters: Vec<String> = Vec::new();
    if gain_db != 0.0 {
        filters.push(format!("volume={gain_db:.2}dB"));
        if gain_db > 0.0 {
            filters.push("alimiter=limit=0.97".into());
        }
    }
    let common = strings(&["-vn", "-ac", "2", "-ar", &SAMPLE_RATE.to_string()]);
    let started = now_seconds();

    let mut cut = strings(&["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{:.6}", start - lead), "-i", &text(&source), "-t", &format!("{:.6}", lead + duration + context)]);
    cut.extend(common.clone());
    if !filters.is_empty() {
        cut.extend(strings(&["-af", &filters.join(",")]));
    }
    cut.extend(strings(&["-c:a", "pcm_s16le", &text(&context_path)]));
    ffmpeg_run(env, cut, "ffmpeg could not cut the scene audio.")?;
    let mut exact = strings(&["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{start:.6}"), "-i", &text(&source), "-t", &format!("{duration:.6}")]);
    exact.extend(common);
    exact.extend(strings(&["-c:a", "pcm_s16le", &text(&original_path)]));
    ffmpeg_run(env, exact, "ffmpeg could not cut the scene audio.")?;

    let separated = separator(&context_path, &model, &device);
    let _ = fs::remove_file(&context_path);
    let Separated { model, stems } = separated?;
    let first = (lead * f64::from(SAMPLE_RATE)).round_ties_even() as usize;
    for name in SIX_STEMS {
        let path = folder.join(format!("{name}.wav"));
        match stems.iter().find(|(n, _)| n == name) {
            Some((_, audio)) => {
                let piece: Audio = [audio[0].iter().skip(first).take(samples).copied().collect(), audio[1].iter().skip(first).take(samples).copied().collect()];
                write_wav(&path, &fit(&piece, samples))?;
            }
            // A stem the new model does not have, left by an earlier split.
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    write_wav(&original_path, &fit(&read_wav(&original_path)?, samples))?;
    let _ = fs::remove_file(folder.join("masked_mix.wav"));
    let finished = now_seconds();
    let meta = json!({"separation": {
        "model_name": model, "stem_names": stems.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(), "input_gain_db": gain_db,
        "context_seconds": context, "device": device, "source_path": text(&source), "start_seconds": start,
        "duration_seconds": duration, "created_at": finished, "seconds_taken": round_to(finished - started, 2),
    }});
    atomic_write_json(&meta_path(&folder), &meta).map_err(|e| e.to_string())?;
    scene_state(payload)
}

/// `POST .../audio_mask/render`: build `masked_mix.wav` from the saved stems and the per stem settings.
pub fn render_masked_mix(payload: &Obj) -> Reply {
    let null = Value::Null;
    let folder = scene_folder(payload.get("project_folder").unwrap_or(&null), payload.get("scene_id").unwrap_or(&null), false)?;
    let mut meta = load_meta(&folder);
    let names = stem_names(&meta);
    let paths: Vec<PathBuf> = names.iter().map(|n| folder.join(format!("{n}.wav"))).collect();
    if meta.is_empty() || !paths.iter().all(|p| p.is_file()) {
        return Err("Split this scene's audio into stems first.".into());
    }
    let stems: Vec<(String, Audio)> = names.iter().zip(&paths).map(|(n, p)| read_wav(p).map(|a| (n.clone(), a))).collect::<Result<_, _>>()?;
    let samples = stems[0].1[0].len();
    let duration = samples as f64 / f64::from(SAMPLE_RATE);
    let settings = normalize_stem_settings(payload.get("stems"), &names, duration);
    let mix_path = folder.join("masked_mix.wav");
    write_wav(&mix_path, &fit(&mix_stems(&stems, &settings), samples))?;
    let saved: Obj = settings.iter().map(|(n, s)| (n.clone(), s.as_json())).collect();
    meta.insert("mix".into(), json!({"stems": saved, "built_at": now_seconds(), "path": text(&mix_path), "duration_seconds": round_to(duration, 4)}));
    atomic_write_json(&meta_path(&folder), &Value::Object(meta)).map_err(|e| e.to_string())?;
    scene_state(payload)
}

/// `POST .../audio_mask/list`: the scenes that have stems saved, so stems of scenes that no longer exist can be removed.
pub fn list_scene_stems(payload: &Obj) -> Reply {
    let project = project_of(payload.get("project_folder").unwrap_or(&Value::Null))?;
    let root = project.join("audio_masks");
    let mut ids: Vec<String> = fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(META_NAME).is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    ids.sort();
    Ok(json!({"ok": true, "scene_ids": ids}))
}

/// `POST .../audio_mask/delete`: remove a scene's stems and masked mix.
pub fn delete_scene_stems(payload: &Obj) -> Reply {
    let null = Value::Null;
    let folder = scene_folder(payload.get("project_folder").unwrap_or(&null), payload.get("scene_id").unwrap_or(&null), false)?;
    let mut removed = 0;
    if folder.is_dir() {
        let reason: String = match payload.get("reason") {
            Some(v) if truthy(v) => text_or_empty(Some(v)).chars().take(120).collect(),
            _ => "no reason given".into(),
        };
        eprintln!("[VRGDG-RS Audio Mask] Removing the stems of scene {}: {reason}", text_or_empty(payload.get("scene_id")));
        for entry in fs::read_dir(&folder).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == META_NAME || name == PEAKS_NAME || name.ends_with(".wav") || name.ends_with(".tmp") {
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                removed += 1;
            }
        }
        let _ = fs::remove_dir(&folder);
    }
    Ok(json!({"ok": true, "removed": removed}))
}

/// Decode any audio file ffmpeg reads to 44.1 kHz stereo floats. Used for the stems ComfyUI saves.
pub fn decode_audio(env: &Env, path: &Path) -> Result<Audio, String> {
    let tools = env.tools()?;
    let args = strings(&["-hide_banner", "-loglevel", "error", "-i", &text(path), "-vn", "-ac", "2", "-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "pipe:1"]);
    let output = tools.ffmpeg_checked(&args, "ffmpeg could not read the stem").map_err(|e| e.to_string())?;
    let samples: Vec<f32> = output.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    Ok([samples.iter().step_by(2).copied().collect(), samples.iter().skip(1).step_by(2).copied().collect()])
}

/// Keep ffmpeg's console window away on Windows for callers that run it themselves.
pub fn quiet(command: &mut Command) -> &mut Command {
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn tone(samples: usize, value: f32) -> Audio {
        [vec![value; samples], vec![value; samples]]
    }

    #[test]
    fn regions_are_clamped_sorted_and_short_ones_dropped() {
        let raw = json!([{"start": 5, "end": 9}, {"start": -1, "end": 2, "fade_ms": 5000}, {"start": 3, "end": 3.001}, "x", {"start": "a", "end": 1}]);
        let regions = normalize_regions(Some(&raw), 6.0);
        assert_eq!(regions, vec![Region { start: 0.0, end: 2.0, fade_ms: 1000.0 }, Region { start: 5.0, end: 6.0, fade_ms: 30.0 }]);
    }

    #[test]
    fn envelope_is_zero_outside_and_fades_inside_regions() {
        let region = Region { start: 0.1, end: 0.3, fade_ms: 20.0 };
        let envelope = keep_envelope(&[region], SAMPLE_RATE as usize);
        assert_eq!(envelope[0], 0.0);
        assert_eq!(envelope[(0.2 * 44100.0) as usize], 1.0);
        assert!(envelope[(0.1 * 44100.0) as usize] < 0.01);
        assert_eq!(envelope[(0.5 * 44100.0) as usize], 0.0);
    }

    #[test]
    fn mix_applies_mute_level_mask_and_limits_the_peak() {
        let stems = vec![("vocals".to_string(), tone(100, 0.5)), ("drums".to_string(), tone(100, 0.5)), ("bass".to_string(), tone(100, 0.9))];
        let settings = normalize_stem_settings(Some(&json!({"drums": {"mute": true}, "vocals": {"db": -6.0206}, "bass": {"mask": true}})), &stems.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(), 1.0);
        let mix = mix_stems(&stems, &settings);
        // Drums are muted, bass is masked with no region, vocals are about half level.
        assert!((mix[0][50] - 0.25).abs() < 0.001);
        let loud = mix_stems(&[("a".to_string(), tone(10, 0.9)), ("b".to_string(), tone(10, 0.9))], &[]);
        assert!((loud[0][0] - 0.98).abs() < 1e-6);
    }

    #[test]
    fn wav_round_trip_and_peaks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let audio: Audio = [(0..1000).map(|i| (i as f32 / 1000.0) - 0.5).collect(), vec![0.25; 1000]];
        write_wav(&path, &audio).unwrap();
        let back = read_wav(&path).unwrap();
        assert_eq!(back[0].len(), 1000);
        assert!((back[0][999] - audio[0][999]).abs() < 0.001);
        assert_eq!(wav_samples(&path).unwrap(), 1000);
        let lane = peaks(&back, 10);
        assert_eq!(lane.len(), 10);
        assert!(lane.iter().all(|p| *p <= 0.5001));
    }

    #[test]
    fn scene_folder_refuses_odd_ids() {
        let dir = tempfile::tempdir().unwrap();
        let project = json!(dir.path().display().to_string());
        assert!(scene_folder(&project, &json!("../x"), false).is_err());
        assert!(scene_folder(&project, &json!(".."), false).is_err());
        assert!(scene_folder(&project, &json!(""), false).is_err());
        assert!(scene_folder(&project, &json!("scene_1.a-b"), true).unwrap().is_dir());
        assert!(scene_folder(&json!("Y:\\nowhere"), &json!("a"), false).is_err());
    }

    #[test]
    fn separation_cuts_the_scene_and_saves_the_stems() {
        let env = Env { output: PathBuf::new(), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: PathBuf::new() };
        if env.tools().is_err() {
            return; // ffmpeg is not installed here
        }
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.wav");
        write_wav(&song, &tone(44_100 * 6, 0.2)).unwrap();
        let payload = obj(json!({
            "project_folder": dir.path().display().to_string(), "scene_id": "s1", "audio_path": text(&song),
            "start_seconds": 1.0, "duration_seconds": 2.0, "model_name": "htdemucs", "device": "cpu", "context_seconds": 1.0,
        }));
        let separator = |context: &Path, model: &str, device: &str| -> Result<Separated, String> {
            assert!(context.is_file());
            assert_eq!((model, device), ("htdemucs", "cpu"));
            let length = read_wav(context).unwrap()[0].len();
            // One second of lead, two of scene and one after.
            assert!((length as i64 - 44_100 * 4).abs() < 2000, "context was {length} samples");
            Ok(Separated { model: model.to_string(), stems: FOUR_STEMS.iter().map(|n| (n.to_string(), tone(length, 0.1))).collect() })
        };
        let state = separate_scene_stems(&env, &payload, &separator).unwrap();
        assert_eq!(state["exists"], true);
        assert_eq!(state["duration"], 2.0);
        assert_eq!(state["separation"]["model_name"], "htdemucs");
        assert_eq!(state["stem_names"], json!(FOUR_STEMS));
        let folder = dir.path().join("audio_masks").join("s1");
        assert!(!folder.join("_context.wav").exists());
        assert_eq!(wav_samples(&folder.join("vocals.wav")).unwrap(), 88_200);
        assert_eq!(wav_samples(&folder.join("original.wav")).unwrap(), 88_200);
        let failing = |_: &Path, _: &str, _: &str| -> Result<Separated, String> { Err("no GPU".into()) };
        assert_eq!(separate_scene_stems(&env, &payload, &failing).unwrap_err(), "no GPU");
        assert!(!folder.join("_context.wav").exists());
    }

    #[test]
    fn state_render_list_and_delete_follow_the_saved_files() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().display().to_string();
        let request = obj(json!({"project_folder": project, "scene_id": "s1"}));
        assert_eq!(scene_state(&request).unwrap()["exists"], false);
        assert!(render_masked_mix(&request).unwrap_err().starts_with("Split this scene"));

        let folder = scene_folder(&json!(project), &json!("s1"), true).unwrap();
        for name in FOUR_STEMS {
            write_wav(&folder.join(format!("{name}.wav")), &tone(4410, 0.3)).unwrap();
        }
        write_wav(&folder.join("original.wav"), &tone(4410, 0.3)).unwrap();
        atomic_write_json(&meta_path(&folder), &json!({"separation": {"stem_names": FOUR_STEMS, "model_name": "htdemucs"}})).unwrap();
        let state = scene_state(&request).unwrap();
        assert_eq!(state["exists"], true);
        assert_eq!(state["duration"], 0.1);
        assert_eq!(state["mix"], Value::Null);
        assert!(state["peaks"]["vocals"].as_array().unwrap().len() <= PEAK_COUNT);

        let rendered = render_masked_mix(&obj(json!({"project_folder": project, "scene_id": "s1", "stems": {"vocals": {"mask": true, "regions": [{"start": 0.02, "end": 0.08}]}}}))).unwrap();
        assert!(rendered["mix"]["path"].as_str().unwrap().ends_with("masked_mix.wav"));
        assert_eq!(rendered["mix"]["stems"]["vocals"]["regions"][0]["fade_ms"], 30.0);
        assert!(rendered["files"].get("masked_mix").is_some());
        assert_eq!(list_scene_stems(&obj(json!({"project_folder": project}))).unwrap()["scene_ids"], json!(["s1"]));
        let states = scene_states(&obj(json!({"project_folder": project, "scene_ids": ["s1", "missing"]}))).unwrap();
        assert_eq!(states["states"]["s1"]["exists"], true);
        assert_eq!(states["states"]["missing"]["exists"], false);
        assert!(scene_states(&obj(json!({"project_folder": project}))).is_err());

        assert!(delete_scene_stems(&request).unwrap()["removed"].as_i64().unwrap() >= 6);
        assert!(!folder.exists());
    }
}
