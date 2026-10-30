//! Choosing what to import from a folder of effect packs, with real files: what is listed, what is left out, the matte joined into a
//! transparent clip, the list of what was imported before, and a plain folder of videos. Needs ffmpeg and ffprobe on `PATH` (the tests say
//! so and stop when they are missing).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use h3_builder::overlay_library::{delete, list};
use h3_builder::pack_import::{image_path, import, preview_path, scan};
use h3_builder::Env;
use h3_core::pyval::Obj;
use h3_media::Tools;
use serde_json::{json, Value};

fn tools() -> Option<Tools> {
    let found = Tools::find("", "").ok();
    if found.is_none() {
        eprintln!("ffmpeg and ffprobe are not on PATH: pack import tests skipped");
    }
    found
}

fn env(dir: &Path) -> Env {
    Env { output: dir.join("output"), roots: vec![], ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), luts: dir.join("config").join("luts"), overlay_dir: None }
}

fn payload(value: Value) -> Obj {
    value.as_object().unwrap().clone()
}

fn make(tools: &Tools, path: &Path, source: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let out = Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", source, "-c:v", "libx264", "-pix_fmt", "yuv420p"]).arg(path).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// Black with a white box, 2 seconds.
fn clip(tools: &Tools, path: &Path) {
    make(tools, path, "color=c=black:s=160x90:r=24:d=2,drawbox=x=20:y=15:w=60:h=40:color=white:t=fill");
}

/// A matte: black with a green box where the clip is to show.
fn matte(tools: &Tools, path: &Path) {
    make(tools, path, "color=c=black:s=160x90:r=24:d=2,drawbox=x=20:y=15:w=60:h=40:color=0x00FF00:t=fill");
}

/// A pack the way Filmora lays it out: the picture in the pack folder and `Data` with the XML and the clips.
fn pack(folder: &Path, name: &str, transition: u8, blend: Option<u32>, video: &str, matte_file: Option<&str>) -> PathBuf {
    let data = folder.join("Data");
    fs::create_dir_all(&data).unwrap();
    fs::write(folder.join(format!("{name}.png")), b"\x89PNG\r\n\x1a\n").unwrap();
    let blend = blend.map(|b| format!("<BlendType>{b}</BlendType>")).unwrap_or_default();
    let ml = matte_file.map(|_| format!("<MultiLayer>{name}.ml</MultiLayer>")).unwrap_or_default();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Filter>\n <GUID>{guid}</GUID>\n <Name>{name}</Name>\n <Transition>{transition}</Transition>\n <VideoOverlay>\n  <VideoFile>{video}</VideoFile>\n  {blend}\n  {ml}\n </VideoOverlay>\n</Filter>\n",
        guid = format!("GUID-{}", name.replace(' ', "-"))
    );
    fs::write(data.join("filter.xml"), xml).unwrap();
    if let Some(matte_file) = matte_file {
        fs::write(data.join(format!("{name}.ml")), format!("<?xml version=\"1.0\"?>\n<Layers>\n <Layer File = \"{matte_file}\" AlphaLayer=\"1\" BlendType=\"0\"/>\n</Layers>\n")).unwrap();
    }
    data
}

/// A folder like the downloaded one: every kind of pack, one level down.
fn packs(tools: &Tools, root: &Path) {
    let base = root.join("Effect Packs");
    let data = pack(&base.join("4_Dust_Overlay_1"), "Dust Overlay 1", 0, Some(0), "Dust Overlay 1.mp4", Some("Dust Overlay 1_L0.mp4"));
    clip(tools, &data.join("Dust Overlay 1.mp4"));
    matte(tools, &data.join("Dust Overlay 1_L0.mp4"));
    let data = pack(&base.join("4_Glow_Overlay_2"), "Glow Overlay 2", 0, Some(8), "Glow Overlay 2.mp4", None);
    clip(tools, &data.join("Glow Overlay 2.mp4"));
    let data = pack(&base.join("4_Grain_Overlay_3"), "Grain Overlay 3", 0, Some(9), "Grain Overlay 3.mp4", None);
    clip(tools, &data.join("Grain Overlay 3.mp4"));
    // Left out: a blend this app does not know, a clip in a format it does not read, a transition, a title.
    let data = pack(&base.join("4_Key_Overlay_4"), "Key Overlay 4", 0, Some(6), "Key Overlay 4.mp4", None);
    clip(tools, &data.join("Key Overlay 4.mp4"));
    let data = pack(&base.join("4_Old_Overlay_5"), "Old Overlay 5", 0, Some(8), "Old Overlay 5.flv", None);
    fs::write(data.join("Old Overlay 5.flv"), b"FLV").unwrap();
    let data = pack(&base.join("2_Wipe_Transition_1"), "Wipe Transition 1", 1, Some(2), "Wipe Transition 1.mp4", None);
    clip(tools, &data.join("Wipe Transition 1.mp4"));
    let data = pack(&base.join("1_Big_Title_1"), "Big Title 1", 0, Some(0), "Big Title 1.mp4", None);
    clip(tools, &data.join("Big Title 1.mp4"));
}

