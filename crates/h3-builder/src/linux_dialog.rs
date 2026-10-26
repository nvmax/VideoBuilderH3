//! The native file and folder dialogs on Linux, through `zenity` or `kdialog`, whichever is installed. Windows has PowerShell dialogs for the same
//! picks (`files::pick_path`, `refmod_studio::pick_image_files`). The filters are written the Windows way, `Audio (*.wav;*.mp3)|*.wav;*.mp3|All files
//! (*.*)|*.*`, and are turned into the form each program takes.

use std::io::ErrorKind;
use std::process::{Command, Stdio};

/// What one dialog asks for.
pub struct Dialog<'a> {
    pub title: &'a str,
    /// A Windows style filter, or none.
    pub filter: Option<&'a str>,
    pub folder: bool,
    pub multiple: bool,
    /// A save dialog (one new or replaced file) that starts with this file name.
    pub save_as: Option<&'a str>,
}

/// What one try at one program came to.
#[derive(Debug, PartialEq)]
enum Attempt {
    /// The program is not installed.
    Missing,
    Chosen(Vec<String>),
    /// The dialog was closed without a choice.
    Cancelled,
    Failed(String),
}

/// The `(label, patterns)` pairs of a Windows style filter. `*.*` is every file there and `*` here. File names on Linux are case sensitive, so a
/// pattern such as `*.wav` also gets `*.WAV`.
fn filters(filter: &str) -> Vec<(String, Vec<String>)> {
    let parts: Vec<&str> = filter.split('|').collect();
    parts
        .chunks(2)
        .filter(|pair| pair.len() == 2)
        .map(|pair| {
            let label = pair[0].split(" (").next().unwrap_or(pair[0]).trim().to_string();
            let mut patterns: Vec<String> = Vec::new();
            for pattern in pair[1].split(';').map(str::trim).filter(|p| !p.is_empty()) {
                let pattern = if pattern == "*.*" { "*" } else { pattern };
                for variant in [pattern.to_string(), pattern.to_uppercase()] {
                    if !patterns.contains(&variant) {
                        patterns.push(variant);
                    }
                }
            }
            (label, patterns)
        })
        .collect()
}

/// Where a save dialog starts: the home folder, with the suggested name filled in.
fn save_start(name: &str) -> String {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| ".".into());
    format!("{}/{name}", home.trim_end_matches('/'))
}

fn zenity_args(dialog: &Dialog) -> Vec<String> {
    let mut args = vec!["--file-selection".to_string(), format!("--title={}", dialog.title)];
    if dialog.folder {
        args.push("--directory".into());
        return args;
    }
    if let Some(name) = dialog.save_as {
        args.extend(["--save".to_string(), "--confirm-overwrite".to_string(), format!("--filename={}", save_start(name))]);
    } else if dialog.multiple {
        args.push("--multiple".into());
        args.push("--separator=\n".into());
    }
    for (label, patterns) in dialog.filter.map(filters).unwrap_or_default() {
        args.push(format!("--file-filter={label} | {}", patterns.join(" ")));
    }
    args
}

