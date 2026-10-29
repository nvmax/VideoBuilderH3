//! The overlay pass in a real ffmpeg, on small synthetic clips: what it does inside a clip's time window, that it leaves the video alone
//! outside it, and that sound, size and length survive. Needs ffmpeg and ffprobe on `PATH` (the tests say so and stop when they are
//! missing).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use h3_media::fonts::{ensure_fonts, resolve_font};
use h3_media::overlay::render_overlay;
use h3_media::Tools;
use serde_json::{json, Value};

const W: usize = 320;
const H: usize = 180;

fn tools() -> Option<Tools> {
    let found = Tools::find("", "").ok();
    if found.is_none() {
        eprintln!("ffmpeg and ffprobe are not on PATH: overlay render tests skipped");
    }
    found
}

fn ffmpeg(tools: &Tools, args: &[&str]) {
    let out = Command::new(&tools.ffmpeg).args(["-y", "-v", "error"]).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// A 4 second 24 fps clip of one flat gray with a sine tone, 320x180.
fn base_clip(tools: &Tools, dir: &Path, gray: &str) -> PathBuf {
    let path = dir.join(format!("base_{gray}.mp4"));
    ffmpeg(
        tools,
        &["-f", "lavfi", "-i", &format!("color=c=0x{gray}{gray}{gray}:s={W}x{H}:r=24:d=4"), "-f", "lavfi", "-i", "sine=frequency=440:duration=4", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", &path.display().to_string()],
    );
    path
}

/// One second of black with a white box, 160x90: a stand-in for a dust or particle clip.
fn overlay_clip(tools: &Tools, dir: &Path) -> PathBuf {
    let path = dir.join("dust.mp4");
    ffmpeg(
        tools,
        &["-f", "lavfi", "-i", "color=c=black:s=160x90:r=24:d=1,drawbox=x=20:y=15:w=80:h=45:color=white:t=fill", "-c:v", "libx264", "-pix_fmt", "yuv420p", &path.display().to_string()],
    );
    path
}

/// A `.cube` file that inverts the picture.
fn invert_cube(dir: &Path) -> PathBuf {
    let path = dir.join("Invert.cube");
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                text.push_str(&format!("{} {} {}\n", 1 - r, 1 - g, 1 - b));
            }
        }
    }
    fs::write(&path, text).unwrap();
    path
}

/// The luma plane of the frame at `seconds`.
fn frame(tools: &Tools, video: &Path, seconds: f64) -> Vec<u8> {
    let out = Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-ss", &seconds.to_string(), "-i", &video.display().to_string(), "-frames:v", "1", "-pix_fmt", "yuv420p", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.len() >= W * H, "no frame at {seconds}");
    out.stdout[..W * H].to_vec()
}

