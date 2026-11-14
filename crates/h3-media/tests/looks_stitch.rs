//! Effects and filters on a stitched video: which frames they cover, in which order they are applied, and that a stitch keeps the timeline's frames.
//! The stitch tests need ffmpeg and ffprobe on PATH and a graphics adapter.

use std::path::Path;

use h3_media::looks::{block_state, items_from_json, plain_parts, plan, segments, Block, LookItem, Part};
use h3_media::stitch::{stitch_scene_videos, StitchRequest, TimingItem};
use h3_media::tools::{args, run, Tools};
use h3_transitions::Kind;
use serde_json::json;

fn item(kind: Kind, name: &str, start: f64, seconds: f64) -> LookItem {
    LookItem { kind, name: name.into(), params: json!({}), start, seconds, intensity: 1.0 }
}

fn filter(name: &str, start: f64, seconds: f64) -> LookItem {
    item(Kind::Filter, name, start, seconds)
}

fn effect(name: &str, start: f64, seconds: f64) -> LookItem {
    item(Kind::Effect, name, start, seconds)
}

#[test]
fn a_block_covers_the_frames_it_is_asked_for() {
    let (blocks, notes) = plan(&[effect("Negative", 1.0, 0.5)], 96, 24);
    assert_eq!(blocks, vec![Block { item: 0, first: 24, last: 36 }]);
    assert!(notes.is_empty());
}

#[test]
fn filters_are_applied_before_effects_each_in_the_order_of_the_timeline() {
    let items = [effect("Glow", 1.0, 2.0), filter("Sepia", 2.0, 1.0), effect("Negative", 0.5, 1.0), filter("TealAndOrange", 0.0, 4.0)];
    let (blocks, _) = plan(&items, 96, 24);
    let order: Vec<&str> = blocks.iter().map(|b| items[b.item].name.as_str()).collect();
    assert_eq!(order, vec!["TealAndOrange", "Sepia", "Negative", "Glow"]);
}

#[test]
fn a_block_past_the_end_is_cut_short_or_left_out() {
    let (blocks, notes) = plan(&[effect("Negative", 3.5, 2.0), effect("Glow", 4.0, 1.0), effect("Flash", 9.0, 1.0)], 96, 24);
    assert_eq!(blocks, vec![Block { item: 0, first: 84, last: 96 }]);
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert!(notes[0].contains("shortened") && notes[1].contains("left out") && notes[2].contains("left out"));
}

#[test]
fn the_video_is_cut_where_blocks_start_and_end_and_plain_stretches_stay_whole() {
    let items = [filter("Sepia", 0.5, 1.5), effect("Negative", 1.0, 1.5)];
    let (blocks, _) = plan(&items, 120, 24);
    let cut = segments(&blocks, 120);
    let shape: Vec<(i64, i64, Vec<usize>)> = cut.iter().map(|s| (s.from, s.to, s.blocks.clone())).collect();
    // The filter is frames 12..48, the effect 24..60: nothing, filter, filter and effect, effect, nothing.
    assert_eq!(shape, vec![(0, 12, vec![]), (12, 24, vec![0]), (24, 48, vec![0, 1]), (48, 60, vec![1]), (60, 120, vec![])]);
    assert_eq!(segments(&[], 120).len(), 1, "no blocks, one plain piece");
}

#[test]
fn a_block_runs_from_just_after_zero_to_just_before_one() {
    let block = Block { item: 0, first: 24, last: 48 };
    let (first, time) = block_state(&block, 24, 24);
    assert!((first - 0.5 / 24.0).abs() < 1e-6 && time == 0.0);
    let (last, time) = block_state(&block, 47, 24);
    assert!((last - 23.5 / 24.0).abs() < 1e-6 && (time - 23.0 / 24.0).abs() < 1e-6);
}

