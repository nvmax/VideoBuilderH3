//! Thumbnail intro: keep a generated thumbnail in the project and put it in front of the stitched video. Port of
//! `builder/thumbnail.py`.
//!
//! The Builder's Thumbnail dialog makes the picture (the LLM writes the edit prompt, NanoBanana or Flux/Klein draws it) and
//! calls [`save_thumbnail_image`] to keep it in `<project>/thumbnail`. After the final stitch the Builder calls
//! [`prepend_thumbnail_intro`], which shows the picture for a few seconds in silence before the first scene. The scene clips,
//! the timeline and the audio sync are never touched: only the stitched video is rewritten.

use std::fs;
use std::path::{Path, PathBuf};

use h3_core::pyval::{to_float, Obj};
use h3_media::audio::probe_duration;
use h3_media::tools::{args, scene_audio_args, scene_video_args, source_crf, Tools};
use h3_media::video::{probe_stream_fields, Ratio};
use image::ImageFormat;
use serde_json::{json, Value};

use crate::payload::path_text;
use crate::runner::{resolve_comfy_image, ComfyFolders};
use crate::Env;

type Reply = Result<Value, String>;

/// The project folder the pictures are kept in.
pub const THUMBNAIL_FOLDER: &str = "thumbnail";
/// Bumped when the routes or their fields change in a way the Builder has to know about.
pub const THUMBNAIL_VERSION: u32 = 1;
pub const DEFAULT_INTRO_SECONDS: f64 = 2.0;
pub const MIN_INTRO_SECONDS: f64 = 0.5;
pub const MAX_INTRO_SECONDS: f64 = 10.0;
/// How far the joined video may differ from source + intro before the stream-copy join is thrown away.
const DURATION_TOLERANCE_SECONDS: f64 = 0.35;
const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "m4v"];

/// `GET /vrgdg/music_builder/thumbnail/capabilities`: tells the Builder that this build can save thumbnails and prepend the intro.
pub fn capabilities() -> Reply {
    Ok(json!({"version": THUMBNAIL_VERSION}))
}

fn absolute(raw: &str) -> PathBuf {
    h3_core::abspath::absolute(raw).unwrap_or_else(|_| PathBuf::from(raw)).components().collect()
}

/// `_intro_seconds`: the intro length, kept between [`MIN_INTRO_SECONDS`] and [`MAX_INTRO_SECONDS`] (the default for anything else).
pub fn intro_seconds(value: Option<&Value>) -> f64 {
    match value.and_then(to_float) {
        Some(seconds) if seconds.is_finite() => seconds.clamp(MIN_INTRO_SECONDS, MAX_INTRO_SECONDS),
        _ => DEFAULT_INTRO_SECONDS,
    }
}

/// `_next_thumbnail_path`: `thumbnail_NNNN.png` with the first number that is free in `folder`.
fn next_thumbnail_path(folder: &Path) -> PathBuf {
    let mut number = 1;
    while folder.join(format!("thumbnail_{number:04}.png")).exists() {
        number += 1;
    }
    folder.join(format!("thumbnail_{number:04}.png"))
}

/// `POST /vrgdg/music_builder/thumbnail/save`: `{project_folder, image}` (a ComfyUI `/history` image entry) or `{project_folder, source_path}`.
/// Every call keeps its own numbered PNG, so earlier tries stay available in the dialog's history.
pub fn save_thumbnail_image(folders: &ComfyFolders, payload: &Obj) -> Reply {
    // An empty path would resolve to the folder the server runs in, so it is refused before any folder is made.
    let project_text = path_text(payload, "project_folder");
    if project_text.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let folder = absolute(&project_text).join(THUMBNAIL_FOLDER);
    let source = match payload.get("image").and_then(Value::as_object) {
        Some(info) => resolve_comfy_image(folders, info)?,
        None => {
            let path = absolute(&path_text(payload, "source_path"));
            if !path.is_file() {
                return Err(format!("Thumbnail image was not found: {}", path.display()));
            }
            path
        }
    };
    let picture = image::open(&source).map_err(|e| format!("Could not read the picture {}: {e}", source.display()))?;
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let target = next_thumbnail_path(&folder);
    picture.to_rgb8().save_with_format(&target, ImageFormat::Png).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(json!({"image_path": target.display().to_string(), "width": picture.width(), "height": picture.height()}))
}