fn region(luma: &[u8], x: usize, y: usize, w: usize, h: usize) -> Vec<f64> {
    let mut values = Vec::new();
    for row in y..y + h {
        for col in x..x + w {
            values.push(f64::from(luma[row * W + col]));
        }
    }
    values
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn spread(values: &[f64]) -> f64 {
    let m = mean(values);
    (values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}

fn render(tools: &Tools, input: &Path, output: &Path, clips: Value, luts: &Path) -> Value {
    let resolve = |name: &str| Some(luts.join(name)).filter(|p| p.is_file());
    let fonts = luts.join("fonts");
    ensure_fonts(&fonts).unwrap();
    let font = |name: &str| resolve_font(&fonts, name);
    render_overlay(tools, input, output, &clips, &resolve, &font, 18, "veryfast").unwrap()
}

/// The box of the dust clip, as it lands in the 320x180 frame.
fn box_region(luma: &[u8]) -> Vec<f64> {
    region(luma, 90, 50, 80, 50)
}

#[test]
fn a_media_clip_shows_only_inside_its_window_and_the_sound_stays() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let dust = overlay_clip(&tools, dir.path());
    let out = dir.path().join("out.mp4");
    let reply = render(&tools, &base, &out, json!([{"kind": "media", "id": "dust", "start": 1, "end": 3, "path": dust.display().to_string(), "opacity": 1, "fade_in": 0, "fade_out": 0}]), dir.path());
    assert_eq!(reply["changed"], true, "{reply}");
    assert_eq!(reply["applied"], json!(["dust"]));
    assert_eq!(reply["audio_kept"], true);
    let (before, inside, after) = (frame(&tools, &out, 0.5), frame(&tools, &out, 2.0), frame(&tools, &out, 3.5));
    let original = frame(&tools, &base, 2.0);
    assert!(mean(&box_region(&inside)) > mean(&box_region(&original)) + 100.0, "the box should be white inside the window");
    assert!(mean(&box_region(&before)) < mean(&box_region(&original)) + 6.0, "nothing before the window");
    assert!(mean(&box_region(&after)) < mean(&box_region(&original)) + 6.0, "nothing after the window");
    let probe = tools.ffprobe_checked(&["-v".into(), "error".into(), "-show_entries".into(), "stream=codec_type,width,height:format=duration".into(), "-of".into(), "json".into(), out.display().to_string()], "probe").unwrap();
    let info: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = info["streams"].as_array().unwrap();
    assert!(streams.iter().any(|s| s["codec_type"] == "audio"), "the audio was dropped");
    let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
    assert_eq!((video["width"].as_u64(), video["height"].as_u64()), (Some(320), Some(180)));
    let length: f64 = info["format"]["duration"].as_str().unwrap().parse().unwrap();
    assert!((length - 4.0).abs() < 0.15, "length {length}");
}

#[test]
fn a_media_clip_that_starts_at_zero_and_one_that_is_looped_both_work() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let dust = overlay_clip(&tools, dir.path());
    let out = dir.path().join("out.mp4");
    // The overlay is one second long and the clip runs three seconds, so it has to loop to still show at 2.5 seconds.
    let reply = render(&tools, &base, &out, json!([{"kind": "media", "id": "dust", "start": 0, "end": 3, "path": dust.display().to_string(), "opacity": 1, "fade_in": 0, "fade_out": 0, "loop": true}]), dir.path());
    assert_eq!(reply["changed"], true, "{reply}");
    let original = frame(&tools, &base, 2.5);
    assert!(mean(&box_region(&frame(&tools, &out, 0.1))) > mean(&box_region(&original)) + 100.0);
    assert!(mean(&box_region(&frame(&tools, &out, 2.5))) > mean(&box_region(&original)) + 100.0);
    assert!(mean(&box_region(&frame(&tools, &out, 3.5))) < mean(&box_region(&original)) + 6.0);
}

#[test]
fn darken_hard_light_color_dodge_and_color_burn_give_the_usual_result() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "80");
    let dust = overlay_clip(&tools, dir.path());
    let original = frame(&tools, &base, 2.0);
    let (flat, flat_box) = (mean(&region(&original, 220, 130, 80, 40)), mean(&box_region(&original)));
    // The dust clip is black around a white box. Per mode: what the black part does to a mid gray, then what the white box does.
    for (mode, black, white) in [("darken", "black", "same"), ("hardlight", "black", "white"), ("colordodge", "same", "white"), ("colorburn", "black", "same")] {
        let out = dir.path().join(format!("{mode}.mp4"));
        let clip = json!([{"kind": "media", "id": "dust", "start": 1, "end": 3, "path": dust.display().to_string(), "blend_mode": mode, "opacity": 1, "fade_in": 0, "fade_out": 0, "scale": "stretch"}]);
        let reply = render(&tools, &base, &out, clip, dir.path());
        assert_eq!(reply["applied"], json!(["dust"]), "{mode}: {reply}");
        let frame = frame(&tools, &out, 2.0);
        for (name, got, was, expect) in [("black part", mean(&region(&frame, 220, 130, 80, 40)), flat, black), ("white box", mean(&box_region(&frame)), flat_box, white)] {
            match expect {
                "same" => assert!((got - was).abs() < 8.0, "{mode} {name}: {got} should stay {was}"),
                "black" => assert!(got < was - 70.0, "{mode} {name}: {got} should go black from {was}"),
                _ => assert!(got > was + 70.0, "{mode} {name}: {got} should go white from {was}"),
            }
        }
    }
}

