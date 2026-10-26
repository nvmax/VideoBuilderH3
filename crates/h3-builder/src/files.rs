//! Small file routes of the Builder: SRT and prompt files, editable text files, default paths, opening a file in the
//! operating system and the native file pickers. Ports of the helpers in `builder/audio.py`, `builder/paths.py` and
//! `builder/project.py` behind the `/vrgdg/music_builder/*` routes of the same names.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fancy_regex::Regex;
use h3_core::pyval::{round_to, text_or_empty, Obj};
use h3_project::atomic::atomic_write_text;
use h3_project::layout::{context_folder, srt_path};
use serde_json::{json, Value};

use crate::linux_dialog;
use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

fn absolute(path: &Path) -> PathBuf {
    h3_core::abspath::absolute(path).unwrap_or_else(|_| path.to_path_buf()).components().collect()
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

/// `_resolve_existing_file`.
pub fn resolve_existing_file(raw: &str, label: &str) -> Result<PathBuf, String> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return Err(format!("{label} path is empty."));
    }
    let path = absolute(Path::new(raw));
    if !path.is_file() {
        return Err(format!("{label} was not found: {}", path.display()));
    }
    Ok(path)
}

fn read_text_lossy(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string())
}

// -- SRT ----------------------------------------------------------------------------------------------------------------

/// `_parse_srt_time`: `HH:MM:SS,mmm` (a comma or a dot) as seconds. The fraction digits count as whole milliseconds.
pub fn parse_srt_time(value: &str) -> Result<f64, String> {
    let bad = || format!("Invalid SRT time: {value}");
    let parts: Vec<&str> = value.trim().splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(bad());
    }
    let (seconds_part, fraction) = parts[2].split_once([',', '.']).ok_or_else(bad)?;
    let digits = |s: &str| -> Result<f64, String> {
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        s.parse::<f64>().map_err(|_| bad())
    };
    Ok(digits(parts[0])? * 3600.0 + digits(parts[1])? * 60.0 + digits(seconds_part)? + digits(fraction)? / 1000.0)
}

/// `_parse_srt_segments`: one scene per timing block.
pub fn parse_srt_segments(srt: &str) -> Result<Vec<Value>, String> {
    let splitter = Regex::new(r"\n\s*\n").map_err(|e| e.to_string())?;
    let normalized = srt.trim().to_string();
    let mut segments = Vec::new();
    for block in splitter.split(&normalized).filter_map(Result::ok) {
        let lines: Vec<&str> = block.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let Some(timing) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let (left, right) = lines[timing].split_once("-->").map(|(a, b)| (a.trim(), b.trim())).unwrap_or(("", ""));
        let start = parse_srt_time(left)?;
        let end = (start + 0.1).max(parse_srt_time(right)?);
        let joined = lines[timing + 1..].join(" ");
        let label = joined.trim().to_string();
        let number = segments.len() + 1;
        let label = if label.is_empty() { format!("Scene {number}") } else { label };
        let short: String = label.chars().take(80).collect();
        segments.push(json!({
            "id": format!("srt_{number}_{}", (start * 1000.0) as i64),
            "start": round_to(start, 3),
            "end": round_to(end, 3),
            "label": if short.is_empty() { format!("Scene {number}") } else { short },
            "notes": label,
            "t2i_prompt": "",
            "i2v_prompt": "",
            "ref_image_path": "",
            "use_vision_reference": false,
            "image": null,
            "source": "srt",
        }));
    }
    Ok(segments)
}

/// `POST /vrgdg/music_builder/load_srt`.
pub fn load_srt(payload: &Obj) -> Reply {
    let path = resolve_existing_file(&path_text(payload, "srt_path"), "SRT file")?;
    let segments = parse_srt_segments(&read_text_lossy(&path)?)?;
    if segments.is_empty() {
        return Err("No SRT timing blocks were found.".into());
    }
    Ok(json!({"srt_path": text(&path), "segments": segments}))
}

// -- prompt files -------------------------------------------------------------------------------------------------------

/// The first number in a key, `999999` when it has none. Sorting prompts by this keeps `scene_2` before `scene_10`.
fn prompt_key_number(key: &str) -> i64 {
    let digits: String = key.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(999_999)
}