#[test]
fn the_players_frames_are_the_exports_frames() {
    // Generated blocks with the frames and the progress the player's rule gives each: `fixtures/cases/look_frame_cases.json` is the pack's own
    // `tests/look_frame_cases.json` (the Builder's `look_model.mjs` runs it too). The pack writes it with `node scripts/export_look_frame_cases.mjs`.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cases/look_frame_cases.json");
    let file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() >= 500);
    for (n, case) in cases.iter().enumerate() {
        let total = case["total"].as_i64().unwrap();
        let (blocks, _) = plan(&[effect("Glow", case["start"].as_f64().unwrap(), case["seconds"].as_f64().unwrap())], total, 24);
        let got = blocks.first().map(|b| (b.first, b.last));
        let want = case["frames"].as_object().map(|f| (f["first"].as_i64().unwrap(), f["last"].as_i64().unwrap()));
        assert_eq!(got, want, "case {n}: {case}");
        let Some(block) = blocks.first() else { continue };
        for state in case["states"].as_array().unwrap() {
            let frame = state["frame"].as_i64().unwrap();
            let covered = frame >= block.first && frame < block.last;
            assert_eq!(covered, !state["progress"].is_null(), "case {n} frame {frame}: {case}");
            if covered {
                let (progress, time) = block_state(block, frame, 24);
                assert!((f64::from(progress) - state["progress"].as_f64().unwrap()).abs() < 1e-5, "case {n} frame {frame} progress: {case}");
                assert!((f64::from(time) - state["time"].as_f64().unwrap()).abs() < 1e-5, "case {n} frame {frame} time: {case}");
            }
        }
    }
}

#[test]
fn items_come_from_json_with_defaults_and_are_checked() {
    let items = items_from_json(Kind::Filter, Some(&json!([{"look": "Sepia", "start": 1.5}]))).unwrap();
    assert_eq!((items[0].start, items[0].seconds, items[0].intensity), (1.5, 1.0, 1.0));
    let clamped = items_from_json(Kind::Effect, Some(&json!([{"look": "Glow", "start": 0, "seconds": 0.0, "intensity": 7}]))).unwrap();
    assert_eq!((clamped[0].seconds, clamped[0].intensity), (0.05, 1.0));
    assert!(items_from_json(Kind::Effect, Some(&json!([{"look": "Sepia", "start": 0}]))).is_err(), "Sepia is a filter, not an effect");
    assert!(items_from_json(Kind::Filter, Some(&json!([{"look": "Nope", "start": 0}]))).is_err());
    assert!(items_from_json(Kind::Filter, Some(&json!([{"look": "Sepia"}]))).is_err(), "it needs its start");
    assert!(items_from_json(Kind::Filter, Some(&json!([{"look": "Sepia", "start": 0, "params": {"nope": 1}}]))).is_err());
    assert!(items_from_json(Kind::Filter, None).unwrap().is_empty());
}

fn part(from: i64, to: i64, copy: bool) -> Part {
    Part { from, to, copy }
}

#[test]
fn frames_without_a_look_are_copied_from_keyframe_to_keyframe_and_the_rest_is_encoded() {
    let keys = [0, 30, 60, 90, 120];
    // Cut at the keyframes inside the stretch: the frames before the first and after the last are encoded again.
    assert_eq!(plain_parts(10, 100, 200, &keys, 24), vec![part(10, 30, false), part(30, 90, true), part(90, 100, false)]);
    // A stretch that starts on a keyframe has nothing in front of it.
    assert_eq!(plain_parts(0, 100, 200, &keys, 24), vec![part(0, 90, true), part(90, 100, false)]);
    // Up to the end of the video the last group of pictures ends with the video, so it is copied whole.
    assert_eq!(plain_parts(100, 200, 200, &[0, 96, 120, 168], 24), vec![part(100, 120, false), part(120, 200, true)]);
    assert_eq!(plain_parts(120, 200, 200, &[0, 96, 120, 168], 24), vec![part(120, 200, true)]);
}

#[test]
fn a_stretch_with_no_whole_group_of_pictures_in_it_is_encoded_whole() {
    assert_eq!(plain_parts(10, 50, 200, &[0, 30, 60], 24), vec![part(10, 50, false)], "no keyframe to end the copy on");
    assert_eq!(plain_parts(31, 59, 200, &[0, 30, 60], 24), vec![part(31, 59, false)], "no keyframe inside");
    assert_eq!(plain_parts(30, 90, 200, &[0, 30, 40, 90], 24), vec![part(30, 90, true)]);
    assert_eq!(plain_parts(25, 55, 200, &[0, 30, 40, 60], 24), vec![part(25, 55, false)], "10 frames are not worth a piece of their own");
    assert_eq!(plain_parts(0, 100, 200, &[], 24), vec![part(0, 100, false)], "no keyframes known");
}

// -- a real stitch ----------------------------------------------------------------------------------------------------------

const W: usize = 320;
const H: usize = 180;
const FPS: u32 = 24;

