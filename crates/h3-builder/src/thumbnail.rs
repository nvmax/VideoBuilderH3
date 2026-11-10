//! Thumbnail intro: keep a generated thumbnail in the project and put it in front of the stitched video. Port of
//! `builder/thumbnail.py`.
//!
//! The Builder's Thumbnail dialog makes the picture (the LLM writes the edit prompt, NanoBanana or Flux/Klein draws it) and
//! calls [`save_thumbnail_image`] to keep it in `<project>/thumbnail`. After the final stitch the Builder calls
//! [`prepend_thumbnail_intro`], which makes the picture the video's first frame: exactly one frame in front of the first scene,
//! with the same length of silence. The scene clips, the timeline and the audio sync are never touched: only the stitched video
//! is rewritten.

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
pub(crate) const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "m4v"];

/// `GET /vrgdg/music_builder/thumbnail/capabilities`: tells the Builder that this build can save thumbnails, prepend the intro, take
/// one exact frame of a video, draw it with the image API of the LLM Runner's provider (`api_image`: the providers and their image models,
/// see `thumbnail_api_image.rs`), make it with Flow, GPT Image or Meta AI in the person's Chrome (`browser_ai`, and `browser_ai_direct`
/// because the app drives Chrome itself, see `thumbnail_browser_ai.rs`, where the pack runs a ComfyUI workflow) and join the person's own
/// intro video (`video_intro`, see `video_intro.rs`). It has no `overlay`, so the dialog does not offer that engine here.
pub fn capabilities() -> Reply {
    Ok(json!({"version": THUMBNAIL_VERSION, "frame": true, "api_image": h3_llm::api_image::capabilities(), "browser_ai": true, "browser_ai_direct": true, "video_intro": true}))
}

pub(crate) fn absolute(raw: &str) -> PathBuf {
    h3_core::abspath::absolute(raw).unwrap_or_else(|_| PathBuf::from(raw)).components().collect()
}

/// `_next_thumbnail_path`: `<prefix>_NNNN.png` with the first number that is free in `folder`.
pub(crate) fn next_thumbnail_path(folder: &Path, prefix: &str) -> PathBuf {
    let mut number = 1;
    while folder.join(format!("{prefix}_{number:04}.png")).exists() {
        number += 1;
    }
    folder.join(format!("{prefix}_{number:04}.png"))
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
    let target = next_thumbnail_path(&folder, "thumbnail");
    picture.to_rgb8().save_with_format(&target, ImageFormat::Png).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(json!({"image_path": target.display().to_string(), "width": picture.width(), "height": picture.height()}))
}

/// `POST /vrgdg/music_builder/thumbnail/frame`: `{project_folder, video_path, seconds}`. Takes one exact frame of a video as
/// `<project>/thumbnail/source_NNNN.png`, the picture the thumbnail is made from. `seconds` is the moment in `video_path`. It is snapped
/// to the nearest frame, and the seek lands half a frame before that frame's time, so the frame the Builder showed at that moment is the
/// one that is saved, not its neighbor. A moment past the end gives the last frame.
pub fn extract_thumbnail_frame(env: &Env, payload: &Obj) -> Reply {
    let project_text = path_text(payload, "project_folder");
    if project_text.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let video = absolute(&path_text(payload, "video_path"));
    let supported = video.extension().is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()));
    if !video.is_file() || !supported {
        return Err(format!("The video to take the frame from was not found: {}", video.display()));
    }
    let Some(seconds) = payload.get("seconds").and_then(to_float) else {
        return Err("Say which moment of the video to use (seconds).".into());
    };
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("The moment in the video must be 0 seconds or more.".into());
    }

    let tools = env.tools()?;
    let fields = probe_stream_fields(&tools, &video, "v:0", &["width", "height", "r_frame_rate", "avg_frame_rate"], false).map_err(|e| e.to_string())?;
    let rate = frame_rate(&fields);
    let fps = rate.as_f64();
    let duration = length_of(&tools, &video);
    let mut frame = (seconds * fps + 0.5).floor() as i64;
    if duration > 0.0 {
        frame = frame.min((((duration * fps + 0.5).floor() as i64) - 1).max(0));
    }
    let start = if frame > 0 { ((frame as f64 - 0.5) / fps).max(0.0) } else { 0.0 };

    let folder = absolute(&project_text).join(THUMBNAIL_FOLDER);
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let target = next_thumbnail_path(&folder, "source");
    let command = args(&["-y", "-ss", &format!("{start:.6}"), "-i", &video.display().to_string(), "-frames:v", "1", "-an", &target.display().to_string()]);
    let taken = tools
        .ffmpeg_checked(&command, "FFmpeg could not take the frame.")
        .map_err(|e| e.to_string())
        .and_then(|_| {
            if fs::metadata(&target).map(|m| m.len()).unwrap_or(0) == 0 {
                return Err("FFmpeg did not find a frame at that moment of the video.".to_string());
            }
            image::image_dimensions(&target).map_err(|e| format!("Could not read the picture {}: {e}", target.display()))
        });
    match taken {
        Ok((width, height)) => Ok(json!({
            "image_path": target.display().to_string(), "width": width, "height": height,
            "frame": frame, "seconds": frame as f64 / fps, "fps": fps,
        })),
        Err(message) => {
            let _ = fs::remove_file(&target);
            Err(message)
        }
    }
}