fn sorted_prompts(map: &Obj) -> Vec<String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by_key(|k| prompt_key_number(k));
    keys.into_iter().map(|k| text_or_empty(map.get(k)).trim().to_string()).collect()
}

/// `POST /vrgdg/music_builder/load_prompt_json`.
pub fn load_prompt_json(payload: &Obj) -> Reply {
    let path = resolve_existing_file(&path_text(payload, "prompt_json_path"), "Prompt JSON")?;
    let data = h3_core::pyjson::parse(&read_text_lossy(&path)?).map_err(|e| e.to_string())?;
    let mut prompts: Vec<String> = Vec::new();
    match &data {
        Value::Object(map) => prompts.extend(sorted_prompts(map)),
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(s) => prompts.push(s.trim().to_string()),
                    Value::Object(map) => prompts.extend(sorted_prompts(map)),
                    _ => {}
                }
            }
        }
        _ => return Err("Prompt JSON must be an object or list.".into()),
    }
    if prompts.is_empty() {
        return Err("Prompt JSON did not contain any prompt text.".into());
    }
    Ok(json!({"prompt_json_path": text(&path), "prompts": prompts}))
}

// -- editable text files ------------------------------------------------------------------------------------------------

fn editable_path(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return Err("Text file path is empty.".into());
    }
    let path = absolute(Path::new(raw));
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if extension != "txt" && extension != "json" {
        return Err("Only .txt or .json files can be edited here.".into());
    }
    Ok(path)
}

/// `POST /vrgdg/music_builder/load_text_file`.
pub fn load_text_file(payload: &Obj) -> Reply {
    let path = editable_path(&path_text(payload, "path"))?;
    if !path.is_file() {
        return Err(format!("Text file was not found: {}", path.display()));
    }
    Ok(json!({"path": text(&path), "content": read_text_lossy(&path)?}))
}

/// `POST /vrgdg/music_builder/save_text_file`.
pub fn save_text_file(payload: &Obj) -> Reply {
    let path = editable_path(&path_text(payload, "path"))?;
    atomic_write_text(&path, &text_or_empty(payload.get("content"))).map_err(|e| e.to_string())?;
    Ok(json!({"path": text(&path)}))
}

// -- default paths ------------------------------------------------------------------------------------------------------

fn text_file_path(env: &Env, folder: &str, file: &str) -> String {
    text(&env.output.join("VRGDG_TEMP").join("TextFiles").join(folder).join(file))
}

/// `GET /vrgdg/music_builder/default_context_paths`.
pub fn default_context_paths(env: &Env) -> Value {
    json!({
        "concept_prompts_path": text_file_path(env, "ConceptPrompts", "ConceptPrompts.txt"),
        "i2v_motion_notes_path": text_file_path(env, "I2VMotionNotes", "I2VMotionNotes.txt"),
        "theme_style_path": text_file_path(env, "themestyle", "themestyle.txt"),
        "story_idea_path": text_file_path(env, "storyconcept", "storyconcept.txt"),
        "subject_scene_path": text_file_path(env, "subjectandscenes", "subjectsandscenes.txt"),
    })
}

/// The most recently changed file in `folder` with one of the extensions, or "".
pub fn newest_file(folder: &Path, extensions: &[&str]) -> String {
    let Ok(entries) = fs::read_dir(folder) else { return String::new() };
    entries
        .flatten()
        .filter(|e| e.path().is_file() && extensions.iter().any(|x| e.file_name().to_string_lossy().to_lowercase().ends_with(x)))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| text(&e.path()))
        .unwrap_or_default()
}

/// `GET /vrgdg/music_builder/default_audio_srt_paths`: the newest song in the output folder's audio folder. The pack also
/// looks in its own `srt_files` folder, which this app does not have.
pub fn default_audio_srt_paths(env: &Env) -> Value {
    let audio_folder = env.output.join("VRGDG_AudioFiles");
    json!({
        "audio_path": newest_file(&audio_folder, &[".wav", ".mp3", ".flac", ".m4a", ".ogg"]),
        "srt_path": "",
        "audio_folder": text(&audio_folder),
        "srt_folder": "",
    })
}