#[test]
fn color_dodge_and_color_burn_follow_the_opacity() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "80");
    let dust = overlay_clip(&tools, dir.path());
    let was = mean(&box_region(&frame(&tools, &base, 2.0)));
    // Half opacity puts the white box half way between the picture and full white (color dodge), the black part half way to black (color burn).
    for (mode, region_of, goal) in [("colordodge", box_region as fn(&[u8]) -> Vec<f64>, 235.0), ("colorburn", |luma: &[u8]| region(luma, 220, 130, 80, 40), 16.0)] {
        let out = dir.path().join(format!("{mode}_half.mp4"));
        let clip = json!([{"kind": "media", "id": "dust", "start": 1, "end": 3, "path": dust.display().to_string(), "blend_mode": mode, "opacity": 0.5, "fade_in": 0, "fade_out": 0, "scale": "stretch"}]);
        render(&tools, &base, &out, clip, dir.path());
        let got = mean(&region_of(&frame(&tools, &out, 2.0)));
        let was = if mode == "colordodge" { was } else { mean(&region(&frame(&tools, &base, 2.0), 220, 130, 80, 40)) };
        let middle = (was + goal) / 2.0;
        assert!((got - middle).abs() < 14.0, "{mode}: {got} should be near {middle} (picture {was}, full effect {goal})");
    }
}

#[test]
fn a_fade_in_ramps_the_effect_up() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let dust = overlay_clip(&tools, dir.path());
    let out = dir.path().join("out.mp4");
    render(&tools, &base, &out, json!([{"kind": "media", "id": "dust", "start": 1, "end": 4, "path": dust.display().to_string(), "opacity": 1, "fade_in": 1.2, "fade_out": 0}]), dir.path());
    let early = mean(&box_region(&frame(&tools, &out, 1.2)));
    let late = mean(&box_region(&frame(&tools, &out, 3.0)));
    let original = mean(&box_region(&frame(&tools, &base, 1.2)));
    assert!(early > original + 3.0 && early < late - 60.0, "early {early}, late {late}, original {original}");
}

#[test]
fn a_lut_clip_inverts_the_picture_inside_its_window_only() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "40");
    invert_cube(dir.path());
    let out = dir.path().join("out.mp4");
    let reply = render(&tools, &base, &out, json!([{"kind": "lut", "id": "inv", "start": 2, "end": 4, "lut": "Invert.cube", "opacity": 1, "strength": 1, "fade_in": 0, "fade_out": 0}]), dir.path());
    assert_eq!(reply["changed"], true, "{reply}");
    let spot = |luma: &[u8]| mean(&region(luma, 100, 60, 40, 40));
    let (original, before, inside) = (spot(&frame(&tools, &base, 3.0)), spot(&frame(&tools, &out, 1.0)), spot(&frame(&tools, &out, 3.0)));
    assert!((before - original).abs() < 4.0, "before the window the picture is unchanged: {before} vs {original}");
    // Limited range luma runs 16 to 235, so an inverted level is 251 minus the original one.
    assert!((inside - (251.0 - original)).abs() < 10.0, "inside the window: {inside}, original {original}");
}

#[test]
fn a_vignette_darkens_the_corners_and_not_the_middle() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "80");
    let out = dir.path().join("out.mp4");
    render(&tools, &base, &out, json!([{"kind": "vignette", "id": "v", "start": 0, "end": 4, "strength": 1, "fade_in": 0, "fade_out": 0}]), dir.path());
    let picture = frame(&tools, &out, 2.0);
    let original = frame(&tools, &base, 2.0);
    let corner = mean(&region(&picture, 0, 0, 16, 16));
    let middle = mean(&region(&picture, 140, 80, 40, 20));
    assert!(corner < mean(&region(&original, 0, 0, 16, 16)) * 0.7, "corner {corner}");
    assert!((middle - mean(&region(&original, 140, 80, 40, 20))).abs() < 8.0, "middle {middle}");
}

#[test]
fn grain_adds_noise_inside_its_window_only_and_it_is_not_green() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "80");
    let out = dir.path().join("out.mp4");
    render(&tools, &base, &out, json!([{"kind": "grain", "id": "g", "start": 1, "end": 3, "strength": 45, "fade_in": 0, "fade_out": 0}]), dir.path());
    let flat = |luma: &[u8]| spread(&region(luma, 100, 60, 100, 60));
    let (before, inside, after) = (flat(&frame(&tools, &out, 0.5)), flat(&frame(&tools, &out, 2.0)), flat(&frame(&tools, &out, 3.5)));
    assert!(before < 1.5 && after < 1.5, "no grain outside the window: {before}, {after}");
    assert!(inside > 4.0, "grain inside the window: {inside}");
    // Grain is made on the luma, so the colors of a gray picture stay gray. Compare the two chroma planes with their neutral value.
    let raw = Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-ss", "2.0", "-i", &out.display().to_string(), "-frames:v", "1", "-pix_fmt", "yuv420p", "-f", "rawvideo", "-"])
        .output()
        .unwrap()
        .stdout;
    let (cb, cr) = (&raw[W * H..W * H + W * H / 4], &raw[W * H + W * H / 4..W * H + W * H / 2]);
    for plane in [cb, cr] {
        let values: Vec<f64> = plane.iter().map(|v| f64::from(*v)).collect();
        assert!((mean(&values) - 128.0).abs() < 3.0, "the chroma moved: {}", mean(&values));
    }
}

