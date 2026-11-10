//! Video intro: put the person's own video in front of the stitched video. Port of `builder/video_intro.py`.
//!
//! The Thumbnail dialog lets the person choose a video file they already have (a logo sting, a title card, an artist intro). After the
//! final stitch the Builder calls [`prepend_video_intro`], which makes that clip match the stitched video (size, frame rate, pixel format,
//! audio format) and joins it in front of the first scene. The whole clip is kept: it is fitted inside the frame with black bars when its
//! shape differs, never cropped, and its own sound plays (silence when it has none). The scene clips, the timeline and the audio sync are
//! never touched: only the stitched video is rewritten, so stitching again gives the plain video.
//!
//! When the Thumbnail intro is on too, the Builder joins this clip first and the thumbnail picture second, so the picture is the very
//! first frame of the finished video and the intro follows it.

use std::fs;
use std::path::Path;

use h3_core::pyval::{to_float, Obj};
use h3_media::tools::{args, scene_audio_args, scene_video_args, source_crf};
use h3_media::video::{probe_stream_fields, Ratio};
use serde_json::{json, Value};

use crate::payload::path_text;
use crate::thumbnail::{absolute, frame_rate, join_by_copy, join_by_encoding, length_of, rate_text, video_packets, whole_field, WorkDir, VIDEO_EXTENSIONS};
use crate::Env;

type Reply = Result<Value, String>;

/// The ffmpeg arguments that turn the intro into a clip in the stitched video's size, frame rate and audio format. The picture is fitted
/// inside the frame (black bars, no cropping). `seconds` is the intro's own length: `-t` and `apad` make the sound exactly that long, so
/// the join keeps the video and the sound together. With `audio` set the clip always has a sound track (the intro's own, or silence), so
/// the stream layout equals the stitched video's.
#[allow(clippy::too_many_arguments)]
fn intro_clip_command(intro: &Path, output: &Path, seconds: f64, size: (i64, i64), rate: Ratio, intro_has_audio: bool, audio: Option<(i64, i64)>, crf: Option<f64>) -> Vec<String> {
    let (width, height) = size;
    let rate = rate_text(rate);
    let fit = format!("fps={rate},scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,format=yuv420p");
    let length = format!("{seconds:.6}");
    let mut command = args(&["-y", "-i", &intro.display().to_string()]);
    if let Some((sample_rate, channels)) = audio.filter(|_| !intro_has_audio) {
        let layout = if channels == 1 { "mono" } else { "stereo" };
        command.extend(args(&["-f", "lavfi", "-t", &length, "-i", &format!("anullsrc=r={sample_rate}:cl={layout}")]));
    }
    command.extend(args(&["-map", "0:v:0"]));
    if audio.is_some() {
        command.extend(args(&["-map", if intro_has_audio { "0:a:0" } else { "1:a:0" }]));
    }
    command.extend(args(&["-vf", &fit, "-r", &rate]));
    command.extend(scene_video_args(crf));
    match audio {
        Some((sample_rate, channels)) => {
            command.extend(scene_audio_args());
            command.extend(args(&["-ar", &sample_rate.to_string(), "-ac", &channels.to_string(), "-af", "apad"]));
        }
        None => command.push("-an".into()),
    }
    command.extend(args(&["-t", &length]));
    command.push(output.display().to_string());
    command
}