/// A frame rate as ffmpeg reads it: `24` or `30000/1001`.
pub(crate) fn rate_text(rate: Ratio) -> String {
    if rate.den == 1 { rate.num.to_string() } else { format!("{}/{}", rate.num, rate.den) }
}

pub(crate) fn frame_rate(values: &std::collections::HashMap<String, String>) -> Ratio {
    ["r_frame_rate", "avg_frame_rate"].iter().filter_map(|k| values.get(*k).and_then(|v| Ratio::parse(v))).find(Ratio::is_positive).unwrap_or(Ratio::new(24, 1))
}

/// The ffmpeg arguments that turn the picture into a one frame clip (with a matching silence) in the video's size, frame rate and audio
/// format. `seconds` is the length of that one frame (1 / frame rate), and `-frames:v 1` makes it exactly one frame.
fn intro_command(image: &Path, output: &Path, seconds: f64, size: (i64, i64), rate: Ratio, audio: Option<(i64, i64)>, crf: Option<f64>) -> Vec<String> {
    let (width, height) = size;
    let fill = format!("scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1,format=yuv420p");
    let length = format!("{seconds:.6}");
    let rate = rate_text(rate);
    let mut command = args(&["-y", "-loop", "1", "-framerate", &rate, "-t", &length, "-i", &image.display().to_string()]);
    if let Some((sample_rate, channels)) = audio {
        let layout = if channels == 1 { "mono" } else { "stereo" };
        command.extend(args(&["-f", "lavfi", "-t", &length, "-i", &format!("anullsrc=r={sample_rate}:cl={layout}")]));
    }
    command.extend(args(&["-vf", &fill, "-r", &rate, "-frames:v", "1"]));
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
pub(crate) fn join_by_copy(tools: &Tools, intro: &Path, source: &Path, output: &Path, work: &Path, what: &str) -> Result<(), String> {
    let listing = work.join("join.txt");
    let mut lines = String::new();
    for path in [intro, source] {
        lines.push_str(&format!("file '{}'\n", path.display().to_string().replace('\\', "/").replace('\'', "'\\''")));
    }
    fs::write(&listing, lines).map_err(|e| e.to_string())?;
    let command = args(&["-y", "-f", "concat", "-safe", "0", "-i", &listing.display().to_string(), "-c", "copy", "-movflags", "+faststart", &output.display().to_string()]);
    tools.ffmpeg_checked(&command, &format!("FFmpeg could not join the {what} and the video.")).map(|_| ()).map_err(|e| e.to_string())
}

/// Join with the concat filter and one new encode, for the rare video the stream-copy join does not fit.
pub(crate) fn join_by_encoding(tools: &Tools, intro: &Path, source: &Path, output: &Path, has_audio: bool, crf: Option<f64>, what: &str) -> Result<(), String> {
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
    tools.ffmpeg_checked(&command, &format!("FFmpeg could not join the {what} and the video.")).map(|_| ()).map_err(|e| e.to_string())
}

/// A hidden work folder beside the video, removed when it goes out of scope.
pub(crate) struct WorkDir(pub(crate) PathBuf);

impl WorkDir {
    /// `prefix` names what the folder is for (`.thumbnail_intro_`, `.video_intro_`), so a leftover is easy to tell.
    pub(crate) fn new(beside: &Path, prefix: &str) -> Result<Self, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let path = beside.join(format!("{prefix}{}_{nanos}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// How many video frames a file holds, counted from its packets (no decoding, so it is quick even for a long video).
pub(crate) fn video_packets(tools: &Tools, path: &Path) -> i64 {
    probe_stream_fields(tools, path, "v:0", &["nb_read_packets"], true).ok().and_then(|f| whole_field(&f, "nb_read_packets")).unwrap_or(0)
}

pub(crate) fn length_of(tools: &Tools, path: &Path) -> f64 {
    probe_duration(tools, &path.display().to_string()).unwrap_or(0.0)
}

pub(crate) fn whole_field(values: &std::collections::HashMap<String, String>, key: &str) -> Option<i64> {
    values.get(key).and_then(|v| v.trim().parse().ok())
}

/// `POST /vrgdg/music_builder/thumbnail/prepend`: `{input_path, image_path, replace_source?, crf?}`. Makes `image_path` the first frame of
/// `input_path` (a stitched video): one frame in front of it, in place unless `replace_source` is false.
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
    let replace_source = payload.get("replace_source") != Some(&Value::Bool(false));

    let tools = env.tools()?;
    let video = probe_stream_fields(&tools, &source, "v:0", &["width", "height", "r_frame_rate", "avg_frame_rate"], false).map_err(|e| e.to_string())?;
    let (Some(width), Some(height)) = (whole_field(&video, "width").filter(|w| *w > 0), whole_field(&video, "height").filter(|h| *h > 0)) else {
        return Err(format!("Could not read the size of the video: {}", source.display()));
    };
    let audio_fields = probe_stream_fields(&tools, &source, "a:0", &["sample_rate", "channels"], false).map_err(|e| e.to_string())?;
    let audio = whole_field(&audio_fields, "sample_rate").map(|rate| (rate, whole_field(&audio_fields, "channels").unwrap_or(2).max(1)));
    let rate = frame_rate(&video);
    let seconds = 1.0 / rate.as_f64();
    let crf = payload.get("crf").and_then(to_float).or_else(|| source_crf(&[&source]));
    let source_frames = video_packets(&tools, &source);

    let folder = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let work = WorkDir::new(&folder, ".thumbnail_intro_")?;
    let intro = work.0.join("intro.mp4");
    let command = intro_command(&image_path, &intro, seconds, (width, height), rate, audio, crf);
    tools.ffmpeg_checked(&command, "FFmpeg could not make the thumbnail clip.").map_err(|e| e.to_string())?;
    let joined = work.0.join("joined.mp4");
    let mut method = "copy";
    // One frame is far shorter than a length check could tell, so the frames are counted: exactly one more than before.
    let copied = join_by_copy(&tools, &intro, &source, &joined, &work.0, "thumbnail").and_then(|_| {
        if video_packets(&tools, &joined) == source_frames + 1 { Ok(()) } else { Err("The joined video does not have exactly one more frame.".to_string()) }
    });
    if copied.is_err() {
        method = "reencode";
        join_by_encoding(&tools, &intro, &source, &joined, audio.is_some(), crf, "thumbnail")?;
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
        "intro_frames": 1,
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

    /// Average color of frame `number` (0 is the first frame).
    fn frame_color(env: &Env, video: &Path, number: u32) -> [u8; 3] {
        let tools = env.tools().unwrap();
        let frame = video.with_extension(format!("frame{number}.png"));
        let filter = format!("select=eq(n\\,{number})");
        tools.ffmpeg_checked(&args(&["-y", "-i", &video.display().to_string(), "-vf", &filter, "-frames:v", "1", "-fps_mode", "passthrough", &frame.display().to_string()]), "frame").unwrap();
        let small = image::imageops::resize(&image::open(&frame).unwrap().to_rgb8(), 1, 1, image::imageops::FilterType::Triangle);
        small.get_pixel(0, 0).0
    }

    fn frame_count(env: &Env, video: &Path) -> i64 {
        probe_stream_fields(&env.tools().unwrap(), video, "v:0", &["nb_read_packets"], true).unwrap()["nb_read_packets"].parse().unwrap()
    }

    fn is_red(color: [u8; 3]) -> bool {
        color[0] > 150 && color[1] < 90 && color[2] < 90
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
    fn the_capabilities_answer_with_a_version_the_frame_picker_the_api_image_providers_browser_ai_and_the_video_intro() {
        let answer = capabilities().unwrap();
        assert_eq!((answer["version"].clone(), answer["frame"].clone(), answer["video_intro"].clone()), (json!(THUMBNAIL_VERSION), json!(true), json!(true)));
        // No overlay: the dialog leaves that engine out. Browser AI is offered, and the app runs it itself (no ComfyUI workflow, no Playwright).
        assert!(answer.get("overlay").is_none());
        assert_eq!((answer["browser_ai"].clone(), answer["browser_ai_direct"].clone()), (json!(true), json!(true)));
        let providers = answer["api_image"]["providers"].as_object().unwrap();
        assert_eq!(providers.keys().cloned().collect::<Vec<_>>(), ["gemini", "grok", "openai"]);
        assert_eq!(providers["gemini"]["default"], "gemini-3-pro-image-preview");
    }

    /// One second of red, then one second of blue: frames 0 to 23 are red and 24 to 47 are blue at 24 fps.
    fn make_two_color_video(env: &Env, path: &Path) {
        let tools = env.tools().unwrap();
        let command = args(&[
            "-y", "-f", "lavfi", "-i", "color=c=red:s=160x90:r=24:d=1", "-f", "lavfi", "-i", "color=c=blue:s=160x90:r=24:d=1",
            "-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0[v]", "-map", "[v]", "-c:v", "libx264", "-pix_fmt", "yuv420p", &path.display().to_string(),
        ]);
        tools.ffmpeg_checked(&command, "two color video").unwrap();
    }

    fn dominant(path: &str) -> &'static str {
        let small = image::imageops::resize(&image::open(path).unwrap().to_rgb8(), 1, 1, image::imageops::FilterType::Triangle);
        let [red, _, blue] = small.get_pixel(0, 0).0;
        if red > 150 && blue < 100 { "red" } else if blue > 150 && red < 100 { "blue" } else { "other" }
    }

    fn take(env: &Env, project: &Path, video: &Path, seconds: Value) -> Reply {
        extract_thumbnail_frame(env, &obj(json!({"project_folder": project.display().to_string(), "video_path": video.display().to_string(), "seconds": seconds})))
    }

    #[test]
    fn the_frame_at_a_moment_is_the_frame_the_player_shows_there() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("scene.mp4");
        make_two_color_video(&env, &video);
        // Frame 23 is the last red one, frame 24 the first blue one: a cut that lands exactly on a frame edge.
        let before = take(&env, dir.path(), &video, json!(23.0 / 24.0)).unwrap();
        let after = take(&env, dir.path(), &video, json!(1.0)).unwrap();
        assert_eq!((before["frame"].clone(), dominant(before["image_path"].as_str().unwrap())), (json!(23), "red"));
        assert_eq!((after["frame"].clone(), dominant(after["image_path"].as_str().unwrap())), (json!(24), "blue"));
        for (seconds, color) in [(0.0, "red"), (0.5, "red"), (1.5, "blue")] {
            let reply = take(&env, dir.path(), &video, json!(seconds)).unwrap();
            assert_eq!(dominant(reply["image_path"].as_str().unwrap()), color, "at {seconds} s");
        }
    }

    #[test]
    fn a_moment_between_frames_snaps_to_the_nearest_frame_and_past_the_end_gives_the_last() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("scene.mp4");
        make_two_color_video(&env, &video);
        let snapped = take(&env, dir.path(), &video, json!(0.9999)).unwrap();
        assert_eq!(snapped["frame"], 24);
        assert!((snapped["seconds"].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(snapped["fps"], 24.0);
        assert_eq!((snapped["width"].clone(), snapped["height"].clone()), (json!(160), json!(90)));
        let last = take(&env, dir.path(), &video, json!(99)).unwrap();
        assert_eq!(last["frame"], 47);
        assert_eq!(dominant(last["image_path"].as_str().unwrap()), "blue");
    }

    #[test]
    fn each_frame_gets_its_own_numbered_file_in_the_thumbnail_folder() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("scene.mp4");
        make_two_color_video(&env, &video);
        let names: Vec<String> = (0..2).map(|_| Path::new(take(&env, dir.path(), &video, json!(0.1)).unwrap()["image_path"].as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["source_0001.png", "source_0002.png"]);
        let mut listed: Vec<String> = fs::read_dir(dir.path().join(THUMBNAIL_FOLDER)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        listed.sort();
        assert_eq!(listed, names);
    }

    #[test]
    fn bad_frame_requests_are_refused_and_leave_nothing_behind() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("scene.mp4");
        make_two_color_video(&env, &video);
        assert_eq!(extract_thumbnail_frame(&env, &obj(json!({"project_folder": "", "video_path": video.display().to_string(), "seconds": 0}))).unwrap_err(), "Project folder is empty.");
        assert!(take(&env, dir.path(), &dir.path().join("missing.mp4"), json!(0)).unwrap_err().starts_with("The video to take the frame from was not found"));
        for bad in [json!(-1), json!("abc"), Value::Null] {
            assert!(take(&env, dir.path(), &video, bad.clone()).is_err(), "{bad} should be refused");
        }
        assert!(!dir.path().join(THUMBNAIL_FOLDER).exists());
    }

    #[test]
    fn the_picture_is_exactly_the_first_frame_and_the_video_follows_unchanged() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("FINAL_VIDEO.mp4");
        make_video(&env, &video, 3, true, "320x180", 24);
        let before = frame_count(&env, &video);
        assert_eq!(before, 72);
        let image = dir.path().join("thumb.png");
        picture(&image, [220, 30, 30], (640, 360));
        let reply = prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string()}))).unwrap();
        assert_eq!(reply["final_video_path"], video.display().to_string());
        assert_eq!(reply["intro_frames"], 1);
        assert!((reply["intro_seconds"].as_f64().unwrap() - 1.0 / 24.0).abs() < 1e-6);
        // One frame more, no more: the picture is frame 0, and the old first frame is now frame 1.
        assert_eq!(frame_count(&env, &video), before + 1);
        assert!(is_red(frame_color(&env, &video, 0)), "frame 0 is the picture");
        assert!(!is_red(frame_color(&env, &video, 1)), "frame 1 is the video's own first frame");
        assert!(!is_red(frame_color(&env, &video, 2)));
        assert_eq!(probe(&env, &video, "stream=width,height", Some("v:0")), ["320", "180"]);
        assert_eq!(probe(&env, &video, "stream=codec_name", Some("a:0")), ["aac"]);
        // The audio is not pushed by more than the one frame: the file is 1/24 s longer, within an audio frame.
        assert!((probe(&env, &video, "format=duration", None)[0].parse::<f64>().unwrap() - (3.0 + 1.0 / 24.0)).abs() < 0.06);
        assert!((reply["duration"].as_f64().unwrap() - (3.0 + 1.0 / 24.0)).abs() < 0.06, "{reply}");
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(".thumbnail_intro")).collect();
        assert!(left.is_empty(), "work folder left behind: {left:?}");
    }

    #[test]
    fn a_length_in_the_request_does_not_make_the_picture_stay_longer() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("FINAL_VIDEO.mp4");
        make_video(&env, &video, 3, true, "320x180", 24);
        let image = dir.path().join("thumb.png");
        picture(&image, [220, 30, 30], (640, 360));
        let reply = prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string(), "seconds": 5}))).unwrap();
        assert_eq!(frame_count(&env, &video), 73);
        assert_eq!(reply["intro_frames"], 1);
    }

    #[test]
    fn a_video_without_audio_gets_no_audio() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("silent.mp4");
        make_video(&env, &video, 3, false, "320x180", 24);
        let image = dir.path().join("thumb.png");
        picture(&image, [220, 30, 30], (640, 360));
        prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string()}))).unwrap();
        assert!(probe(&env, &video, "stream=codec_type", Some("a:0")).is_empty());
        assert_eq!(frame_count(&env, &video), 73);
    }

    #[test]
    fn a_picture_of_another_shape_fills_the_frame_without_changing_the_video_size_or_rate() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("wide.mp4");
        make_video(&env, &video, 3, true, "640x360", 30);
        let image = dir.path().join("tall.png");
        picture(&image, [220, 30, 30], (400, 800));
        let reply = prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string()}))).unwrap();
        assert_eq!(probe(&env, &video, "stream=width,height", Some("v:0")), ["640", "360"]);
        assert_eq!(probe(&env, &video, "stream=r_frame_rate", Some("v:0")), ["30/1"]);
        assert_eq!(frame_count(&env, &video), 91, "3 s at 30 fps plus one frame");
        assert!((reply["intro_seconds"].as_f64().unwrap() - 1.0 / 30.0).abs() < 1e-6);
    }

    #[test]
    fn replace_source_false_leaves_the_original() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("FINAL_VIDEO.mp4");
        make_video(&env, &video, 3, true, "320x180", 24);
        let image = dir.path().join("thumb.png");
        picture(&image, [220, 30, 30], (640, 360));
        let reply = prepend_thumbnail_intro(&env, &obj(json!({"input_path": video.display().to_string(), "image_path": image.display().to_string(), "replace_source": false}))).unwrap();
        let made = PathBuf::from(reply["final_video_path"].as_str().unwrap());
        assert!(made.to_string_lossy().ends_with("FINAL_VIDEO_with_thumbnail.mp4"));
        assert_eq!(frame_count(&env, &video), 72);
        assert_eq!(frame_count(&env, &made), 73);
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