#[test]
fn coarse_grain_also_adds_noise() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "80");
    let out = dir.path().join("out.mp4");
    render(&tools, &base, &out, json!([{"kind": "grain", "id": "g", "start": 0, "end": 4, "size": "coarse", "strength": 60, "fade_in": 0, "fade_out": 0}]), dir.path());
    assert!(spread(&region(&frame(&tools, &out, 2.0), 100, 60, 100, 60)) > 3.0);
}

#[test]
fn clips_that_cannot_be_applied_write_nothing_and_say_why() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "40");
    let out = dir.path().join("out.mp4");
    let reply = render(&tools, &base, &out, json!([{"kind": "media", "id": "gone", "start": 0, "end": 2, "path": "Z:/nowhere/dust.mov"}, {"kind": "text", "id": "title", "start": 0, "end": 2, "text": "  "}]), dir.path());
    assert_eq!(reply["changed"], false);
    assert_eq!(reply["applied"], json!([]));
    assert_eq!(reply["skipped"].as_array().unwrap().len(), 2);
    assert!(!out.exists());
}

#[test]
fn every_kind_together_still_renders() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "40");
    let dust = overlay_clip(&tools, dir.path());
    invert_cube(dir.path());
    let out = dir.path().join("out.mp4");
    let reply = render(
        &tools,
        &base,
        &out,
        json!([
            {"kind": "media", "id": "dust", "start": 0.5, "end": 3.5, "path": dust.display().to_string(), "speed": 1.5, "flip_h": true, "tint": "#ffaa00", "brightness": 0.1, "contrast": 1.2, "blend_mode": "overlay", "scale": "contain"},
            {"kind": "grain", "id": "g1", "start": 0, "end": 4, "size": "medium"},
            {"kind": "lut", "id": "lut", "start": 1, "end": 3, "lut": "Invert.cube", "strength": 0.5},
            {"kind": "vignette", "id": "v", "start": 0, "end": 4},
        ]),
        dir.path(),
    );
    assert_eq!(reply["changed"], true, "{reply}");
    assert_eq!(reply["applied"], json!(["dust", "g1", "lut", "v"]));
    assert!(out.is_file() && fs::metadata(&out).unwrap().len() > 1000);
}

#[test]
fn a_graph_too_long_for_the_command_line_goes_through_a_file() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "80");
    let out = dir.path().join("out.mp4");
    // 80 vignette clips make a graph of more than 20,000 characters.
    let clips: Vec<Value> = (0..80).map(|i| json!({"kind": "vignette", "id": format!("v{i}"), "start": 0, "end": 4, "strength": 0.05, "fade_in": 0, "fade_out": 0})).collect();
    let reply = render(&tools, &base, &out, Value::Array(clips), dir.path());
    assert_eq!(reply["changed"], true, "{reply}");
    assert_eq!(reply["applied"].as_array().unwrap().len(), 80);
    assert!(out.is_file());
}

#[test]
fn grain_and_vignette_fades_ramp_up() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "80");
    let out = dir.path().join("out.mp4");
    render(
        &tools,
        &base,
        &out,
        json!([
            {"kind": "grain", "id": "g", "start": 0, "end": 4, "strength": 60, "fade_in": 1.8, "fade_out": 0},
            {"kind": "vignette", "id": "v", "start": 0, "end": 4, "strength": 1, "fade_in": 1.8, "fade_out": 0},
        ]),
        dir.path(),
    );
    let grain = |t: f64| spread(&region(&frame(&tools, &out, t), 100, 60, 100, 60));
    let corner = |t: f64| mean(&region(&frame(&tools, &out, t), 0, 0, 16, 16));
    assert!(grain(0.1) < grain(3.0) - 2.0, "grain should grow: {} then {}", grain(0.1), grain(3.0));
    assert!(corner(0.1) > corner(3.0) + 20.0, "the corner should darken: {} then {}", corner(0.1), corner(3.0));
    // And the fade out at the end of a clip.
    let out2 = dir.path().join("out2.mp4");
    render(&tools, &base, &out2, json!([{"kind": "vignette", "id": "v", "start": 0, "end": 4, "strength": 1, "fade_in": 0, "fade_out": 1.8}]), dir.path());
    let corner2 = |t: f64| mean(&region(&frame(&tools, &out2, t), 0, 0, 16, 16));
    assert!(corner2(3.8) > corner2(1.0) + 20.0, "the corner should lighten again: {} then {}", corner2(1.0), corner2(3.8));
}