fn names(reply: &Value, key: &str) -> Vec<String> {
    reply[key].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect()
}

/// The alpha channel of one pixel of the clip at half a second.
fn alpha_at(tools: &Tools, clip: &Path, x: usize, y: usize) -> u8 {
    let out = Command::new(&tools.ffmpeg).args(["-v", "error", "-ss", "0.5", "-i"]).arg(clip).args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout[(y * 160 + x) * 4 + 3]
}

#[test]
fn only_the_overlay_packs_this_app_can_use_are_listed_and_the_rest_is_counted() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    packs(&tools, &dir.path().join("downloads"));
    let reply = scan(&env, &payload(json!({"path": dir.path().join("downloads").display().to_string()}))).unwrap();
    assert_eq!(reply["mode"], "packs");
    assert_eq!(names(&reply, "packs"), ["Dust Overlay 1", "Glow Overlay 2", "Grain Overlay 3"], "{reply}");
    assert_eq!((reply["unsupported"].clone(), reply["not_overlays"].clone(), reply["hidden"].clone()), (json!(2), json!(2), json!(0)));
    let first = &reply["packs"][0];
    assert_eq!(first["id"], "guid-dust-overlay-1", "the pack's own id, in lower case");
    assert!(first["note"].as_str().unwrap().contains("matte"), "{first}");
    assert!(reply["packs"][1]["note"].as_str().unwrap().contains("screen"));
    assert!(reply["packs"][2]["note"].as_str().unwrap().contains("overlay"));
    assert!(first["size_bytes"].as_u64().unwrap() > 0);
    let url = first["image_url"].as_str().unwrap();
    assert!(url.starts_with("/vrgdg/music_builder/overlay_library/packs/image?root=") && url.contains("4_Dust_Overlay_1"), "{url}");
    let previews: Vec<&str> = reply["packs"].as_array().unwrap().iter().map(|p| p["preview_url"].as_str().unwrap()).collect();
    assert!(previews.iter().all(|u| u.starts_with("/vrgdg/music_builder/overlay_library/packs/preview?root=")), "{previews:?}");
    assert!(previews[0].contains("Dust%20Overlay%201.mp4") && previews[0].contains("_L0.mp4") && previews[0].ends_with("blend=normal"), "a pack with a matte previews as a transparent clip: {}", previews[0]);
    assert!(previews[1].contains("&matte=&") && previews[1].ends_with("blend=screen"), "{}", previews[1]);
    assert!(previews[2].ends_with("blend=overlay"), "{}", previews[2]);
    assert!(list(&env).unwrap()["items"].as_array().unwrap().is_empty(), "listing copies nothing");
}

#[test]
fn the_chosen_packs_are_imported_a_matte_joins_its_clip_and_what_is_left_is_not_copied() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let root = dir.path().join("downloads");
    packs(&tools, &root);
    let path = root.display().to_string();
    let reply = import(&env, &payload(json!({"path": path, "ids": ["guid-dust-overlay-1", "guid-glow-overlay-2"]}))).unwrap();
    assert_eq!(names(&reply, "added"), ["Dust Overlay 1", "Glow Overlay 2"], "{reply}");
    assert!(reply["failed"].as_array().unwrap().is_empty());
    let items = reply["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "only the two chosen packs are in the library: {items:?}");
    let joined = items.iter().find(|i| i["name"] == "Dust Overlay 1").unwrap();
    assert_eq!((joined["has_alpha"].clone(), joined["suggested_blend"].clone()), (json!(true), json!("normal")));
    let file = Path::new(joined["path"].as_str().unwrap());
    assert!(alpha_at(&tools, file, 5, 5) < 10, "transparent where the matte is black");
    assert!(alpha_at(&tools, file, 50, 35) > 245, "visible where the matte is green");
    let plain = items.iter().find(|i| i["name"] == "Glow Overlay 2").unwrap();
    assert_eq!((plain["has_alpha"].clone(), plain["suggested_blend"].clone()), (json!(false), json!("screen")));
    assert!(root.join("Effect Packs").join("4_Dust_Overlay_1").join("Data").join("Dust Overlay 1.mp4").is_file(), "the download is copied, not moved");
    assert!(!env.overlay_library().join(".tmp").exists() || fs::read_dir(env.overlay_library().join(".tmp")).unwrap().next().is_none(), "the work folder is cleared");
}