/// `POST /vrgdg/music_builder/project_prompt_creator_paths`: where a project keeps the files the Prompt Creator writes,
/// and which of them exist.
pub fn project_prompt_creator_paths(payload: &Obj) -> Reply {
    let raw = path_text(payload, "project_folder");
    if raw.is_empty() {
        return Err("Create or load a project before importing Prompt Creator data.".into());
    }
    let folder = absolute(Path::new(&raw));
    let context = context_folder(&folder);
    let entries: [(&str, String); 9] = [
        ("project_folder", text(&folder)),
        ("audio_path", newest_file(&folder.join("audio"), &[".wav", ".mp3", ".flac", ".m4a", ".ogg", ".mp4"])),
        ("srt_path", text(&srt_path(&folder))),
        ("lyric_segments_path", text(&folder.join("prompts").join("lyric_segments.json"))),
        ("concept_prompts_path", text(&context.join("ConceptPrompts.txt"))),
        ("i2v_motion_notes_path", text(&context.join("I2VMotionNotes.txt"))),
        ("theme_style_path", text(&context.join("themestyle.txt"))),
        ("story_idea_path", text(&context.join("storyconcept.txt"))),
        ("subject_scene_path", text(&context.join("subjectsandscenes.txt"))),
    ];
    let mut paths = serde_json::Map::new();
    let mut exists = serde_json::Map::new();
    for (key, value) in &entries {
        paths.insert((*key).into(), json!(value));
        if key.ends_with("_path") {
            exists.insert((*key).into(), json!(!value.is_empty() && Path::new(value).is_file()));
        }
    }
    let flag = |key: &str| exists.get(key).and_then(Value::as_bool).unwrap_or(false);
    let ready = flag("srt_path") && flag("concept_prompts_path");
    paths.insert("exists".into(), Value::Object(exists));
    paths.insert("ready".into(), json!(ready));
    Ok(Value::Object(paths))
}

// -- the operating system -----------------------------------------------------------------------------------------------

const OPENABLE_EXTENSIONS: [&str; 16] = [
    ".mp4", ".mov", ".webm", ".mkv", ".avi", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".wav", ".mp3", ".flac", ".m4a", ".txt", ".srt",
];

