//! The Post Process tab: the LUT list, one picture previews and full clip renders of LUT, film grain and Adjust. Ports of
//! the routes in `post_process/lut_video_tools.py`. The pack grades with torch, on the graphics card when there is one.
//! Here the pictures are graded on the CPU by `h3_media::grade` and clips go through ffmpeg.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use h3_core::pyval::{py_str, to_float, to_int, truthy, Obj};
use h3_media::effect::{render_effect, Encode};
use h3_media::grade::{apply_film_grain, clock_seed, Adjust, Cube, Rgb};
use h3_media::post::{apply_lut_to_video, normalize_encode_crf};
use h3_media::tools::{args, command};
use h3_media::video::video_size;
use h3_media::Tools;
use serde_json::{json, Value};

use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

const IMAGE_EXTENSIONS: [&str; 5] = [".png", ".jpg", ".jpeg", ".webp", ".bmp"];
const VIDEO_EXTENSIONS: [&str; 5] = [".mp4", ".mov", ".avi", ".webm", ".mkv"];

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// Letters, digits, `-` and `_` are kept, anything else becomes `_`, and `_` at the ends is dropped.
fn safe_token(value: &str, fallback: &str) -> String {
    let token: String = value.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let token = token.trim_matches('_');
    if token.is_empty() { fallback.to_string() } else { token.to_string() }
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `float(payload.get(key, default))`, with Python's text for a value that is not a number.
fn float_of(payload: &Obj, key: &str, default: f64) -> Result<f64, String> {
    match payload.get(key) {
        None => Ok(default),
        Some(value) => to_float(value).ok_or_else(|| match value {
            Value::String(s) => format!("could not convert string to float: '{s}'"),
            Value::Null => "float() argument must be a string or a real number, not 'NoneType'".to_string(),
            _ => "float() argument must be a string or a real number".to_string(),
        }),
    }
}

/// `bool(payload.get(key, default))`.
fn flag(payload: &Obj, key: &str, default: bool) -> bool {
    payload.get(key).map_or(default, truthy)
}

fn crf_of(payload: &Obj, absent: f64) -> i64 {
    let value = match payload.get("encode_crf") {
        None => absent,
        Some(v) => to_float(v).unwrap_or(23.0),
    };
    normalize_encode_crf(Some(value))
}

fn preset_of(payload: &Obj) -> String {
    h3_media::post::normalize_encode_preset(payload.get("encode_preset").and_then(Value::as_str).unwrap_or("medium"))
}

fn seed_of(payload: &Obj) -> Option<u64> {
    match payload.get("seed") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(v) => to_int(v).map(|n| n as u64),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// LUT files

/// Percent encoding as `urllib.parse.quote`.
fn quote(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn example_key(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// `GET /vrgdg/music_builder/luts`: the `.cube` files in the LUT folder, with an example picture when one is named like the LUT.
pub fn list_luts(env: &Env) -> Reply {
    // The folder is made so there is somewhere to drop LUT files.
    let _ = fs::create_dir_all(&env.luts);
    let examples = env.luts.join("examples");
    let mut reply = json!({"luts": [], "luts_dir": text(&env.luts), "examples_dir": text(&examples)});
    let Ok(entries) = fs::read_dir(&env.luts) else {
        return Ok(reply);
    };
    let mut examples_by_name: HashMap<String, String> = HashMap::new();
    for entry in fs::read_dir(&examples).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && IMAGE_EXTENSIONS.contains(&extension(Path::new(&name)).as_str()) {
            let base = stem(Path::new(&name));
            examples_by_name.insert(base.to_lowercase(), name.clone());
            examples_by_name.insert(example_key(&base), name);
        }
    }
    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_file() && extension(&e.path()) == ".cube")
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    files.sort_by_key(|(name, _)| name.to_lowercase());
    let luts: Vec<Value> = files
        .into_iter()
        .map(|(name, path)| {
            let base = stem(&path);
            let example = examples_by_name.get(&base.to_lowercase()).or_else(|| examples_by_name.get(&example_key(&base))).cloned().unwrap_or_default();
            let meta = fs::metadata(&path).ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64());
            json!({
                "name": name,
                "label": base.replace('_', " "),
                "path": text(&path),
                "example_name": example,
                "example_url": if example.is_empty() { String::new() } else { format!("/vrgdg/music_builder/luts/example?name={}", quote(&example)) },
                "size": meta.map_or(0, |m| m.len()),
                "modified": modified,
            })
        })
        .collect();
    reply["luts"] = Value::Array(luts);
    Ok(reply)
}

/// `GET /vrgdg/music_builder/luts/example`: the example picture file.
pub fn lut_example_path(env: &Env, name: &str) -> Result<PathBuf, String> {
    let base = Path::new(name.trim().trim_matches('"')).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if base.is_empty() {
        return Err("Example image name is required.".into());
    }
    let path = env.luts.join("examples").join(&base);
    if !path.is_file() {
        return Err(format!("LUT example image was not found: {base}"));
    }
    Ok(path)
}

/// The `.cube` file a LUT name points to, kept inside the LUT folder.
fn lut_path(env: &Env, lut_name: &str) -> Result<PathBuf, String> {
    let name = Path::new(lut_name.trim().trim_matches('"')).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() {
        return Err("LUT name is required.".into());
    }
    let path = env.luts.join(&name);
    if !path.is_file() {
        return Err(format!("LUT file was not found: {name}"));
    }
    if extension(&path) != ".cube" {
        return Err("Only .cube LUT files are supported.".into());
    }
    Ok(path)
}

fn load_cube(path: &Path) -> Result<Cube, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes).replace('\u{FFFD}', "");
    Cube::parse(&text, &path.display().to_string())
}