fn make_moving_clip(tools: &Tools, path: &Path, hue: u32) {
    let source = format!("testsrc2=s={W}x{H}:r={FPS}:d=2,hue=h={hue}");
    let out = run(&tools.ffmpeg, &args(&["-y", "-f", "lavfi", "-i", &source, "-c:v", "libx264", "-pix_fmt", "yuv420p", &path.display().to_string()])).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn make_song(tools: &Tools, path: &Path) {
    let out = run(&tools.ffmpeg, &args(&["-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=6", &path.display().to_string()])).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// The luma plane of every frame, read without any colour conversion.
fn luma_frames(tools: &Tools, video: &Path) -> Vec<Vec<u8>> {
    let out = run(&tools.ffmpeg, &args(&["-v", "error", "-i", &video.display().to_string(), "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout.chunks(W * H * 3 / 2).map(|frame| frame[..W * H].to_vec()).collect()
}

fn mean_square(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2)).sum::<f64>() / a.len() as f64
}

/// The distinct colour tags the frames of a video carry.
fn colour_tags(tools: &Tools, video: &Path) -> Vec<String> {
    let out = run(&tools.ffprobe, &args(&["-v", "error", "-select_streams", "v:0", "-show_entries", "frame=color_range,color_space", "-of", "csv=p=0", &video.display().to_string()])).unwrap();
    let mut tags: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim_end_matches(',').to_string()).collect();
    tags.sort();
    tags.dedup();
    tags
}

#[test]
fn looks_change_only_the_frames_they_cover_and_the_video_keeps_the_timeline() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    let clips: Vec<_> = (0..3).map(|i| dir.path().join(format!("scene{i}.mp4"))).collect();
    for (i, clip) in clips.iter().enumerate() {
        make_moving_clip(&tools, clip, 100 * (i as u32 + 1));
    }
    let song = dir.path().join("song.wav");
    make_song(&tools, &song);
    let stitch_with = |looks: Vec<LookItem>, prefix: &str| {
        let request = StitchRequest {
            project: dir.path().join("project"),
            scene_paths: clips.clone(),
            audio_path: Some(song.clone()),
            scene_timing: (0..3).map(|i| TimingItem { start: f64::from(i) * 2.0, end: f64::from(i + 1) * 2.0 }).collect(),
            timeline_fps: FPS,
            looks,
            output_prefix: prefix.into(),
            ..Default::default()
        };
        stitch_scene_videos(&tools, &request).expect("stitch")
    };
    let plain = stitch_with(vec![], "PLAIN");
    // A filter over frames 12..36, an effect over 30..60 (across the cut at 48), and a second effect inside the last scene.
    let styled = stitch_with(vec![filter("NoirClassic", 0.5, 1.0), effect("Negative", 1.25, 1.25), effect("Pixelate", 4.5, 0.5)], "STYLED");

    assert_eq!(styled.timeline_frame_count, 144);
    assert_eq!(styled.looks.len(), 3);
    assert_eq!(styled.looks.iter().map(|l| (l.name.as_str(), l.frames)).collect::<Vec<_>>(), vec![("NoirClassic", 24), ("Negative", 30), ("Pixelate", 12)]);
    assert!(styled.look_notes.is_empty(), "{:?}", styled.look_notes);
    assert!(plain.looks.is_empty());

    let (a, b) = (luma_frames(&tools, &plain.final_video_path), luma_frames(&tools, &styled.final_video_path));
    assert_eq!((a.len(), b.len()), (144, 144), "the frames of the timeline are kept");
    let covered = |n: usize| (12..36).contains(&n) || (30..60).contains(&n) || (108..120).contains(&n);
    for n in (0..144).filter(|n| !covered(*n)) {
        let error = mean_square(&a[n], &b[n]);
        assert!(error < 40.0, "frame {n} is not the plain video's frame (mean square error {error:.1})");
    }
    for n in [20, 45, 114] {
        let error = mean_square(&a[n], &b[n]);
        assert!(error > 150.0, "frame {n} should be changed by its look (mean square error {error:.1})");
    }
    assert_eq!(colour_tags(&tools, &styled.final_video_path), colour_tags(&tools, &plain.final_video_path), "the drawn frames carry the same colour tags as the rest");
}

#[test]
fn a_stitch_with_looks_but_no_scene_timing_says_so() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("scene.mp4");
    make_moving_clip(&tools, &clip, 100);
    let song = dir.path().join("song.wav");
    make_song(&tools, &song);
    let request = StitchRequest {
        project: dir.path().join("project"),
        scene_paths: vec![clip],
        audio_path: Some(song),
        looks: vec![filter("Sepia", 0.0, 1.0)],
        output_prefix: "NOTIMING".into(),
        ..Default::default()
    };
    let result = stitch_scene_videos(&tools, &request).expect("stitch");
    assert!(result.looks.is_empty());
    assert_eq!(result.look_notes.len(), 1, "{:?}", result.look_notes);
}