fn kdialog_args(dialog: &Dialog) -> Vec<String> {
    let start = std::env::var("HOME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| ".".into());
    let mut args = vec!["--title".to_string(), dialog.title.to_string()];
    if dialog.folder {
        args.extend(["--getexistingdirectory".to_string(), start]);
        return args;
    }
    match dialog.save_as {
        Some(name) => args.extend(["--getsavefilename".to_string(), save_start(name)]),
        None => args.extend(["--getopenfilename".to_string(), start]),
    }
    let filter: Vec<String> = dialog.filter.map(filters).unwrap_or_default().into_iter().map(|(label, patterns)| format!("{}|{label}", patterns.join(" "))).collect();
    if !filter.is_empty() {
        args.push(filter.join("\n"));
    }
    if dialog.multiple && dialog.save_as.is_none() {
        args.extend(["--multiple".to_string(), "--separate-output".to_string()]);
    }
    args
}

fn attempt(program: &str, args: &[String]) -> Attempt {
    let output = match Command::new(program).args(args).stdin(Stdio::null()).output() {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => return Attempt::Missing,
        Err(error) => return Attempt::Failed(format!("The file dialog could not be started: {error}")),
    };
    if output.status.success() {
        let chosen: Vec<String> = String::from_utf8_lossy(&output.stdout).lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()).map(str::to_string).collect();
        return if chosen.is_empty() { Attempt::Cancelled } else { Attempt::Chosen(chosen) };
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // Both programs end with 1 when the dialog is closed, and zenity also ends with 1 when there is no screen to open it on.
    if output.status.code() == Some(1) && !message.to_lowercase().contains("display") {
        return Attempt::Cancelled;
    }
    Attempt::Failed(if message.is_empty() { format!("{program} failed.") } else { message })
}

/// Show the dialog and return the chosen paths: none when it was closed without a choice.
pub fn choose(dialog: &Dialog) -> Result<Vec<String>, String> {
    for (program, args) in [("zenity", zenity_args(dialog)), ("kdialog", kdialog_args(dialog))] {
        match attempt(program, &args) {
            Attempt::Missing => continue,
            Attempt::Chosen(paths) => return Ok(paths),
            Attempt::Cancelled => return Ok(Vec::new()),
            Attempt::Failed(message) => return Err(message),
        }
    }
    Err("Native file dialog is not available: install zenity or kdialog, or type the path instead.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUDIO: &str = "Audio files (*.wav;*.mp3)|*.wav;*.mp3|All files (*.*)|*.*";

    fn dialog(filter: Option<&'static str>, folder: bool, multiple: bool) -> Dialog<'static> {
        Dialog { title: "Choose audio file", filter, folder, multiple, save_as: None }
    }

    const MP4: &str = "MP4 video (*.mp4)|*.mp4";

    fn save_dialog() -> Dialog<'static> {
        Dialog { title: "Export video", filter: Some(MP4), folder: false, multiple: true, save_as: Some("My Song.mp4") }
    }

    #[test]
    fn a_windows_filter_becomes_labels_and_patterns_in_both_cases() {
        let list = filters(AUDIO);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "Audio files");
        assert_eq!(list[0].1, ["*.wav", "*.WAV", "*.mp3", "*.MP3"]);
        assert_eq!(list[1], ("All files".to_string(), vec!["*".to_string()]));
    }

    #[test]
    fn zenity_gets_the_filters_and_a_line_separator_for_many_files() {
        let args = zenity_args(&dialog(Some(AUDIO), false, true));
        assert_eq!(args[..2], ["--file-selection", "--title=Choose audio file"]);
        assert!(args.contains(&"--multiple".to_string()) && args.contains(&"--separator=\n".to_string()));
        assert!(args.contains(&"--file-filter=Audio files | *.wav *.WAV *.mp3 *.MP3".to_string()));
        assert!(args.contains(&"--file-filter=All files | *".to_string()));
        assert_eq!(zenity_args(&dialog(None, true, false)), ["--file-selection", "--title=Choose audio file", "--directory"]);
    }

    #[test]
    fn kdialog_gets_the_filters_in_its_own_form() {
        let args = kdialog_args(&dialog(Some(AUDIO), false, true));
        assert_eq!(args[..2], ["--title", "Choose audio file"]);
        assert_eq!(args[2], "--getopenfilename");
        assert!(args.contains(&"*.wav *.WAV *.mp3 *.MP3|Audio files\n*|All files".to_string()));
        assert!(args.ends_with(&["--multiple".to_string(), "--separate-output".to_string()]));
        assert_eq!(kdialog_args(&dialog(None, true, false))[2], "--getexistingdirectory");
    }

    #[test]
    fn a_save_dialog_starts_with_the_name_and_asks_before_replacing() {
        let args = zenity_args(&save_dialog());
        assert_eq!(args[..2], ["--file-selection", "--title=Export video"]);
        assert!(args.contains(&"--save".to_string()) && args.contains(&"--confirm-overwrite".to_string()));
        assert!(args.iter().any(|a| a.starts_with("--filename=") && a.ends_with("/My Song.mp4")), "{args:?}");
        assert!(args.contains(&"--file-filter=MP4 video | *.mp4 *.MP4".to_string()));
        assert!(!args.contains(&"--multiple".to_string()), "a save dialog picks one file");

        let args = kdialog_args(&save_dialog());
        assert_eq!(args[..2], ["--title", "Export video"]);
        assert_eq!(args[2], "--getsavefilename");
        assert!(args[3].ends_with("/My Song.mp4"), "{args:?}");
        assert_eq!(args[4], "*.mp4 *.MP4|MP4 video");
        assert!(!args.contains(&"--multiple".to_string()));
    }

    #[test]
    fn a_program_that_is_not_installed_is_skipped() {
        assert_eq!(attempt("h3-no-such-dialog-program", &[]), Attempt::Missing);
    }

    #[cfg(unix)]
    fn script(dir: &std::path::Path, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-dialog");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn the_answer_of_a_dialog_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let chosen = script(dir.path(), "printf '/music/a.wav\\n/music/b.wav\\n'");
        assert_eq!(attempt(&chosen, &[]), Attempt::Chosen(vec!["/music/a.wav".into(), "/music/b.wav".into()]));
        let closed = script(dir.path(), "exit 1");
        assert_eq!(attempt(&closed, &[]), Attempt::Cancelled);
        let no_screen = script(dir.path(), "echo 'cannot open display:' >&2; exit 1");
        assert_eq!(attempt(&no_screen, &[]), Attempt::Failed("cannot open display:".into()));
        let broken = script(dir.path(), "echo boom >&2; exit 5");
        assert_eq!(attempt(&broken, &[]), Attempt::Failed("boom".into()));
    }
}