/// `POST /vrgdg/music_builder/open_local_file`: show a folder, or open a video, image, audio or text file in its default
/// program. Anything that could run a program is refused.
pub fn open_local_file(payload: &Obj) -> Reply {
    let raw = path_text(payload, "path");
    let target = fs::canonicalize(&raw).unwrap_or_else(|_| absolute(Path::new(&raw)));
    if !target.is_dir() {
        if !target.is_file() {
            return Err("File was not found.".into());
        }
        let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let extension = target.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
        if !OPENABLE_EXTENSIONS.contains(&extension.as_str()) && extension != ".json" {
            return Err(format!("Only video, image, audio and text files can be opened: {name}"));
        }
    }
    let shown = target.display().to_string();
    let shown = shown.strip_prefix(r"\\?\").unwrap_or(&shown).to_string();
    open_with_system(Path::new(&shown)).map_err(|e| e.to_string())?;
    Ok(json!({"path": shown}))
}

fn open_with_system(target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(target).creation_flags(0x0800_0000);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

/// The title, file filter and kind of one native file dialog.
fn picker_spec(kind: &str) -> Result<(&'static str, Option<&'static str>, bool), String> {
    Ok(match kind {
        "audio" => ("Choose audio file", Some("Audio files (*.wav;*.mp3;*.flac;*.m4a;*.ogg)|*.wav;*.mp3;*.flac;*.m4a;*.ogg|All files (*.*)|*.*"), false),
        "srt" => ("Choose SRT file", Some("SRT files (*.srt)|*.srt|All files (*.*)|*.*"), false),
        "image" => ("Choose image file", Some("Image files (*.png;*.jpg;*.jpeg;*.webp)|*.png;*.jpg;*.jpeg;*.webp|All files (*.*)|*.*"), false),
        "video" => ("Choose video file", Some("Video files (*.mp4;*.mov;*.mkv;*.webm;*.avi)|*.mp4;*.mov;*.mkv;*.webm;*.avi|All files (*.*)|*.*"), false),
        "gguf" => ("Choose GGUF model file", Some("GGUF model files (*.gguf)|*.gguf|All files (*.*)|*.*"), false),
        "project_folder" => ("Choose project folder", None, true),
        "project_root" => ("Choose projects root folder", None, true),
        "folder" => ("Choose folder", None, true),
        "program" => ("Choose program", Some("Programs (*.exe)|*.exe|All files (*.*)|*.*"), false),
        other => return Err(format!("Unknown picker type: {other}")),
    })
}

/// Run a PowerShell dialog script and return what it wrote: the chosen path, or "" when the dialog was cancelled.
fn powershell_dialog(script: &str) -> Result<String, String> {
    let mut command = Command::new("powershell");
    command.args(["-NoProfile", "-STA", "-Command", script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| format!("Native file dialog is not available: {e}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(if output.stderr.is_empty() { &output.stdout } else { &output.stderr }).trim().to_string();
        return Err(if message.is_empty() { "PowerShell picker failed.".into() } else { message });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

const SAVE_VIDEO_FILTER: &str = "MP4 video (*.mp4)|*.mp4";

/// The PowerShell Save dialog that starts with `name`. Single quotes in a PowerShell string are doubled.
fn save_video_script(name: &str) -> String {
    format!(
        "Add-Type -AssemblyName System.Windows.Forms\n$dialog = New-Object System.Windows.Forms.SaveFileDialog\n$dialog.Title = 'Export video'\n$dialog.Filter = '{SAVE_VIDEO_FILTER}'\n$dialog.DefaultExt = 'mp4'\n$dialog.AddExtension = $true\n$dialog.OverwritePrompt = $true\n$dialog.FileName = '{}'\nif ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ [Console]::Write($dialog.FileName) }}",
        name.replace('\'', "''")
    )
}

/// The `save_video` dialog of the Export button: where to save the finished video, starting with `file_name`. The reply is the
/// chosen path, or "" when the dialog was closed. The dialog asks before it replaces a video.
fn save_video_dialog(file_name: &str) -> Reply {
    let name = crate::export_video::export_file_name(file_name);
    if cfg!(target_os = "linux") {
        let dialog = linux_dialog::Dialog { title: "Export video", filter: Some(SAVE_VIDEO_FILTER), folder: false, multiple: false, save_as: Some(&name) };
        return Ok(json!({"path": linux_dialog::choose(&dialog)?.first().cloned().unwrap_or_default()}));
    }
    if !cfg!(windows) {
        return Err("Native file dialog is not available on this system. Type the path instead.".into());
    }
    Ok(json!({"path": powershell_dialog(&save_video_script(&name))?}))
}

/// `POST /vrgdg/music_builder/pick_path`: a native file or folder dialog on this computer. Returns the chosen path, or ""
/// when the dialog was cancelled. `kind: "save_video"` is a save dialog that starts with the name in `file_name`.
pub fn pick_path(payload: &Obj) -> Reply {
    let kind = text_or_empty(payload.get("kind"));
    if kind == "save_video" {
        return save_video_dialog(&text_or_empty(payload.get("file_name")));
    }
    let (title, filter, folder) = picker_spec(&kind)?;
    if cfg!(target_os = "linux") {
        // A program has no file extension on Linux, so that dialog lists every file.
        let filter = filter.filter(|_| kind != "program");
        let chosen = linux_dialog::choose(&linux_dialog::Dialog { title, filter, folder, multiple: false, save_as: None })?;
        return Ok(json!({"path": chosen.first().cloned().unwrap_or_default()}));
    }
    if !cfg!(windows) {
        return Err("Native file dialog is not available on this system. Type the path instead.".into());
    }
    let script = match (folder, filter) {
        (true, _) => format!(
            "Add-Type -AssemblyName System.Windows.Forms\n$dialog = New-Object System.Windows.Forms.FolderBrowserDialog\n$dialog.Description = '{title}'\nif ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ [Console]::Write($dialog.SelectedPath) }}"
        ),
        (false, filter) => format!(
            "Add-Type -AssemblyName System.Windows.Forms\n$dialog = New-Object System.Windows.Forms.OpenFileDialog\n$dialog.Title = '{title}'\n$dialog.Filter = '{}'\nif ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ [Console]::Write($dialog.FileName) }}",
            filter.unwrap_or("All files (*.*)|*.*")
        ),
    };
    Ok(json!({"path": powershell_dialog(&script)?}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(value: Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn srt_blocks_become_scenes() {
        let srt = "1\n00:00:00,000 --> 00:00:04,200\nFirst line\n\n2\n00:00:04.200 --> 00:00:04.250\nsecond\nline\n\n3\nnot a block\n";
        let segments = parse_srt_segments(srt).unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0]["id"], "srt_1_0");
        assert_eq!(segments[0]["end"], 4.2);
        assert_eq!(segments[1]["id"], "srt_2_4200");
        assert_eq!(segments[1]["label"], "second line");
        // An end before start + 0.1 is pushed out to 0.1 s.
        assert_eq!(segments[1]["end"], 4.3);
        assert!(parse_srt_time("1:2").is_err());
        assert_eq!(parse_srt_time("00:01:02,5").unwrap(), 62.005);
    }

    #[test]
    fn prompt_json_sorts_keys_by_number() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        fs::write(&path, r#"{"prompt_10": "ten", "prompt_2": " two ", "other": "x"}"#).unwrap();
        let reply = load_prompt_json(&obj(json!({"prompt_json_path": text(&path)}))).unwrap();
        assert_eq!(reply["prompts"], json!(["two", "ten", "x"]));
        fs::write(&path, "[]").unwrap();
        assert!(load_prompt_json(&obj(json!({"prompt_json_path": text(&path)}))).unwrap_err().contains("did not contain"));
    }

    #[test]
    fn text_files_are_limited_to_txt_and_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("note.txt");
        save_text_file(&obj(json!({"path": text(&path), "content": "hello"}))).unwrap();
        assert_eq!(load_text_file(&obj(json!({"path": text(&path)}))).unwrap()["content"], "hello");
        assert!(save_text_file(&obj(json!({"path": text(&dir.path().join("x.exe")), "content": ""}))).is_err());
        assert!(load_text_file(&obj(json!({"path": text(&dir.path().join("missing.txt"))}))).is_err());
    }

    #[test]
    fn open_refuses_programs() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("run.bat");
        fs::write(&exe, "echo").unwrap();
        assert!(open_local_file(&obj(json!({"path": text(&exe)}))).unwrap_err().starts_with("Only video, image"));
        assert_eq!(open_local_file(&obj(json!({"path": text(&dir.path().join("none.mp4"))}))).unwrap_err(), "File was not found.");
    }

    #[test]
    fn the_save_dialog_asks_before_replacing_and_a_quote_cannot_end_its_string() {
        let script = save_video_script("My Song.mp4");
        assert!(script.contains("System.Windows.Forms.SaveFileDialog"));
        assert!(script.contains("$dialog.Filter = 'MP4 video (*.mp4)|*.mp4'"));
        assert!(script.contains("$dialog.DefaultExt = 'mp4'"));
        assert!(script.contains("$dialog.OverwritePrompt = $true"));
        assert!(script.contains("$dialog.FileName = 'My Song.mp4'"));
        assert!(save_video_script("Don't stop.mp4").contains("$dialog.FileName = 'Don''t stop.mp4'"));
        let injected = save_video_script(&crate::export_video::export_file_name("a'; Remove-Item x; '"));
        assert!(injected.contains("$dialog.FileName = 'a''; Remove-Item x; ''.mp4'"), "{injected}");
    }

    #[test]
    fn prompt_creator_paths_report_what_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("builder_segments.srt"), "x").unwrap();
        let paths = project_prompt_creator_paths(&obj(json!({"project_folder": text(dir.path())}))).unwrap();
        assert_eq!(paths["exists"]["srt_path"], true);
        assert_eq!(paths["exists"]["concept_prompts_path"], false);
        assert_eq!(paths["ready"], false);
    }
}