#[test]
fn many_clips_render_in_a_reasonable_time() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "40");
    let dust = overlay_clip(&tools, dir.path());
    let out = dir.path().join("out.mp4");
    // Twelve dust clips, twelve grain clips and a vignette: the chain used to double its cost with every clip.
    let mut clips: Vec<Value> = (0..12).map(|i| json!({"kind": "media", "id": format!("d{i}"), "start": f64::from(i) * 0.3, "end": f64::from(i) * 0.3 + 1.0, "path": dust.display().to_string()})).collect();
    clips.extend((0..12).map(|i| json!({"kind": "grain", "id": format!("g{i}"), "start": f64::from(i) * 0.3, "end": f64::from(i) * 0.3 + 1.0})));
    clips.push(json!({"kind": "vignette", "id": "v", "start": 0, "end": 4}));
    let reply = render(&tools, &base, &out, Value::Array(clips), dir.path());
    assert_eq!(reply["applied"].as_array().unwrap().len(), 25, "{reply}");
    let seconds = reply["elapsed_seconds"].as_f64().unwrap();
    assert!(seconds < 20.0, "25 clips took {seconds} s");
}

fn frame_luma(png: &[u8]) -> Vec<u8> {
    let picture = image::load_from_memory(png).unwrap().to_luma8();
    assert_eq!((picture.width() as usize, picture.height() as usize), (W, H));
    picture.into_raw()
}

#[test]
fn one_frame_is_the_picture_the_render_makes_at_that_moment() {
    use h3_core::overlay_layers::shift_overlay_layers;
    use h3_media::overlay::render_overlay_frame;
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let dust = overlay_clip(&tools, dir.path());
    let resolve = |_: &str| None;
    // Timeline clips: the dust shows from 1 s to 3 s of the timeline, and this video starts at 0 on the timeline.
    let timeline = json!([{"kind": "media", "id": "dust", "start": 1, "end": 3, "path": dust.display().to_string(), "opacity": 1, "fade_in": 0, "fade_out": 0}]);
    let mut seen = Vec::new();
    for seek in [0.5, 2.0, 3.5] {
        let clips = shift_overlay_layers(&timeline, seek);
        let (reply, png) = render_overlay_frame(&tools, &base, seek, &clips, &resolve, &|_: &str| None).unwrap();
        assert_eq!(&png[1..4], b"PNG", "the answer is a PNG picture");
        seen.push((mean(&box_region(&frame_luma(&png))), reply["applied"].as_array().unwrap().len()));
    }
    assert!(seen[0].0 < 90.0 && seen[2].0 < 90.0, "no dust before or after its window: {seen:?}");
    assert!(seen[1].0 > 150.0 && seen[1].1 == 1, "dust inside its window: {seen:?}");
}

#[test]
fn a_frame_of_a_looped_clip_that_began_long_before_starts_inside_its_file() {
    use h3_core::overlay_layers::shift_overlay_layers;
    use h3_media::overlay::render_overlay_frame;
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let dust = overlay_clip(&tools, dir.path());
    // A one second file, in a clip that began 2.5 s before this frame: the frame is 0.5 s into the second loop, not past the end of the file.
    let timeline = json!([{"kind": "media", "id": "dust", "start": 0, "end": 4, "path": dust.display().to_string(), "opacity": 1, "fade_in": 0, "fade_out": 0, "loop": true}]);
    let clips = shift_overlay_layers(&timeline, 2.5);
    assert_eq!(clips[0]["source_start"], 2.5, "the shift says how far into the file the clip already is");
    let (_, png) = render_overlay_frame(&tools, &base, 2.5, &clips, &|_: &str| None, &|_: &str| None).unwrap();
    assert!(mean(&box_region(&frame_luma(&png))) > 150.0, "the dust is there");
}

