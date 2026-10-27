//! The overlay library with real files: importing a clip (and a folder that holds one), the refusals, the previews, using a clip in a project and
//! deleting it. Needs ffmpeg and ffprobe on `PATH` (the tests say so and stop when they are missing).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use h3_builder::overlay_library::{delete, import, list, thumb_path, use_in_project};
use h3_builder::Env;
use h3_core::pyval::Obj;
use h3_media::Tools;
use serde_json::{json, Value};

fn tools() -> Option<Tools> {
    let found = Tools::find("", "").ok();
    if found.is_none() {
        eprintln!("ffmpeg and ffprobe are not on PATH: Overlay library tests skipped");
    }
    found
}

fn env(dir: &Path) -> Env {
    Env { output: dir.join("output"), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: dir.join("config").join("luts"), overlay_dir: None }
}

fn payload(value: Value) -> Obj {
    value.as_object().unwrap().clone()
}

/// A two second clip of dark gray with a white box, `name` in `folder`.
fn clip(tools: &Tools, folder: &Path, name: &str, seconds: u32) -> PathBuf {
    fs::create_dir_all(folder).unwrap();
    let path = folder.join(name);
    let out = Command::new(&tools.ffmpeg)
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", &format!("color=c=black:s=160x90:r=24:d={seconds},drawbox=x=20:y=15:w=60:h=40:color=white:t=fill"), "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    path
}

fn ids(items: &Value) -> Vec<String> {
    items.as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_clip_is_copied_in_probed_and_given_previews() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let source = clip(&tools, &dir.path().join("downloads"), "Dust Hit - 002.mp4", 2);
    let reply = import(&env, &payload(json!({"path": source.display().to_string()}))).unwrap();
    let added = reply["added"].as_array().unwrap();
    assert_eq!(added.len(), 1, "{reply}");
    let item = &added[0];
    assert_eq!(item["id"], "dust/Dust Hit - 002.mp4");
    assert_eq!(item["category"], "dust");
    assert_eq!((item["width"].as_u64(), item["height"].as_u64()), (Some(160), Some(90)));
    assert!((item["duration"].as_f64().unwrap() - 2.0).abs() < 0.1);
    assert_eq!(item["has_alpha"], false);
    assert_eq!(item["suggested_blend"], "screen");
    assert!(item["thumb_url"].as_str().unwrap().contains("kind=image"));
    assert!(item["loop_url"].as_str().unwrap().contains("kind=loop"));
    assert!(env.overlay_library().join("dust").join("Dust Hit - 002.mp4").is_file());
    assert!(thumb_path(&env, "dust/Dust Hit - 002.mp4", "image").unwrap().is_file());
    assert!(thumb_path(&env, "dust/Dust Hit - 002.mp4", "loop").unwrap().is_file());
    assert!(source.is_file(), "the download is copied, not moved");
    assert_eq!(ids(&list(&env).unwrap()["items"]), ["dust/Dust Hit - 002.mp4"]);
}

#[test]
fn importing_the_same_clip_again_adds_nothing_and_a_different_one_with_the_same_name_gets_a_number() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let first = clip(&tools, &dir.path().join("a"), "Smoke 1.mp4", 2);
    import(&env, &payload(json!({"path": first.display().to_string(), "category": "smoke"}))).unwrap();
    let again = import(&env, &payload(json!({"path": first.display().to_string(), "category": "smoke"}))).unwrap();
    assert_eq!(again["added"], json!([]));
    assert_eq!(again["already_there"].as_array().unwrap().len(), 1);
    let other = clip(&tools, &dir.path().join("b"), "Smoke 1.mp4", 3);
    let reply = import(&env, &payload(json!({"path": other.display().to_string(), "category": "Smoke"}))).unwrap();
    assert_eq!(reply["added"][0]["id"], "smoke/Smoke 1_2.mp4");
    assert_eq!(ids(&list(&env).unwrap()["items"]), ["smoke/Smoke 1.mp4", "smoke/Smoke 1_2.mp4"]);
}

#[test]
fn a_download_folder_is_searched_and_red_raw_is_refused() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    // What an FX Elements ProRes download unzips to: a folder with a read me and the clip, plus a macOS leftover.
    let download = dir.path().join("FX Elements - Air Particles - 008");
    clip(&tools, &download, "AirParticles-008.mp4", 2);
    fs::write(download.join("-FX Elements - Read Me - ProRes.pdf"), b"%PDF").unwrap();
    fs::create_dir_all(dir.path().join("__MACOSX")).unwrap();
    let reply = import(&env, &payload(json!({"path": download.display().to_string()}))).unwrap();
    assert_eq!(reply["added"][0]["id"], "particles/AirParticles-008.mp4");
    // What the RAW download looks like: a folder with an .RDC folder that holds an .R3D file.
    let raw = dir.path().join("Atmosphere-014-RAW");
    fs::create_dir_all(raw.join("FX Elements - Atmosphere - 014").join("Atmosphere-014.RDC")).unwrap();
    fs::write(raw.join("FX Elements - Atmosphere - 014").join("Atmosphere-014.RDC").join("Atmosphere-014.R3D"), b"RED2").unwrap();
    let error = import(&env, &payload(json!({"path": raw.display().to_string()}))).unwrap_err();
    assert!(error.contains("ProRes"), "{error}");
    let error = import(&env, &payload(json!({"path": raw.join("FX Elements - Atmosphere - 014").join("Atmosphere-014.RDC").join("Atmosphere-014.R3D").display().to_string()}))).unwrap_err();
    assert!(error.contains("RED RAW") && error.contains("ProRes"), "{error}");
    assert_eq!(ids(&list(&env).unwrap()["items"]), ["particles/AirParticles-008.mp4"], "nothing from the RAW download was kept");
}