// ---------------------------------------------------------------------------------------------------------------------
// Pictures

fn load_picture(path: &Path) -> Result<Rgb, String> {
    let picture = image::open(path).map_err(|e| format!("Could not read the picture {}: {e}", path.display()))?.to_rgb8();
    Ok(Rgb::from_bytes(picture.width() as usize, picture.height() as usize, picture.as_raw()))
}

fn rgb_image(picture: &Rgb) -> Result<image::RgbImage, String> {
    image::RgbImage::from_raw(picture.width as u32, picture.height as u32, picture.to_bytes()).ok_or_else(|| "The picture has the wrong size.".to_string())
}

/// Save by the file's extension. JPEG quality is the encoder's default, as with the pack's `Image.save`.
fn save_picture(path: &Path, picture: &Rgb) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    rgb_image(picture)?.save(path).map_err(|e| format!("Could not save the picture {}: {e}", path.display()))
}

fn save_jpeg(path: &Path, picture: &image::RgbImage, quality: u8) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut file = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode(picture.as_raw(), picture.width(), picture.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("Could not save the picture {}: {e}", path.display()))?;
    file.write_all(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The first frame of a video as a picture.
fn first_frame(tools: &Tools, video: &Path) -> Result<Rgb, String> {
    let (width, height) = video_size(tools, video).map_err(|_| format!("Could not open input video: {}", video.display()))?;
    let (width, height) = (width as usize, height as usize);
    let output = command(&tools.ffmpeg, &args(&["-loglevel", "error", "-i", &text(video), "-map", "0:v:0", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(format!("Could not open input video: {}", video.display()));
    }
    if output.stdout.len() < width * height * 3 {
        return Err("Could not read the first frame from the input video.".into());
    }
    Ok(Rgb::from_bytes(width, height, &output.stdout[..width * height * 3]))
}

/// Save a small JPEG of the first frame of `video` at `thumbnail_path`. Empty when there is no path or the frame cannot be read.
fn write_video_thumbnail(tools: &Tools, video: &Path, thumbnail_path: &str) -> String {
    let wanted = thumbnail_path.trim().trim_matches('"');
    if wanted.is_empty() {
        return String::new();
    }
    let target = absolute(Path::new(wanted));
    let made = (|| -> Result<(), String> {
        let frame = rgb_image(&first_frame(tools, video)?)?;
        let (width, height) = (frame.width(), frame.height());
        let scale = (960.0 / f64::from(width)).min(540.0 / f64::from(height)).min(1.0);
        let small = if scale < 1.0 {
            image::imageops::resize(&frame, (f64::from(width) * scale).round().max(1.0) as u32, (f64::from(height) * scale).round().max(1.0) as u32, image::imageops::FilterType::Lanczos3)
        } else {
            frame
        };
        save_jpeg(&target, &small, 90)
    })();
    if made.is_ok() { text(&target) } else { String::new() }
}

// ---------------------------------------------------------------------------------------------------------------------
// One picture previews

fn preview_root(project_folder: &str) -> Result<PathBuf, String> {
    let project = project_folder.trim().trim_matches('"');
    let root = if project.is_empty() { std::env::temp_dir().join("vrgdg_lut_previews") } else { absolute(Path::new(project)).join("_tmp").join("lut_previews") };
    fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    Ok(root)
}

fn resolve_file(raw: &str, label: &str) -> Result<PathBuf, String> {
    let wanted = raw.trim().trim_matches('"');
    if wanted.is_empty() {
        return Err(format!("{label} is required."));
    }
    let resolved = absolute(Path::new(wanted));
    if !resolved.is_file() {
        return Err(format!("{label} was not found: {}", resolved.display()));
    }
    Ok(resolved)
}

/// What a preview grades: a picture, or the first frame of a clip.
struct PreviewSource {
    input: PathBuf,
    is_video: bool,
}

fn preview_source(payload: &Obj) -> Result<PreviewSource, String> {
    let input = resolve_file(&path_text(payload, "input_path"), "Input media")?;
    let ext = extension(&input);
    let requested = payload.get("media_type").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    let kind = if !requested.is_empty() {
        requested
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        "video".into()
    } else {
        "image".into()
    };
    match kind.as_str() {
        "image" if !IMAGE_EXTENSIONS.contains(&ext.as_str()) => Err("Input image type is not supported.".into()),
        "video" if !VIDEO_EXTENSIONS.contains(&ext.as_str()) => Err("Input video type is not supported.".into()),
        "image" | "video" => Ok(PreviewSource { input, is_video: kind == "video" }),
        _ => Err("Preview media type must be image or video.".into()),
    }
}

/// Grade one picture, or the first frame of a clip, into a preview JPEG in the project's `_tmp/lut_previews` folder.
/// `label` is the part of the file name after the scene and source names. `describe` adds what the grading reports.
fn preview(env: &Env, payload: &Obj, source: &PreviewSource, label: &str, grade: &dyn Fn(&mut Rgb), describe: &dyn Fn(&Path, &Path, f64) -> Value) -> Reply {
    let root = preview_root(&path_text(payload, "project_folder"))?;
    let scene = safe_token(&payload.get("scene_id").filter(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default(), "scene");
    let source_name = safe_token(&stem(&source.input), "media");
    let stamp = millis();
    let output = root.join(format!("{scene}_{source_name}_{label}_{stamp}.jpg"));
    let mut frame_path: Option<PathBuf> = None;
    let graded_from = if source.is_video {
        let tools = env.tools()?;
        let frame = first_frame(&tools, &source.input)?;
        let path = root.join(format!("{scene}_{source_name}_source_frame_{stamp}.jpg"));
        save_jpeg(&path, &rgb_image(&frame)?, 92)?;
        frame_path = Some(path.clone());
        path
    } else {
        source.input.clone()
    };
    let started = Instant::now();
    let result = load_picture(&graded_from).and_then(|mut picture| {
        grade(&mut picture);
        save_picture(&output, &picture)
    });
    if let Err(e) = result {
        if let Some(path) = &frame_path {
            let _ = fs::remove_file(path);
        }
        return Err(e);
    }
    let mut reply = describe(&graded_from, &output, started.elapsed().as_secs_f64());
    if let Some(map) = reply.as_object_mut() {
        map.insert("preview_path".into(), json!(text(&output)));
        map.insert("source_media".into(), json!(text(&source.input)));
        map.insert("source_type".into(), json!(if source.is_video { "video" } else { "image" }));
        map.insert("source_preview_path".into(), json!(text(&graded_from)));
        map.insert("source_preview_temporary".into(), json!(frame_path.is_some()));
    }
    Ok(reply)
}

/// `POST /vrgdg/music_builder/luts/preview`.
pub fn preview_lut(env: &Env, payload: &Obj) -> Reply {
    let source = preview_source(payload)?;
    let cube_path = lut_path(env, &path_text(payload, "lut_name"))?;
    let lut_name = cube_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let strength = float_of(payload, "strength", 10.0)?;
    let cube = load_cube(&cube_path)?;
    let label = safe_token(&stem(&cube_path), "lut");
    preview(env, payload, &source, &label, &|picture| cube.apply(picture, strength), &|input, output, elapsed| {
        json!({"input": text(input), "output": text(output), "lut": lut_name, "device": "cpu", "strength": strength, "replace_source": false, "elapsed_seconds": elapsed})
    })
}

/// `POST /vrgdg/music_builder/post_process/adjust/preview`.
pub fn preview_adjust(env: &Env, payload: &Obj) -> Reply {
    let source = preview_source(payload)?;
    let adjust = Adjust::from_settings(payload.get("settings"));
    preview(env, payload, &source, "adjust", &|picture| adjust.apply(picture), &|input, output, elapsed| {
        json!({"input": text(input), "output": text(output), "effect": "adjust", "device": "cpu", "settings": adjust.to_json(), "replace_source": false, "elapsed_seconds": elapsed})
    })
}

/// `POST /vrgdg/music_builder/post_process/film_grain/preview`.
pub fn preview_film_grain(env: &Env, payload: &Obj) -> Reply {
    let source = preview_source(payload)?;
    let intensity = float_of(payload, "grain_intensity", 0.04)?;
    let saturation = float_of(payload, "saturation_mix", 0.5)?;
    let seed = seed_of(payload).unwrap_or_else(clock_seed);
    preview(env, payload, &source, "film_grain", &|picture| apply_film_grain(picture, intensity, saturation, seed), &|input, output, elapsed| {
        json!({"input": text(input), "output": text(output), "effect": "film_grain", "device": "cpu", "grain_intensity": intensity, "saturation_mix": saturation, "replace_source": false, "elapsed_seconds": elapsed})
    })
}

/// `POST /vrgdg/music_builder/luts/delete_preview`: remove a preview picture. Only files in the preview folder.
pub fn delete_preview(payload: &Obj) -> Reply {
    let root = preview_root(&path_text(payload, "project_folder"))?;
    let raw = path_text(payload, "path");
    let resolved = absolute(Path::new(&raw));
    if raw.is_empty() || !is_inside(&root, &resolved) {
        return Err("Invalid LUT preview path.".into());
    }
    let deleted = resolved.is_file();
    if deleted {
        fs::remove_file(&resolved).map_err(|e| format!("{}: {e}", resolved.display()))?;
    }
    Ok(json!({"deleted": deleted}))
}

/// Whether `path` is `root` or below it. Windows paths compare without regard to case.
fn is_inside(root: &Path, path: &Path) -> bool {
    let key = |p: &Path| -> Vec<String> { absolute(p).components().map(|c| if cfg!(windows) { c.as_os_str().to_string_lossy().to_lowercase() } else { c.as_os_str().to_string_lossy().into_owned() }).collect() };
    let (root, path) = (key(root), key(path));
    path.len() >= root.len() && path[..root.len()] == root[..]
}

// ---------------------------------------------------------------------------------------------------------------------
// Clip renders

/// The files of a clip render: the source, which must be a video, and where the result goes.
fn video_paths(payload: &Obj, default_output: &dyn Fn(&Path) -> PathBuf) -> Result<(PathBuf, PathBuf, bool), String> {
    let input = resolve_file(&path_text(payload, "input_path"), "Input video")?;
    if !VIDEO_EXTENSIONS.contains(&extension(&input).as_str()) {
        return Err("Input video type is not supported.".into());
    }
    let replace = flag(payload, "replace_source", false);
    let output = if replace {
        input.clone()
    } else {
        let wanted = path_text(payload, "output_path");
        if wanted.is_empty() { absolute(&default_output(&input)) } else { absolute(Path::new(&wanted)) }
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    Ok((input, output, replace))
}

fn default_effect_output(input: &Path, effect: &str) -> PathBuf {
    let ext = input.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    input.with_file_name(format!("{}_{}{ext}", stem(input), safe_token(effect, "effect")))
}

fn finish(mut reply: Value, render: Value) -> Reply {
    if let (Some(map), Value::Object(extra)) = (reply.as_object_mut(), render) {
        map.extend(extra);
    }
    Ok(reply)
}

/// `POST /vrgdg/music_builder/luts/apply_video`.
pub fn apply_lut_video(env: &Env, payload: &Obj) -> Reply {
    let cube = lut_path(env, &path_text(payload, "lut_name"))?;
    let lut_name = cube.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (input, output, replace) = video_paths(payload, &|input| {
        let ext = input.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        input.with_file_name(format!("{}_lut_{}{ext}", stem(input), safe_token(&stem(Path::new(&lut_name)), "lut")))
    })?;
    let strength = float_of(payload, "strength", 10.0)?;
    let tools = env.tools()?;
    let render = apply_lut_to_video(&tools, &input, &output, &cube, strength, flag(payload, "preserve_audio", true), crf_of(payload, 23.0), &preset_of(payload)).map_err(|e| e.to_string())?;
    let thumbnail = write_video_thumbnail(&tools, &output, &path_text(payload, "thumbnail_path"));
    finish(json!({"input": text(&input), "output": text(&output), "lut": lut_name, "strength": strength, "replace_source": replace, "thumbnail_path": thumbnail}), render)
}

/// `POST /vrgdg/music_builder/post_process/film_grain/apply_video`.
pub fn apply_film_grain_video(env: &Env, payload: &Obj) -> Reply {
    let (input, output, replace) = video_paths(payload, &|input| default_effect_output(input, "film_grain"))?;
    let intensity = float_of(payload, "grain_intensity", 0.04)?;
    let saturation = float_of(payload, "saturation_mix", 0.5)?;
    let seed = seed_of(payload);
    let base = seed.unwrap_or_else(clock_seed);
    let tools = env.tools()?;
    let encode = Encode { preserve_audio: flag(payload, "preserve_audio", true), crf: crf_of(payload, 26.0), preset: &preset_of(payload) };
    let render = render_effect(&tools, &input, &output, &encode, |frame, index| apply_film_grain(frame, intensity, saturation, base.wrapping_add(index as u64))).map_err(|e| e.to_string())?;
    let thumbnail = write_video_thumbnail(&tools, &output, &path_text(payload, "thumbnail_path"));
    finish(json!({"input": text(&input), "output": text(&output), "effect": "film_grain", "device": "cpu", "grain_intensity": intensity, "saturation_mix": saturation, "replace_source": replace, "thumbnail_path": thumbnail}), render)
}

/// `POST /vrgdg/music_builder/post_process/adjust/apply_video`.
pub fn apply_adjust_video(env: &Env, payload: &Obj) -> Reply {
    let (input, output, replace) = video_paths(payload, &|input| default_effect_output(input, "adjust"))?;
    let adjust = Adjust::from_settings(payload.get("settings"));
    let tools = env.tools()?;
    let encode = Encode { preserve_audio: flag(payload, "preserve_audio", true), crf: crf_of(payload, 23.0), preset: &preset_of(payload) };
    let render = render_effect(&tools, &input, &output, &encode, |frame, _| adjust.apply(frame)).map_err(|e| e.to_string())?;
    let thumbnail = write_video_thumbnail(&tools, &output, &path_text(payload, "thumbnail_path"));
    finish(json!({"input": text(&input), "output": text(&output), "effect": "adjust", "device": "cpu", "settings": adjust.to_json(), "replace_source": replace, "thumbnail_path": thumbnail}), render)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &Path) -> Env {
        Env { output: dir.join("output"), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: dir.join("luts"), overlay_dir: None }
    }

    fn payload(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    fn write_picture(path: &Path) {
        let mut picture = Rgb::new(24, 16);
        for (i, v) in picture.data.iter_mut().enumerate() {
            *v = (i % 97) as f32 / 96.0;
        }
        save_picture(path, &picture).unwrap();
    }

    #[test]
    fn the_lut_list_has_names_labels_and_examples() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        assert_eq!(list_luts(&env).unwrap()["luts"], json!([]));
        fs::write(env.luts.join("Warm_Tone.cube"), "LUT_3D_SIZE 2\n").unwrap();
        fs::write(env.luts.join("notes.txt"), "x").unwrap();
        fs::create_dir_all(env.luts.join("examples")).unwrap();
        fs::write(env.luts.join("examples").join("warm tone.png"), "x").unwrap();
        let listing = list_luts(&env).unwrap();
        let luts = listing["luts"].as_array().unwrap();
        assert_eq!(luts.len(), 1);
        assert_eq!(luts[0]["name"], "Warm_Tone.cube");
        assert_eq!(luts[0]["label"], "Warm Tone");
        assert_eq!(luts[0]["example_name"], "warm tone.png");
        assert_eq!(luts[0]["example_url"], "/vrgdg/music_builder/luts/example?name=warm%20tone.png");
    }

    #[test]
    fn a_picture_preview_lands_in_the_project_preview_folder_and_can_be_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        let project = dir.path().join("project");
        let source = dir.path().join("scene one.png");
        write_picture(&source);
        let reply = preview_adjust(&env, &payload(json!({"input_path": source.display().to_string(), "settings": {"contrast": 30}, "scene_id": "scene 1", "project_folder": project.display().to_string()}))).unwrap();
        let preview_path = PathBuf::from(reply["preview_path"].as_str().unwrap());
        assert!(preview_path.is_file());
        assert!(preview_path.starts_with(project.join("_tmp").join("lut_previews")));
        assert!(preview_path.file_name().unwrap().to_string_lossy().starts_with("scene_1_scene_one_adjust_"));
        assert_eq!(reply["source_type"], "image");
        assert_eq!(reply["source_preview_temporary"], false);
        assert_eq!(reply["settings"]["contrast"], 30.0);
        let outside = dir.path().join("keep.png");
        write_picture(&outside);
        let body = |path: &Path| payload(json!({"path": path.display().to_string(), "project_folder": project.display().to_string()}));
        assert_eq!(delete_preview(&body(&outside)).unwrap_err(), "Invalid LUT preview path.");
        assert!(outside.is_file());
        assert_eq!(delete_preview(&body(&preview_path)).unwrap()["deleted"], true);
        assert_eq!(delete_preview(&body(&preview_path)).unwrap()["deleted"], false);
    }

    #[test]
    fn requests_with_bad_media_get_the_packs_messages() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        let text_file = dir.path().join("notes.txt");
        fs::write(&text_file, "x").unwrap();
        let missing = dir.path().join("none.png");
        assert!(preview_adjust(&env, &payload(json!({"input_path": missing.display().to_string()}))).unwrap_err().starts_with("Input media was not found: "));
        assert_eq!(preview_adjust(&env, &payload(json!({"input_path": text_file.display().to_string()}))).unwrap_err(), "Input image type is not supported.");
        let picture = dir.path().join("a.png");
        write_picture(&picture);
        let wrong = payload(json!({"input_path": picture.display().to_string(), "media_type": "audio"}));
        assert_eq!(preview_adjust(&env, &wrong).unwrap_err(), "Preview media type must be image or video.");
        let no_lut = payload(json!({"input_path": picture.display().to_string(), "lut_name": "gone.cube"}));
        assert_eq!(preview_lut(&env, &no_lut).unwrap_err(), "LUT file was not found: gone.cube");
        let bad_grain = payload(json!({"input_path": picture.display().to_string(), "grain_intensity": "lots"}));
        assert_eq!(preview_film_grain(&env, &bad_grain).unwrap_err(), "could not convert string to float: 'lots'");
    }

    #[test]
    fn a_lut_preview_grades_the_picture() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        fs::create_dir_all(&env.luts).unwrap();
        let all_white = "LUT_3D_SIZE 2\n".to_string() + &"1 1 1\n".repeat(8);
        fs::write(env.luts.join("white.cube"), all_white).unwrap();
        let picture = dir.path().join("a.png");
        write_picture(&picture);
        let reply = preview_lut(&env, &payload(json!({"input_path": picture.display().to_string(), "lut_name": "white.cube", "strength": 10, "project_folder": dir.path().join("p").display().to_string()}))).unwrap();
        assert_eq!(reply["lut"], "white.cube");
        let graded = load_picture(Path::new(reply["preview_path"].as_str().unwrap())).unwrap();
        assert!(graded.data.iter().all(|&v| v > 0.95), "JPEG of an all white picture");
    }
}