/// How many pixels of the middle of the frame are bright, and where their center is (row).
fn bright(luma: &[u8]) -> (usize, f64) {
    let (mut count, mut rows) = (0usize, 0.0);
    for y in 30..150 {
        for x in 30..290 {
            if luma[y * W + x] > 150 {
                count += 1;
                rows += y as f64;
            }
        }
    }
    (count, if count > 0 { rows / count as f64 } else { 0.0 })
}

#[test]
fn text_shows_only_inside_its_window_with_the_chosen_look() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let out = dir.path().join("out.mp4");
    let reply = render(&tools, &base, &out, json!([{"kind": "text", "id": "title", "start": 1, "end": 3, "text": "VERSE", "font": "Anton", "size": 150, "fade_in": 0, "fade_out": 0}]), dir.path());
    assert_eq!(reply["applied"], json!(["title"]), "{reply}");
    let (before, inside, after) = (bright(&frame(&tools, &out, 0.5)).0, bright(&frame(&tools, &out, 2.0)).0, bright(&frame(&tools, &out, 3.5)).0);
    assert!(before < 20 && after < 20, "no text outside its window: {before}, {after}");
    assert!(inside > 400, "the word is drawn in white inside the window: {inside} bright pixels");
}

#[test]
fn text_is_drawn_in_fonts_installed_on_the_computer() {
    let Some(tools) = tools() else { return };
    let installed = h3_media::fonts::system_fonts(false);
    if installed.is_empty() {
        eprintln!("no fonts are installed on this computer: installed font test skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    // The first, the middle and the last of the list, so a few different families are tried.
    for font in [&installed[0], &installed[installed.len() / 2], &installed[installed.len() - 1]] {
        let out = dir.path().join("installed.mp4");
        let reply = render(&tools, &base, &out, json!([{"kind": "text", "id": "t", "start": 0, "end": 4, "text": "VERSE", "font": font.name, "size": 150, "fade_in": 0, "fade_out": 0}]), dir.path());
        assert_eq!(reply["applied"], json!(["t"]), "{}: {reply}", font.name);
        assert!(bright(&frame(&tools, &out, 2.0)).0 > 300, "{} is drawn", font.name);
    }
}

#[test]
fn text_with_quotes_colons_percent_signs_backslashes_and_other_scripts_is_drawn_as_it_is() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let out = dir.path().join("out.mp4");
    let awkward = "It's 100%: a \\ lyric, with 'quotes' and %{pts} and \u{65e5}\u{672c}\nsecond line";
    let reply = render(&tools, &base, &out, json!([{"kind": "text", "id": "lyric", "start": 0, "end": 4, "text": awkward, "size": 80, "fade_in": 0, "fade_out": 0}]), dir.path());
    assert_eq!(reply["changed"], true, "{reply}");
    assert!(bright(&frame(&tools, &out, 2.0)).0 > 300, "the text is there");
}

#[test]
fn text_fades_in_and_slides_up_while_it_does() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let out = dir.path().join("out.mp4");
    render(&tools, &base, &out, json!([{"kind": "text", "id": "t", "start": 0.5, "end": 4, "text": "RISE", "size": 140, "y": 0.5, "animation": "slide_up", "fade_in": 1.5, "fade_out": 0}]), dir.path());
    let early = bright(&frame(&tools, &out, 0.9));
    let late = bright(&frame(&tools, &out, 3.0));
    assert!(early.0 < late.0 / 2, "it fades in: {early:?} then {late:?}");
    let mid = bright(&frame(&tools, &out, 1.5));
    assert!(mid.0 > 0 && late.0 > 0 && mid.1 > late.1 + 1.0, "it moves up while it fades in: row {} then {}", mid.1, late.1);
}

#[test]
fn a_font_that_is_not_there_falls_back_to_the_default_font() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let base = base_clip(&tools, dir.path(), "30");
    let out = dir.path().join("out.mp4");
    let reply = render(&tools, &base, &out, json!([{"kind": "text", "id": "t", "start": 0, "end": 4, "text": "FALL BACK", "font": "No Such Font", "size": 120, "fade_in": 0, "fade_out": 0}]), dir.path());
    assert_eq!(reply["applied"], json!(["t"]), "{reply}");
    assert!(bright(&frame(&tools, &out, 2.0)).0 > 500);
}