fn is_video_file(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// `POST /vrgdg/music_builder/video_intro/prepend`: `{input_path, intro_path, replace_source?, crf?}`. Makes `intro_path` the start of
/// `input_path` (a stitched video): joined in front of it, in place unless `replace_source` is false.
pub fn prepend_video_intro(env: &Env, payload: &Obj) -> Reply {
    let source = absolute(&path_text(payload, "input_path"));
    let intro = absolute(&path_text(payload, "intro_path"));
    if !is_video_file(&source) {
        return Err(format!("The video to put the intro in front of was not found: {}", source.display()));
    }
    if !is_video_file(&intro) {
        return Err(format!("The intro video was not found: {}", intro.display()));
    }
    if matches!((fs::canonicalize(&source), fs::canonicalize(&intro)), (Ok(a), Ok(b)) if a == b) {
        return Err("The intro video is the final video itself. Choose a different file.".into());
    }
    let replace_source = payload.get("replace_source") != Some(&Value::Bool(false));

    let tools = env.tools()?;
    let video = probe_stream_fields(&tools, &source, "v:0", &["width", "height", "r_frame_rate", "avg_frame_rate"], false).map_err(|e| e.to_string())?;
    let (Some(width), Some(height)) = (whole_field(&video, "width").filter(|w| *w > 0), whole_field(&video, "height").filter(|h| *h > 0)) else {
        return Err(format!("Could not read the size of the video: {}", source.display()));
    };
    let intro_picture = probe_stream_fields(&tools, &intro, "v:0", &["width"], false).map_err(|e| e.to_string())?;
    if whole_field(&intro_picture, "width").is_none() {
        return Err(format!("The intro file has no video picture: {}", intro.display()));
    }
    let audio_fields = probe_stream_fields(&tools, &source, "a:0", &["sample_rate", "channels"], false).map_err(|e| e.to_string())?;
    let audio = whole_field(&audio_fields, "sample_rate").map(|rate| (rate, whole_field(&audio_fields, "channels").unwrap_or(2).max(1)));
    let intro_audio = probe_stream_fields(&tools, &intro, "a:0", &["sample_rate"], false).map_err(|e| e.to_string())?;
    let intro_has_audio = whole_field(&intro_audio, "sample_rate").is_some();
    let seconds = length_of(&tools, &intro);
    if seconds <= 0.0 {
        return Err(format!("Could not read the length of the intro video: {}", intro.display()));
    }
    let rate = frame_rate(&video);
    let crf = payload.get("crf").and_then(to_float).or_else(|| source_crf(&[&source]));
    let source_frames = video_packets(&tools, &source);

    let folder = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let work = WorkDir::new(&folder, ".video_intro_")?;
    let clip = work.0.join("intro.mp4");
    let command = intro_clip_command(&intro, &clip, seconds, (width, height), rate, intro_has_audio, audio, crf);
    tools.ffmpeg_checked(&command, "FFmpeg could not make the intro clip.").map_err(|e| e.to_string())?;
    let intro_frames = video_packets(&tools, &clip);
    if intro_frames < 1 {
        return Err(format!("The intro video has no frames: {}", intro.display()));
    }
    let joined = work.0.join("joined.mp4");
    let mut method = "copy";
    // The frames are counted, so a join that dropped or repeated one is noticed and redone with a new encode.
    let copied = join_by_copy(&tools, &clip, &source, &joined, &work.0, "intro").and_then(|_| {
        if video_packets(&tools, &joined) == source_frames + intro_frames { Ok(()) } else { Err("The joined video does not have the intro's frames plus the video's frames.".to_string()) }
    });
    if copied.is_err() {
        method = "reencode";
        join_by_encoding(&tools, &clip, &source, &joined, audio.is_some(), crf, "intro")?;
    }
    let destination = if replace_source {
        source.clone()
    } else {
        let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let extension = source.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        source.with_file_name(format!("{stem}_with_intro{extension}"))
    };
    fs::rename(&joined, &destination).map_err(|e| format!("The video with the intro could not replace {}: {e}", destination.display()))?;
    Ok(json!({
        "final_video_path": destination.display().to_string(),
        "intro_frames": intro_frames,
        "intro_seconds": intro_frames as f64 / rate.as_f64(),
        "method": method,
        "duration": length_of(&tools, &destination),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn env_with_ffmpeg() -> Option<Env> {
        let env = Env { output: PathBuf::new(), roots: vec![], ffmpeg: String::new(), ffprobe: String::new(), luts: PathBuf::new(), overlay_dir: None };
        env.tools().ok().map(|_| env)
    }

    /// A video of a lavfi `source` (`testsrc2` is the test pattern, `color=c=red` a solid color), with a sine tone when `audio` is set.
    fn make_video(env: &Env, path: &Path, source: &str, size: &str, rate: u32, seconds: u32, audio: bool) {
        let tools = env.tools().unwrap();
        let spec = if source.contains('=') { format!("{source}:s={size}:r={rate}:d={seconds}") } else { format!("{source}=s={size}:r={rate}:d={seconds}") };
        let mut command = args(&["-y", "-f", "lavfi", "-i", &spec]);
        if audio {
            command.extend(args(&["-f", "lavfi", "-i", &format!("sine=frequency=440:sample_rate=48000:duration={seconds}")]));
        }
        command.extend(args(&["-c:v", "libx264", "-pix_fmt", "yuv420p"]));
        command.extend(if audio { args(&["-c:a", "aac", "-shortest"]) } else { args(&["-an"]) });
        command.push(path.display().to_string());
        tools.ffmpeg_checked(&command, "test video").unwrap();
    }

    fn video(env: &Env, path: &Path, seconds: u32, audio: bool) {
        make_video(env, path, "testsrc2", "320x180", 24, seconds, audio);
    }

    fn red(env: &Env, path: &Path, seconds: u32, audio: bool, size: &str, rate: u32) {
        make_video(env, path, "color=c=red", size, rate, seconds, audio);
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

    fn frame_count(env: &Env, video: &Path) -> i64 {
        probe_stream_fields(&env.tools().unwrap(), video, "v:0", &["nb_read_packets"], true).unwrap()["nb_read_packets"].parse().unwrap()
    }

    /// The picture of frame `number` (0 is the first frame) of `video`.
    fn frame(env: &Env, video: &Path, number: u32) -> image::RgbImage {
        let tools = env.tools().unwrap();
        let target = video.with_extension(format!("frame{number}.png"));
        let filter = format!("select=eq(n\\,{number})");
        tools.ffmpeg_checked(&args(&["-y", "-i", &video.display().to_string(), "-vf", &filter, "-frames:v", "1", "-fps_mode", "passthrough", &target.display().to_string()]), "frame").unwrap();
        image::open(&target).unwrap().to_rgb8()
    }

    /// Average color of a frame.
    fn frame_color(env: &Env, video: &Path, number: u32) -> [u8; 3] {
        image::imageops::resize(&frame(env, video, number), 1, 1, image::imageops::FilterType::Triangle).get_pixel(0, 0).0
    }

    fn is_red(color: [u8; 3]) -> bool {
        color[0] > 150 && color[1] < 90 && color[2] < 90
    }

    fn prepend(env: &Env, source: &Path, intro: &Path, extra: Value) -> Reply {
        let mut payload = obj(json!({"input_path": source.display().to_string(), "intro_path": intro.display().to_string()}));
        payload.extend(extra.as_object().cloned().unwrap_or_default());
        prepend_video_intro(env, &payload)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        found.sort();
        found
    }

    #[test]
    fn the_intro_plays_first_and_the_video_follows_unchanged() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let final_video = dir.path().join("FINAL_VIDEO.mp4");
        let intro = dir.path().join("intro.mp4");
        video(&env, &final_video, 3, true);
        red(&env, &intro, 2, true, "160x90", 24);
        assert_eq!(frame_count(&env, &final_video), 72);
        let reply = prepend(&env, &final_video, &intro, json!({})).unwrap();
        assert_eq!(reply["final_video_path"], final_video.display().to_string());
        assert_eq!(reply["intro_frames"], 48);
        assert_eq!(reply["method"], "copy", "the clip was made to match, so no second encode of the video is needed");
        assert!((reply["intro_seconds"].as_f64().unwrap() - 2.0).abs() < 1e-3);
        assert_eq!(frame_count(&env, &final_video), 72 + 48);
        assert!(is_red(frame_color(&env, &final_video, 0)), "the first frame is the intro's");
        assert!(is_red(frame_color(&env, &final_video, 47)), "the last frame of the intro is still the intro");
        assert!(!is_red(frame_color(&env, &final_video, 48)), "the video's own first frame follows the intro");
        assert_eq!(probe(&env, &final_video, "stream=width,height", Some("v:0")), ["320", "180"]);
        assert_eq!(probe(&env, &final_video, "stream=codec_name", Some("a:0")), ["aac"]);
        assert!((probe(&env, &final_video, "format=duration", None)[0].parse::<f64>().unwrap() - 5.0).abs() < 0.08);
        assert!((reply["duration"].as_f64().unwrap() - 5.0).abs() < 0.08, "{reply}");
        let left: Vec<_> = names(dir.path()).into_iter().filter(|n| n.starts_with(".video_intro")).collect();
        assert!(left.is_empty(), "work folder left behind: {left:?}");
    }

    #[test]
    fn an_intro_of_another_shape_and_rate_is_fitted_without_cropping() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let final_video = dir.path().join("FINAL_VIDEO.mp4");
        let intro = dir.path().join("tall.mp4");
        make_video(&env, &final_video, "testsrc2", "640x360", 24, 3, true);
        // A tall 30 fps intro: it keeps its whole picture inside the wide frame, with bars on the sides.
        red(&env, &intro, 2, true, "180x320", 30);
        let reply = prepend(&env, &final_video, &intro, json!({})).unwrap();
        assert_eq!(probe(&env, &final_video, "stream=width,height", Some("v:0")), ["640", "360"]);
        assert_eq!(probe(&env, &final_video, "stream=r_frame_rate", Some("v:0")), ["24/1"]);
        assert_eq!(reply["intro_frames"], 48, "2 s at the video's 24 fps, not the intro's 30");
        assert_eq!(frame_count(&env, &final_video), 72 + 48);
        let picture = frame(&env, &final_video, 10);
        let left = picture.get_pixel(5, 180).0;
        assert!(u32::from(left[0]) + u32::from(left[1]) + u32::from(left[2]) < 60, "black bar at the side");
        assert!(is_red(picture.get_pixel(320, 180).0), "the picture in the middle");
    }

    #[test]
    fn an_intro_without_sound_gets_silence_so_the_sound_stays_with_the_picture() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let final_video = dir.path().join("FINAL_VIDEO.mp4");
        let intro = dir.path().join("silent.mp4");
        video(&env, &final_video, 3, true);
        red(&env, &intro, 2, false, "160x90", 24);
        prepend(&env, &final_video, &intro, json!({})).unwrap();
        assert_eq!(probe(&env, &final_video, "stream=codec_type", Some("a:0")), ["audio"]);
        assert!((probe(&env, &final_video, "stream=duration", Some("a:0"))[0].parse::<f64>().unwrap() - 5.0).abs() < 0.1);
        assert_eq!(frame_count(&env, &final_video), 72 + 48);
    }

    #[test]
    fn a_video_without_sound_gets_no_sound() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let final_video = dir.path().join("silent.mp4");
        let intro = dir.path().join("intro.mp4");
        video(&env, &final_video, 3, false);
        red(&env, &intro, 2, true, "160x90", 24);
        prepend(&env, &final_video, &intro, json!({})).unwrap();
        assert!(probe(&env, &final_video, "stream=codec_type", Some("a:0")).is_empty());
        assert_eq!(frame_count(&env, &final_video), 72 + 48);
    }

    #[test]
    fn replace_source_false_leaves_the_original() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let final_video = dir.path().join("FINAL_VIDEO.mp4");
        let intro = dir.path().join("intro.mp4");
        video(&env, &final_video, 3, true);
        red(&env, &intro, 2, true, "160x90", 24);
        let reply = prepend(&env, &final_video, &intro, json!({"replace_source": false})).unwrap();
        let made = PathBuf::from(reply["final_video_path"].as_str().unwrap());
        assert!(made.to_string_lossy().ends_with("FINAL_VIDEO_with_intro.mp4"));
        assert_eq!(frame_count(&env, &final_video), 72);
        assert_eq!(frame_count(&env, &made), 72 + 48);
    }

    #[test]
    fn missing_files_and_the_video_itself_are_refused_and_nothing_is_left_behind() {
        let Some(env) = env_with_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let final_video = dir.path().join("FINAL_VIDEO.mp4");
        let intro = dir.path().join("intro.mp4");
        video(&env, &final_video, 2, true);
        red(&env, &intro, 2, true, "160x90", 24);
        let notes = dir.path().join("notes.txt");
        fs::write(&notes, "not a video").unwrap();
        let fake = dir.path().join("fake.mp4");
        fs::write(&fake, "not a video").unwrap();
        let before = names(dir.path());
        assert!(prepend(&env, &final_video, &dir.path().join("missing.mp4"), json!({})).unwrap_err().starts_with("The intro video was not found"));
        assert!(prepend(&env, &dir.path().join("x.mp4"), &intro, json!({})).unwrap_err().starts_with("The video to put the intro in front of was not found"));
        assert!(prepend(&env, &final_video, &final_video, json!({})).unwrap_err().contains("the final video itself"));
        assert!(prepend(&env, &final_video, &notes, json!({})).unwrap_err().starts_with("The intro video was not found"), "a text file is not a video");
        assert!(prepend(&env, &final_video, &fake, json!({})).is_err(), "a file named .mp4 that is not one");
        assert_eq!(frame_count(&env, &final_video), 48, "the video is untouched after a refusal");
        assert_eq!(names(dir.path()), before);
    }

    #[test]
    fn the_intro_clip_is_fitted_and_given_a_sound_track_that_matches() {
        let rate = Ratio::new(30000, 1001);
        let with_sound = intro_clip_command(Path::new("in.mp4"), Path::new("out.mp4"), 2.0, (1280, 720), rate, true, Some((48000, 2)), None);
        let text = with_sound.join(" ");
        assert!(text.contains("fps=30000/1001,scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,format=yuv420p"), "{text}");
        assert!(text.contains("-map 0:v:0 -map 0:a:0"), "{text}");
        assert!(text.contains("-ar 48000 -ac 2 -af apad -t 2.000000 out.mp4"), "{text}");
        let silence = intro_clip_command(Path::new("in.mp4"), Path::new("out.mp4"), 2.0, (1280, 720), rate, false, Some((44100, 1)), None).join(" ");
        assert!(silence.contains("anullsrc=r=44100:cl=mono") && silence.contains("-map 0:v:0 -map 1:a:0"), "{silence}");
        let none = intro_clip_command(Path::new("in.mp4"), Path::new("out.mp4"), 2.0, (1280, 720), rate, true, None, None).join(" ");
        assert!(none.contains("-an") && !none.contains("-map 0:a:0"), "{none}");
    }
}