/// A frame rate as ffmpeg reads it: `24` or `30000/1001`.
fn rate_text(rate: Ratio) -> String {
    if rate.den == 1 { rate.num.to_string() } else { format!("{}/{}", rate.num, rate.den) }
}

fn frame_rate(values: &std::collections::HashMap<String, String>) -> Ratio {
    ["r_frame_rate", "avg_frame_rate"].iter().filter_map(|k| values.get(*k).and_then(|v| Ratio::parse(v))).find(Ratio::is_positive).unwrap_or(Ratio::new(24, 1))
}

/// The ffmpeg arguments that turn the picture into a silent clip with the video's size, frame rate and audio format.
fn intro_command(image: &Path, output: &Path, seconds: f64, size: (i64, i64), rate: Ratio, audio: Option<(i64, i64)>, crf: Option<f64>) -> Vec<String> {
    let (width, height) = size;
    let fill = format!("scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1,format=yuv420p");
    let length = format!("{seconds:.3}");
    let rate = rate_text(rate);
    let mut command = args(&["-y", "-loop", "1", "-framerate", &rate, "-t", &length, "-i", &image.display().to_string()]);
    if let Some((sample_rate, channels)) = audio {
        let layout = if channels == 1 { "mono" } else { "stereo" };
        command.extend(args(&["-f", "lavfi", "-t", &length, "-i", &format!("anullsrc=r={sample_rate}:cl={layout}")]));
    }
    command.extend(args(&["-vf", &fill, "-r", &rate]));
    command.extend(scene_video_args(crf));
    match audio {
        Some((sample_rate, channels)) => {
            command.extend(scene_audio_args());
            command.extend(args(&["-ar", &sample_rate.to_string(), "-ac", &channels.to_string(), "-shortest"]));
        }
        None => command.push("-an".into()),
    }
    command.push(output.display().to_string());
    command
}

/// Join without re-encoding the video: the intro was made to match, so the streams can be copied.
fn join_by_copy(tools: &Tools, intro: &Path, source: &Path, output: &Path, work: &Path) -> Result<(), String> {
    let listing = work.join("join.txt");
    let mut lines = String::new();
    for path in [intro, source] {
        lines.push_str(&format!("file '{}'\n", path.display().to_string().replace('\\', "/").replace('\'', "'\\''")));
    }
    fs::write(&listing, lines).map_err(|e| e.to_string())?;
    let command = args(&["-y", "-f", "concat", "-safe", "0", "-i", &listing.display().to_string(), "-c", "copy", "-movflags", "+faststart", &output.display().to_string()]);
    tools.ffmpeg_checked(&command, "FFmpeg could not join the thumbnail and the video.").map(|_| ()).map_err(|e| e.to_string())
}

/// Join with the concat filter and one new encode, for the rare video the stream-copy join does not fit.
fn join_by_encoding(tools: &Tools, intro: &Path, source: &Path, output: &Path, has_audio: bool, crf: Option<f64>) -> Result<(), String> {
    let (graph, maps): (&str, Vec<&str>) = if has_audio {
        ("[0:v][0:a][1:v][1:a]concat=n=2:v=1:a=1[v][a]", vec!["-map", "[v]", "-map", "[a]"])
    } else {
        ("[0:v][1:v]concat=n=2:v=1:a=0[v]", vec!["-map", "[v]", "-an"])
    };
    let mut command = args(&["-y", "-i", &intro.display().to_string(), "-i", &source.display().to_string(), "-filter_complex", graph]);
    command.extend(args(&maps));
    command.extend(scene_video_args(crf));
    if has_audio {
        command.extend(scene_audio_args());
    }
    command.extend(args(&["-movflags", "+faststart", &output.display().to_string()]));
    tools.ffmpeg_checked(&command, "FFmpeg could not join the thumbnail and the video.").map(|_| ()).map_err(|e| e.to_string())
}

/// A hidden work folder beside the video, removed when it goes out of scope.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new(beside: &Path) -> Result<Self, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let path = beside.join(format!(".thumbnail_intro_{}_{nanos}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn length_of(tools: &Tools, path: &Path) -> f64 {
    probe_duration(tools, &path.display().to_string()).unwrap_or(0.0)
}

fn whole_field(values: &std::collections::HashMap<String, String>, key: &str) -> Option<i64> {
    values.get(key).and_then(|v| v.trim().parse().ok())
}