/// A clip with a keyframe every half second, written at `crf` like a scene clip of the stitch.
fn make_gop_clip(tools: &Tools, path: &Path, hue: u32, seconds: u32, crf: u32) {
    let source = format!("testsrc2=s={W}x{H}:r={FPS}:d={seconds},hue=h={hue}");
    let out = run(
        &tools.ffmpeg,
        &args(&[
            "-y", "-f", "lavfi", "-i", &source, "-c:v", "libx264", "-preset", "medium", "-crf", &crf.to_string(), "-g", "12", "-keyint_min", "12", "-sc_threshold", "0", "-pix_fmt", "yuv420p",
            &path.display().to_string(),
        ]),
    )
    .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// The `(crf, ref)` of every x264 settings string in a file: each piece of a joined video carries the one of its own first frame.
fn x264_settings(video: &Path) -> Vec<(String, String)> {
    let bytes = std::fs::read(video).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    let token = |settings: &str, key: &str| settings.split_whitespace().find_map(|w| w.strip_prefix(key)).unwrap_or("").to_string();
    text.split("x264 - core").skip(1).map(|rest| {
        let settings = rest.split('\0').next().unwrap_or("");
        (token(settings, "crf="), token(settings, "ref="))
    }).collect()
}

#[test]
fn frames_between_the_looks_are_copied_and_what_is_encoded_keeps_the_scene_quality() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    // Three scenes of four seconds, rendered at crf 10. The stitch writes each scene again with one keyframe at its start (frames 0, 96 and 192).
    let clips: Vec<_> = (0..3).map(|i| dir.path().join(format!("scene{i}.mp4"))).collect();
    for (i, clip) in clips.iter().enumerate() {
        make_gop_clip(&tools, clip, 100 * (i as u32 + 1), 4, 10);
    }
    let song = dir.path().join("song.wav");
    let out = run(&tools.ffmpeg, &args(&["-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=12", &song.display().to_string()])).unwrap();
    assert!(out.status.success());
    let stitch_with = |looks: Vec<LookItem>, prefix: &str| {
        let request = StitchRequest {
            project: dir.path().join("project"),
            scene_paths: clips.clone(),
            audio_path: Some(song.clone()),
            scene_timing: (0..3).map(|i| TimingItem { start: f64::from(i) * 4.0, end: f64::from(i + 1) * 4.0 }).collect(),
            timeline_fps: FPS,
            looks,
            output_prefix: prefix.into(),
            ..Default::default()
        };
        stitch_scene_videos(&tools, &request).expect("stitch")
    };
    let plain = stitch_with(vec![], "PLAIN");
    // Effects over frames 18..30 and 204..216. Between them, the group of pictures that runs from the keyframe at 96 to the one at 192 is copied. Frames
    // 30..96 and 192..204 are encoded again (no keyframe to cut on), and so are frames 0..18 and 216..288 (no whole group of pictures in them).
    let styled = stitch_with(vec![effect("Negative", 0.75, 0.5), effect("Pixelate", 8.5, 0.5)], "STYLED");
    assert_eq!(styled.timeline_frame_count, 288);
    assert_eq!(styled.looks.len(), 2);

    let (a, b) = (luma_frames(&tools, &plain.final_video_path), luma_frames(&tools, &styled.final_video_path));
    assert_eq!((a.len(), b.len()), (288, 288), "the frames of the timeline are kept");
    for n in 96..192 {
        assert!(a[n] == b[n], "frame {n} was copied from the video, so it is the plain video's frame bit for bit");
    }
    for n in (0..18).chain(30..96).chain(192..204).chain(216..288) {
        let error = mean_square(&a[n], &b[n]);
        assert!(error < 40.0, "frame {n} is encoded again but still the plain video's frame (mean square error {error:.1})");
    }
    for n in [24, 210] {
        assert!(mean_square(&a[n], &b[n]) > 150.0, "frame {n} is changed by its look");
    }

    // The video plays without a decoder complaint, and the pieces were joined without being decoded.
    let check = run(&tools.ffmpeg, &args(&["-v", "error", "-i", &styled.final_video_path.display().to_string(), "-f", "null", "-"])).unwrap();
    assert!(check.stderr.is_empty(), "{}", String::from_utf8_lossy(&check.stderr));

    // What was encoded here is as good as the scene clips (crf 10, not the old crf 17) and the same preset (ref 3, not the veryfast ref 1).
    let settings = x264_settings(&styled.final_video_path);
    assert!(settings.len() >= 3, "{settings:?}");
    for (crf, refs) in &settings {
        assert_eq!(crf, "10.0", "{settings:?}");
        assert_eq!(refs, "3", "{settings:?}");
    }
}