#[test]
fn a_file_that_is_not_a_video_is_refused_and_leaves_no_copy() {
    let Some(_tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let fake = dir.path().join("fake.mov");
    fs::write(&fake, b"this is not a video").unwrap();
    let error = import(&env, &payload(json!({"path": fake.display().to_string()}))).unwrap_err();
    assert!(error.to_lowercase().contains("ffprobe") || error.contains("not a video") || error.contains("no video"), "{error}");
    assert!(!env.overlay_library().join("other").join("fake.mov").exists(), "the bad copy was removed");
    assert!(import(&env, &payload(json!({"path": dir.path().join("missing.mov").display().to_string()}))).unwrap_err().contains("Not found"));
    assert!(import(&env, &payload(json!({"path": ""}))).is_err());
    assert!(import(&env, &payload(json!({"path": dir.path().join("notes.txt").display().to_string()}))).is_err());
}

#[test]
fn a_clip_is_copied_into_a_project_once_and_deleted_with_its_previews() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let source = clip(&tools, &dir.path().join("d"), "Snow-001.mp4", 2);
    import(&env, &payload(json!({"path": source.display().to_string()}))).unwrap();
    let project = dir.path().join("My Song");
    fs::create_dir_all(&project).unwrap();
    let not_yet = use_in_project(&env, &payload(json!({"id": "snow/Snow-001.mp4", "project_folder": project.display().to_string()}))).unwrap_err();
    assert!(not_yet.contains("not a project folder"), "{not_yet}");
    fs::write(project.join("vrgdg_builder_session.json"), b"{}").unwrap();
    let used = use_in_project(&env, &payload(json!({"id": "snow/Snow-001.mp4", "project_folder": project.display().to_string()}))).unwrap();
    assert_eq!(used["path"], "overlay/Snow-001.mp4");
    assert_eq!(used["copied"], true);
    assert!(project.join("overlay").join("Snow-001.mp4").is_file());
    let again = use_in_project(&env, &payload(json!({"id": "snow/Snow-001.mp4", "project_folder": project.display().to_string()}))).unwrap();
    assert_eq!((again["path"].as_str(), again["copied"].as_bool()), (Some("overlay/Snow-001.mp4"), Some(false)));
    assert!(use_in_project(&env, &payload(json!({"id": "../escape.mp4", "project_folder": project.display().to_string()}))).is_err());
    // Delete.
    assert!(delete(&env, &payload(json!({"id": "../x"}))).is_err());
    assert_eq!(delete(&env, &payload(json!({"id": "snow/Snow-001.mp4"}))).unwrap()["deleted"], "snow/Snow-001.mp4");
    assert_eq!(list(&env).unwrap()["items"], json!([]));
    assert!(thumb_path(&env, "snow/Snow-001.mp4", "image").is_err());
    assert!(project.join("overlay").join("Snow-001.mp4").is_file(), "the project keeps its own copy");
    assert!(delete(&env, &payload(json!({"id": "snow/Snow-001.mp4"}))).is_err());
}

#[test]
fn the_library_of_an_empty_machine_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let reply = list(&env(dir.path())).unwrap();
    assert_eq!(reply["items"], json!([]));
    assert!(reply["categories"].as_array().unwrap().len() >= 6);
}

#[test]
fn a_preview_copy_is_made_once_and_only_for_clips_in_the_project_or_the_library() {
    use h3_builder::overlay_library::preview_proxy;
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let project = dir.path().join("My Song");
    clip(&tools, &project.join("overlay"), "Dust.mp4", 2);
    let project_text = project.display().to_string();
    let proxy = preview_proxy(&env, "overlay/Dust.mp4", &project_text).unwrap();
    assert!(proxy.is_file() && proxy.starts_with(project.join("overlay").join(".proxy")), "{}", proxy.display());
    let info = tools.ffprobe_checked(&["-v".into(), "error".into(), "-show_entries".into(), "stream=codec_type,codec_name,width".into(), "-of".into(), "json".into(), proxy.display().to_string()], "probe").unwrap();
    let info: Value = serde_json::from_slice(&info.stdout).unwrap();
    let streams = info["streams"].as_array().unwrap();
    assert!(streams.iter().all(|s| s["codec_type"] == "video"), "the preview copy has no sound");
    assert_eq!(streams[0]["codec_name"], "h264");
    assert!(streams[0]["width"].as_u64().unwrap() <= 960);
    let first = fs::metadata(&proxy).unwrap().modified().unwrap();
    assert_eq!(preview_proxy(&env, "overlay/Dust.mp4", &project_text).unwrap(), proxy);
    assert_eq!(fs::metadata(&proxy).unwrap().modified().unwrap(), first, "the copy is not made again");
    // A clip in the library works by its full path.
    let library_clip = clip(&tools, &env.overlay_library().join("dust"), "Library Dust.mp4", 2);
    assert!(preview_proxy(&env, &library_clip.display().to_string(), "").unwrap().is_file());
    // Nothing else can be read through it.
    let secret = clip(&tools, &dir.path().join("elsewhere"), "Secret.mp4", 1);
    for (path, folder) in [(secret.display().to_string(), project_text.clone()), ("../elsewhere/Secret.mp4".to_string(), project_text.clone()), ("overlay/missing.mp4".to_string(), project_text.clone()), (String::new(), project_text.clone()), ("overlay/Dust.mp4".to_string(), String::new())] {
        assert!(preview_proxy(&env, &path, &folder).is_err(), "{path:?} {folder:?}");
    }
    fs::write(project.join("overlay").join("notes.txt"), b"x").unwrap();
    assert!(preview_proxy(&env, "overlay/notes.txt", &project_text).unwrap_err().contains(".txt"));
}