#[test]
fn a_pack_that_was_imported_is_not_listed_again_until_its_clip_is_deleted() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let root = dir.path().join("downloads");
    packs(&tools, &root);
    let path = root.display().to_string();
    import(&env, &payload(json!({"path": path, "ids": ["guid-glow-overlay-2"]}))).unwrap();
    assert!(env.overlay_library().join(".imported_packs.json").is_file());
    let again = scan(&env, &payload(json!({"path": path}))).unwrap();
    assert_eq!(names(&again, "packs"), ["Dust Overlay 1", "Grain Overlay 3"], "{again}");
    assert_eq!(again["hidden"], 1);
    // Delete the clip from the library and the pack can be chosen again.
    let id = list(&env).unwrap()["items"][0]["id"].as_str().unwrap().to_string();
    delete(&env, &payload(json!({"id": id}))).unwrap();
    let back = scan(&env, &payload(json!({"path": path}))).unwrap();
    assert_eq!(names(&back, "packs"), ["Dust Overlay 1", "Glow Overlay 2", "Grain Overlay 3"]);
    assert_eq!(back["hidden"], 0);
}

#[test]
fn a_pack_that_is_not_in_the_folder_fails_alone_and_nothing_is_imported_without_a_choice() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let root = dir.path().join("downloads");
    packs(&tools, &root);
    let path = root.display().to_string();
    let reply = import(&env, &payload(json!({"path": path, "ids": ["no-such-pack", "guid-grain-overlay-3", "guid-key-overlay-4"]}))).unwrap();
    assert_eq!(names(&reply, "added"), ["Grain Overlay 3"]);
    assert_eq!(names(&reply, "failed"), ["no-such-pack", "Key Overlay 4"], "{reply}");
    assert_eq!(reply["failed"][1]["error"], "it needs a blend this app does not know yet", "a pack that is left out of the list cannot be imported by its id either");
    assert_eq!(reply["items"].as_array().unwrap().len(), 1, "nothing else was copied");
    assert_eq!(import(&env, &payload(json!({"path": path, "ids": []}))).unwrap_err(), "Choose at least one to import.");
    assert!(scan(&env, &payload(json!({"path": dir.path().join("nowhere").display().to_string()}))).is_err());
}

#[test]
fn a_folder_without_packs_lists_its_videos_and_remembers_the_ones_imported() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let root = dir.path().join("downloads");
    clip(&tools, &root.join("Smoke Pack").join("Smoke 1.mp4"));
    clip(&tools, &root.join("Dust 2.mp4"));
    let path = root.display().to_string();
    let reply = scan(&env, &payload(json!({"path": path}))).unwrap();
    assert_eq!(reply["mode"], "files");
    let listed: Vec<&str> = reply["files"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
    assert_eq!(listed, ["Dust 2.mp4", "Smoke Pack/Smoke 1.mp4"], "{reply}");
    let done = import(&env, &payload(json!({"path": path, "ids": ["Smoke Pack/Smoke 1.mp4", "../escape.mp4"]}))).unwrap();
    assert_eq!(done["added"][0]["category"], "smoke");
    assert_eq!(done["failed"][0]["error"], "That file is not in the folder.");
    let after = scan(&env, &payload(json!({"path": path}))).unwrap();
    assert_eq!((after["files"].as_array().unwrap().len(), after["hidden"].clone()), (1, json!(1)));
}

/// Codec, width and height of a preview.
fn shape(tools: &Tools, clip: &Path) -> (String, u64, u64) {
    let out = Command::new(&tools.ffprobe).args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height", "-of", "json"]).arg(clip).output().unwrap();
    let stream = serde_json::from_slice::<Value>(&out.stdout).unwrap()["streams"][0].clone();
    (stream["codec_name"].as_str().unwrap().to_string(), stream["width"].as_u64().unwrap(), stream["height"].as_u64().unwrap())
}

/// The green channel of one pixel of a 384 pixel wide preview, one second in.
fn grey_at(tools: &Tools, clip: &Path, x: usize, y: usize) -> u8 {
    let out = Command::new(&tools.ffmpeg).args(["-v", "error", "-ss", "1", "-i"]).arg(clip).args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout[(y * 384 + x) * 3 + 1]
}