/// `POST /vrgdg/music_builder/thumbnail/prepend`: `{input_path, image_path, seconds?, replace_source?, crf?}`. Shows `image_path` for a few
/// seconds in front of `input_path` (a stitched video), in place unless `replace_source` is false.
pub fn prepend_thumbnail_intro(env: &Env, payload: &Obj) -> Reply {
    let source = absolute(&path_text(payload, "input_path"));
    let image_path = absolute(&path_text(payload, "image_path"));
    let supported = source.extension().is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()));
    if !source.is_file() || !supported {
        return Err(format!("The video to put the thumbnail in front of was not found: {}", source.display()));
    }
    if !image_path.is_file() {
        return Err(format!("The thumbnail image was not found: {}", image_path.display()));
    }
    image::image_dimensions(&image_path).map_err(|e| format!("Could not read the picture {}: {e}", image_path.display()))?;
    let seconds = intro_seconds(payload.get("seconds"));
    let replace_source = payload.get("replace_source") != Some(&Value::Bool(false));

    let tools = env.tools()?;
    let video = probe_stream_fields(&tools, &source, "v:0", &["width", "height", "r_frame_rate", "avg_frame_rate"], false).map_err(|e| e.to_string())?;
    let (Some(width), Some(height)) = (whole_field(&video, "width").filter(|w| *w > 0), whole_field(&video, "height").filter(|h| *h > 0)) else {
        return Err(format!("Could not read the size of the video: {}", source.display()));
    };
    let audio_fields = probe_stream_fields(&tools, &source, "a:0", &["sample_rate", "channels"], false).map_err(|e| e.to_string())?;
    let audio = whole_field(&audio_fields, "sample_rate").map(|rate| (rate, whole_field(&audio_fields, "channels").unwrap_or(2).max(1)));
    let rate = frame_rate(&video);
    let crf = payload.get("crf").and_then(to_float).or_else(|| source_crf(&[&source]));
    let source_duration = length_of(&tools, &source);

    let folder = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let work = WorkDir::new(&folder)?;
    let intro = work.0.join("intro.mp4");
    let command = intro_command(&image_path, &intro, seconds, (width, height), rate, audio, crf);
    tools.ffmpeg_checked(&command, "FFmpeg could not make the thumbnail clip.").map_err(|e| e.to_string())?;
    let joined = work.0.join("joined.mp4");
    let mut method = "copy";
    let copied = join_by_copy(&tools, &intro, &source, &joined, &work.0).and_then(|_| {
        let drift = (length_of(&tools, &joined) - (source_duration + seconds)).abs();
        if source_duration > 0.0 && drift > DURATION_TOLERANCE_SECONDS { Err("The joined video has the wrong length.".to_string()) } else { Ok(()) }
    });
    if copied.is_err() {
        method = "reencode";
        join_by_encoding(&tools, &intro, &source, &joined, audio.is_some(), crf)?;
    }
    let destination = if replace_source {
        source.clone()
    } else {
        let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let extension = source.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        source.with_file_name(format!("{stem}_with_thumbnail{extension}"))
    };
    fs::rename(&joined, &destination).map_err(|e| format!("The video with the thumbnail could not replace {}: {e}", destination.display()))?;
    Ok(json!({
        "final_video_path": destination.display().to_string(),
        "intro_seconds": seconds,
        "method": method,
        "duration": length_of(&tools, &destination),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn folders(root: &Path) -> ComfyFolders {
        ComfyFolders { output: root.join("output").display().to_string(), input: root.join("input").display().to_string(), temp: root.join("temp").display().to_string() }
    }

    fn env_with_ffmpeg() -> Option<Env> {
        let env = Env { output: PathBuf::new(), roots: vec![], ffmpeg: String::new(), ffprobe: String::new(), luts: PathBuf::new(), overlay_dir: None };
        env.tools().ok().map(|_| env)
    }

    fn make_video(env: &Env, path: &Path, seconds: u32, audio: bool, size: &str, rate: u32) {
        let tools = env.tools().unwrap();
        let mut command = args(&["-y", "-f", "lavfi", "-i", &format!("testsrc2=s={size}:r={rate}:d={seconds}")]);
        if audio {
            command.extend(args(&["-f", "lavfi", "-i", &format!("sine=frequency=440:sample_rate=48000:duration={seconds}")]));
        }
        command.extend(args(&["-c:v", "libx264", "-pix_fmt", "yuv420p"]));
        command.extend(if audio { args(&["-c:a", "aac", "-shortest"]) } else { args(&["-an"]) });
        command.push(path.display().to_string());
        tools.ffmpeg_checked(&command, "test video").unwrap();
    }

    fn probe(env: &Env, path: &Path, entries: &str, select: Option<&str>) -> Vec<String> {
        let tools = env.tools().unwrap();
        let mut command = args(&["-v", "error"]);
        if let Some(stream) = select {
            command.extend(args(&["-select_streams", stream]));
        }
        command.extend(args(&["-show_entries", entries, "-of", "default=noprint_wrappers=1:nokey=1", &path.display().to_string()]));
        let out = tools.ffprobe_checked(&command, "probe").unwrap();
        String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::to_string).collect()
    }

    fn picture(path: &Path, color: [u8; 3], size: (u32, u32)) {
        image::RgbImage::from_pixel(size.0, size.1, image::Rgb(color)).save(path).unwrap();
    }

    fn first_color(env: &Env, video: &Path, at: &str) -> [u8; 3] {
        let tools = env.tools().unwrap();
        let frame = video.with_extension("frame.png");
        tools.ffmpeg_checked(&args(&["-y", "-ss", at, "-i", &video.display().to_string(), "-frames:v", "1", &frame.display().to_string()]), "frame").unwrap();
        let small = image::imageops::resize(&image::open(&frame).unwrap().to_rgb8(), 1, 1, image::imageops::FilterType::Triangle);
        small.get_pixel(0, 0).0
    }

    #[test]
    fn the_length_is_kept_in_range() {
        assert_eq!(intro_seconds(Some(&json!(0))), MIN_INTRO_SECONDS);
        assert_eq!(intro_seconds(Some(&json!(99))), MAX_INTRO_SECONDS);
        assert_eq!(intro_seconds(Some(&json!("3.5"))), 3.5);
        for bad in [None, Some(json!("")), Some(json!("abc")), Some(Value::Null)] {
            assert_eq!(intro_seconds(bad.as_ref()), DEFAULT_INTRO_SECONDS);
        }
    }

    #[test]
    fn each_save_gets_its_own_numbered_png() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        picture(&source, [220, 30, 30], (640, 360));
        let payload = obj(json!({"project_folder": dir.path().display().to_string(), "source_path": source.display().to_string()}));
        let first = save_thumbnail_image(&folders(dir.path()), &payload).unwrap();
        let second = save_thumbnail_image(&folders(dir.path()), &payload).unwrap();
        assert!(first["image_path"].as_str().unwrap().ends_with("thumbnail_0001.png"));
        assert!(second["image_path"].as_str().unwrap().ends_with("thumbnail_0002.png"));
        assert_eq!((first["width"].clone(), first["height"].clone()), (json!(640), json!(360)));
        assert!(Path::new(first["image_path"].as_str().unwrap()).parent().unwrap().ends_with("thumbnail"));
    }

    #[test]
    fn a_comfyui_image_entry_is_found_in_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(dir.path().join("output").join("sub")).unwrap();
        picture(&dir.path().join("output").join("sub").join("gen.png"), [10, 200, 10], (32, 18));
        let payload = obj(json!({"project_folder": project.display().to_string(), "image": {"filename": "gen.png", "subfolder": "sub", "type": "output"}}));
        let saved = save_thumbnail_image(&folders(dir.path()), &payload).unwrap();
        assert_eq!(saved["width"], 32);
        let escape = obj(json!({"project_folder": project.display().to_string(), "image": {"filename": "gen.png", "subfolder": "..", "type": "output"}}));
        assert!(save_thumbnail_image(&folders(dir.path()), &escape).is_err());
    }

    #[test]
    fn a_missing_or_non_image_file_and_an_empty_project_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().display().to_string();
        assert!(save_thumbnail_image(&folders(dir.path()), &obj(json!({"project_folder": project, "source_path": dir.path().join("nope.png").display().to_string()}))).is_err());
        let text = dir.path().join("notes.txt");
        fs::write(&text, "not an image").unwrap();
        assert!(save_thumbnail_image(&folders(dir.path()), &obj(json!({"project_folder": project, "source_path": text.display().to_string()}))).is_err());
        let empty = save_thumbnail_image(&folders(dir.path()), &obj(json!({"project_folder": "", "source_path": "x.png"}))).unwrap_err();
        assert_eq!(empty, "Project folder is empty.");
        assert!(!std::env::current_dir().unwrap().join(THUMBNAIL_FOLDER).exists());
    }

    #[test]
    fn the_capabilities_answer_with_a_version() {
        assert_eq!(capabilities().unwrap()["version"], THUMBNAIL_VERSION);
    }

    #[test]
    fn the_picture_shows_first_and_the_video_follows_with_its_audio() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("FINAL_VIDEO.mp4");
        make_video(&env, &video, 3, true, "320x180", 24);
        let image = dir.path().join("thumb.png");
        picture(&image, [220, 30, 30], (640, 360));
        let reply = prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string(), "seconds": 2}))).unwrap();
        assert_eq!(reply["final_video_path"], video.display().to_string());
        assert!((reply["duration"].as_f64().unwrap() - 5.0).abs() < 0.4, "{reply}");
        assert_eq!(probe(&env, &video, "stream=width,height", Some("v:0")), ["320", "180"]);
        assert_eq!(probe(&env, &video, "stream=codec_name", Some("a:0")), ["aac"]);
        let [red, green, blue] = first_color(&env, &video, "1.0");
        assert!(red > 150 && green < 90 && blue < 90, "the intro is not the picture: {red} {green} {blue}");
        let later = first_color(&env, &video, "3.0");
        assert!(!(later[0] > 150 && later[1] < 90), "the scene should follow the intro");
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(".thumbnail_intro")).collect();
        assert!(left.is_empty(), "work folder left behind: {left:?}");
    }

    #[test]
    fn a_video_without_audio_stays_without_audio() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("silent.mp4");
        make_video(&env, &video, 3, false, "320x180", 24);
        let image = dir.path().join("thumb.png");
        picture(&image, [220, 30, 30], (640, 360));
        prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string(), "seconds": 1}))).unwrap();
        assert!(probe(&env, &video, "stream=codec_type", Some("a:0")).is_empty());
        assert!((probe(&env, &video, "format=duration", None)[0].parse::<f64>().unwrap() - 4.0).abs() < 0.4);
    }

    #[test]
    fn a_picture_of_another_shape_fills_the_frame_without_changing_the_video_size() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("wide.mp4");
        make_video(&env, &video, 3, true, "640x360", 30);
        let image = dir.path().join("tall.png");
        picture(&image, [220, 30, 30], (400, 800));
        prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string(), "seconds": 1}))).unwrap();
        assert_eq!(probe(&env, &video, "stream=width,height", Some("v:0")), ["640", "360"]);
        assert_eq!(probe(&env, &video, "stream=r_frame_rate", Some("v:0")), ["30/1"]);
    }

    #[test]
    fn replace_source_false_leaves_the_original() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("FINAL_VIDEO.mp4");
        make_video(&env, &video, 3, true, "320x180", 24);
        let image = dir.path().join("thumb.png");
        picture(&image, [220, 30, 30], (640, 360));
        let reply = prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string(), "seconds": 1, "replace_source": false}))).unwrap();
        assert!(reply["final_video_path"].as_str().unwrap().ends_with("FINAL_VIDEO_with_thumbnail.mp4"));
        assert!((probe(&env, &video, "format=duration", None)[0].parse::<f64>().unwrap() - 3.0).abs() < 0.3);
        assert!((reply["duration"].as_f64().unwrap() - 4.0).abs() < 0.4);
    }

    #[test]
    fn missing_files_are_refused_and_nothing_is_left_behind() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("FINAL_VIDEO.mp4");
        make_video(&env, &video, 2, true, "320x180", 24);
        let names = |dir: &Path| -> Vec<String> { let mut n: Vec<_> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(); n.sort(); n };
        let before = names(dir.path());
        let no_image = obj(json!({"input_path": video.display().to_string(), "image_path": dir.path().join("missing.png").display().to_string()}));
        assert!(prepend_thumbnail_intro(&env, &no_image).unwrap_err().starts_with("The thumbnail image was not found"));
        let no_video = obj(json!({"input_path": dir.path().join("x.mp4").display().to_string(), "image_path": video.display().to_string()}));
        assert!(prepend_thumbnail_intro(&env, &no_video).unwrap_err().starts_with("The video to put the thumbnail in front of was not found"));
        assert_eq!(names(dir.path()), before);
    }
}