#[test]
fn a_preview_is_a_small_clip_with_the_pack_laid_over_a_backdrop_and_it_is_kept() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let root = dir.path().join("downloads");
    packs(&tools, &root);
    let root_text = root.display().to_string();
    let dust = "Effect Packs/4_Dust_Overlay_1/Data";
    let matted = preview_path(&env, &root_text, &format!("{dust}/Dust Overlay 1.mp4"), &format!("{dust}/Dust Overlay 1_L0.mp4"), "normal").unwrap();
    let screened = preview_path(&env, &root_text, "Effect Packs/4_Glow_Overlay_2/Data/Glow Overlay 2.mp4", "", "screen").unwrap();
    let grain = preview_path(&env, &root_text, "Effect Packs/4_Grain_Overlay_3/Data/Grain Overlay 3.mp4", "", "overlay").unwrap();
    for file in [&matted, &screened, &grain] {
        assert_eq!(shape(&tools, file), ("h264".to_string(), 384, 216), "{}", file.display());
    }
    // Inside the box the clip shows (white). Outside it the backdrop shows: a matte keeps it, and so does black blended with screen.
    assert!(grey_at(&tools, &matted, 120, 84) > 225, "the clip shows where the matte is green");
    assert!(grey_at(&tools, &screened, 120, 84) > 225, "a screen blend shows the white box");
    let (a, b) = (grey_at(&tools, &matted, 300, 180), grey_at(&tools, &screened, 300, 180));
    assert!((15..140).contains(&a), "the backdrop shows outside the box: {a}");
    assert!(a.abs_diff(b) < 12, "black blended with screen leaves the picture as it is: {a} and {b}");
    // The same request makes nothing again.
    let stamp = fs::metadata(&matted).unwrap().modified().unwrap();
    assert_eq!(preview_path(&env, &root_text, &format!("{dust}/Dust Overlay 1.mp4"), &format!("{dust}/Dust Overlay 1_L0.mp4"), "normal").unwrap(), matted);
    assert_eq!(fs::metadata(&matted).unwrap().modified().unwrap(), stamp);
    for file in [matted, screened, grain] {
        let _ = fs::remove_file(file);
    }
}

#[test]
fn a_preview_is_made_only_from_videos_inside_the_scanned_folder() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let root = dir.path().join("downloads");
    packs(&tools, &root);
    clip(&tools, &dir.path().join("secret.mp4"));
    let root_text = root.display().to_string();
    let glow = "Effect Packs/4_Glow_Overlay_2/Data/Glow Overlay 2.mp4";
    assert_eq!(preview_path(&env, &root_text, "../secret.mp4", "", "screen").unwrap_err(), "That clip is not in the folder.");
    assert_eq!(preview_path(&env, &root_text, &dir.path().join("secret.mp4").display().to_string(), "", "screen").unwrap_err(), "That clip is not in the folder.");
    assert_eq!(preview_path(&env, &root_text, "Effect Packs/4_Glow_Overlay_2/Data/filter.xml", "", "screen").unwrap_err(), "That clip is not in the folder.", "not a file that is not a video");
    assert_eq!(preview_path(&env, &root_text, "Effect Packs/4_Glow_Overlay_2/Data/missing.mp4", "", "screen").unwrap_err(), "That clip is not in the folder.");
    assert_eq!(preview_path(&env, &root_text, glow, "../secret.mp4", "normal").unwrap_err(), "That matte is not in the folder.");
    assert_eq!(preview_path(&env, &root_text, glow, "", "dodge").unwrap_err(), "That blend is not known.");
    assert!(preview_path(&env, &dir.path().join("nowhere").display().to_string(), glow, "", "screen").is_err());
}

#[test]
fn a_folder_without_packs_gives_each_video_a_preview_as_it_is() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let root = dir.path().join("downloads");
    clip(&tools, &root.join("Smoke Pack").join("Smoke 1.mp4"));
    let reply = scan(&env, &payload(json!({"path": root.display().to_string()}))).unwrap();
    let url = reply["files"][0]["preview_url"].as_str().unwrap();
    assert!(url.contains("Smoke%20Pack") && url.ends_with("blend=normal"), "{url}");
    let made = preview_path(&env, &root.display().to_string(), "Smoke Pack/Smoke 1.mp4", "", "normal").unwrap();
    assert_eq!(shape(&tools, &made), ("h264".to_string(), 384, 216));
    let _ = fs::remove_file(made);
}

#[test]
fn a_picture_is_served_only_from_inside_the_scanned_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("downloads");
    let data = pack(&root.join("4_Dust_Overlay_1"), "Dust Overlay 1", 0, Some(8), "x.mp4", None);
    fs::write(dir.path().join("secret.png"), b"x").unwrap();
    fs::write(data.join("filter.xml.txt"), b"x").unwrap();
    let root_text = root.display().to_string();
    assert!(image_path(&root_text, "4_Dust_Overlay_1/Dust Overlay 1.png").unwrap().is_file());
    assert!(image_path(&root_text, "../secret.png").is_err(), "not outside the folder");
    assert!(image_path(&root_text, "4_Dust_Overlay_1/Data/filter.xml.txt").is_err(), "not a file that is not a picture");
    assert!(image_path(&root_text, "4_Dust_Overlay_1/missing.png").is_err());
}
