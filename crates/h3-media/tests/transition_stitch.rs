//! Transitions as blocks of the timeline: where the windows sit, and that a stitched video keeps the timeline's frames.
//! The stitch tests need ffmpeg and ffprobe on PATH and a graphics adapter.

use std::path::Path;

use h3_media::stitch::{stitch_scene_videos, StitchRequest, TimingItem};
use h3_media::tools::{args, run, Tools};
use h3_media::transition::{clip_at, clip_bounds, items_from_json, pieces, plan, AppliedTransition, Piece, TransitionItem, Window};
use serde_json::json;

fn item(start: f64, seconds: f64) -> TransitionItem {
    named("WipeLeft", start, seconds)
}

fn named(name: &str, start: f64, seconds: f64) -> TransitionItem {
    TransitionItem { name: name.into(), params: json!({}), start, seconds }
}

/// Clips of the given lengths in seconds, one after the other.
fn timeline(lengths: &[f64]) -> Vec<(f64, f64)> {
    let mut at = 0.0;
    lengths
        .iter()
        .map(|length| {
            at += length;
            (at - length, at)
        })
        .collect()
}

fn window(plan: &(Vec<Window>, Vec<String>), n: usize) -> (i64, u32, Option<usize>) {
    let w = &plan.0[n];
    (w.first, w.frames, w.cut)
}

#[test]
fn a_block_over_a_cut_takes_frames_from_both_sides() {
    // The cut is at frame 48, the block is frames 42..54.
    let planned = plan(&[item(1.75, 0.5)], &timeline(&[2.0, 2.0]), 24);
    assert_eq!(window(&planned, 0), (42, 12, Some(0)));
    assert!(planned.1.is_empty());
}

#[test]
fn a_block_that_ends_or_starts_at_a_cut_belongs_to_that_cut() {
    let clips = timeline(&[2.0, 2.0, 2.0]);
    let planned = plan(&[item(1.5, 0.5), item(4.0, 0.5)], &clips, 24);
    assert_eq!(window(&planned, 0), (36, 12, Some(0)), "ends at the cut at frame 48");
    assert_eq!(window(&planned, 1), (96, 12, Some(1)), "starts at the cut at frame 96");
}

#[test]
fn a_block_inside_a_clip_is_an_effect_on_that_clip() {
    let planned = plan(&[item(0.5, 0.5)], &timeline(&[2.0, 2.0]), 24);
    assert_eq!(window(&planned, 0), (12, 12, None));
    // One frame short of the cut it still touches nothing.
    let planned = plan(&[item(1.4583, 0.5 - 1.0 / 24.0)], &timeline(&[2.0, 2.0]), 24);
    assert_eq!(planned.0[0].cut, None);
}

#[test]
fn a_block_over_two_cuts_is_cut_down_to_the_nearer_cut() {
    // Clips of 24 frames: cuts at frames 24 and 48. The block is frames 12..50, nearer the first cut.
    let planned = plan(&[item(0.5, 38.0 / 24.0)], &timeline(&[1.0, 1.0, 1.0]), 24);
    assert_eq!(window(&planned, 0), (12, 36, Some(0)));
    assert_eq!(planned.1.len(), 1);
    assert!(planned.1[0].contains("shortened from 38 to 36"), "{:?}", planned.1);
}

#[test]
fn a_block_that_overlaps_the_one_before_it_starts_where_that_one_ends() {
    let planned = plan(&[item(1.0, 0.5), item(1.25, 0.5)], &timeline(&[4.0]), 24);
    assert_eq!(window(&planned, 0), (24, 12, None));
    assert_eq!(window(&planned, 1), (36, 6, None));
    assert_eq!(planned.1.len(), 1);
}

#[test]
fn blocks_are_planned_in_timeline_order_whatever_order_they_come_in() {
    let planned = plan(&[item(2.0, 0.5), item(0.5, 0.5)], &timeline(&[4.0]), 24);
    assert_eq!((planned.0[0].first, planned.0[1].first), (12, 48));
}

#[test]
fn a_block_with_no_room_is_left_out() {
    // Past the end of the timeline, and over the last frame only.
    let planned = plan(&[item(10.0, 0.5), item(3.999, 0.5)], &timeline(&[2.0, 2.0]), 24);
    assert!(planned.0.is_empty());
    assert_eq!(planned.1.len(), 2);
}

#[test]
fn a_block_that_starts_in_a_gap_between_clips_lands_on_the_next_clip() {
    // Clips 0..1 s and 2..3 s (the scenes between were not stitched): the joined video is 48 frames and the gap is not part of it.
    let planned = plan(&[item(1.5, 0.75)], &[(0.0, 1.0), (2.0, 3.0)], 24);
    assert_eq!(planned.0.len(), 1);
    assert_eq!(planned.0[0].cut, Some(0));
    assert_eq!(planned.0[0].first, 24);
}

#[test]
fn the_players_windows_are_the_exports_windows() {
    // Generated scene layouts and blocks with the window the player's rule gives each: `fixtures/cases/transition_window_cases.json` is the pack's own
    // `tests/transition_window_cases.json` (the Builder's `transition_model.mjs` runs it too). The pack writes it with `node scripts/export_transition_window_cases.mjs`.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cases/transition_window_cases.json");
    let cases: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(cases.len() > 500);
    for (n, case) in cases.iter().enumerate() {
        let clips: Vec<(f64, f64)> = case["clips"].as_array().unwrap().iter().map(|c| (c[0].as_f64().unwrap(), c[1].as_f64().unwrap())).collect();
        let (windows, _) = plan(&[named("WipeLeft", case["start"].as_f64().unwrap(), case["seconds"].as_f64().unwrap())], &clips, 24);
        let got = windows.first().map(|w| (w.first, i64::from(w.frames), w.cut));
        let want = case["expect"].as_object().map(|e| (e["first"].as_i64().unwrap(), e["frames"].as_i64().unwrap(), e["cut"].as_u64().map(|cut| cut as usize)));
        assert_eq!(got, want, "case {n}: {case}");
    }
}

#[test]
fn the_pieces_are_the_clips_with_the_windows_taken_out() {
    let bounds = clip_bounds(&[48, 48, 48]);
    let windows = vec![
        Window { name: "A".into(), params: json!({}), first: 42, frames: 12, cut: Some(0) },
        Window { name: "B".into(), params: json!({}), first: 84, frames: 12, cut: Some(1) },
    ];
    assert_eq!(
        pieces(&bounds, &windows),
        vec![
            Piece::Scene { clip: 0, from: 0, to: 42 },
            Piece::Transition(0),
            Piece::Scene { clip: 1, from: 6, to: 36 },
            Piece::Transition(1),
            Piece::Scene { clip: 2, from: 0, to: 48 },
        ]
    );
    assert_eq!(pieces(&bounds, &[]), vec![Piece::Scene { clip: 0, from: 0, to: 48 }, Piece::Scene { clip: 1, from: 0, to: 48 }, Piece::Scene { clip: 2, from: 0, to: 48 }]);
}

#[test]
fn a_window_that_covers_a_whole_clip_leaves_no_piece_of_it() {
    let bounds = clip_bounds(&[24, 24, 24]);
    let windows = vec![Window { name: "A".into(), params: json!({}), first: 24, frames: 24, cut: Some(1) }];
    assert_eq!(pieces(&bounds, &windows), vec![Piece::Scene { clip: 0, from: 0, to: 24 }, Piece::Transition(0), Piece::Scene { clip: 2, from: 0, to: 24 }]);
    assert_eq!((clip_at(&bounds, 0), clip_at(&bounds, 23), clip_at(&bounds, 24), clip_at(&bounds, 71)), (0, 0, 1, 2));
}

#[test]
fn items_come_from_json_with_defaults() {
    let items = items_from_json(Some(&json!([{"start": 1.5, "transition": "Swirl"}]))).unwrap();
    assert_eq!((items[0].start, items[0].seconds), (1.5, 0.5));
    assert!(items_from_json(Some(&json!([{"start": 0, "transition": "Nope"}]))).is_err());
    assert!(items_from_json(Some(&json!([{"transition": "Swirl"}]))).is_err(), "a transition needs its start");
    assert!(items_from_json(Some(&json!([{"start": -1, "transition": "Swirl"}]))).is_err());
    assert!(items_from_json(None).unwrap().is_empty());
}

#[test]
fn a_reply_says_where_the_transition_sits() {
    let on_cut = AppliedTransition { name: "Swirl".into(), start: 1.75, frames: 12, cut: Some(0) };
    assert_eq!(on_cut.to_json(), json!({"transition": "Swirl", "start": 1.75, "frames": 12, "after_scene_number": 1}));
    let inside = AppliedTransition { name: "FlashColor".into(), start: 0.5, frames: 12, cut: None };
    assert_eq!(inside.to_json()["after_scene_number"], json!(null));
}

// -- a real stitch ----------------------------------------------------------------------------------------------------------

const W: usize = 320;
const H: usize = 180;
const FPS: u32 = 24;

fn make_clip(tools: &Tools, path: &Path, color: &str) {
    let source = format!("color=c={color}:s={W}x{H}:r={FPS}:d=2");
    let out = run(&tools.ffmpeg, &args(&["-y", "-f", "lavfi", "-i", &source, "-c:v", "libx264", "-pix_fmt", "yuv420p", &path.display().to_string()])).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn make_song(tools: &Tools, path: &Path) {
    let out = run(&tools.ffmpeg, &args(&["-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=4", &path.display().to_string()])).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn frames_of(tools: &Tools, video: &Path) -> Vec<Vec<u8>> {
    let out = run(&tools.ffmpeg, &args(&["-v", "error", "-i", &video.display().to_string(), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout.chunks(W * H * 3).map(<[u8]>::to_vec).collect()
}

/// Share of the frame's pixels that are clearly red and clearly blue.
fn red_and_blue(frame: &[u8]) -> (f64, f64) {
    let (mut red, mut blue) = (0usize, 0usize);
    for p in frame.chunks(3) {
        if p[0] > 170 && p[1] < 80 && p[2] < 80 {
            red += 1;
        }
        if p[2] > 170 && p[0] < 80 && p[1] < 80 {
            blue += 1;
        }
    }
    let n = (W * H) as f64;
    (red as f64 / n, blue as f64 / n)
}

fn stitch(tools: &Tools, dir: &Path, transitions: Vec<TransitionItem>) -> (h3_media::stitch::StitchResult, Vec<Vec<u8>>) {
    let (a, b, song) = (dir.join("a.mp4"), dir.join("b.mp4"), dir.join("song.wav"));
    make_clip(tools, &a, "red");
    make_clip(tools, &b, "blue");
    make_song(tools, &song);
    let request = StitchRequest {
        project: dir.join("project"),
        scene_paths: vec![a, b],
        audio_path: Some(song),
        scene_timing: vec![TimingItem { start: 0.0, end: 2.0 }, TimingItem { start: 2.0, end: 4.0 }],
        timeline_fps: FPS,
        transitions,
        output_prefix: "FINAL_VIDEO".into(),
        ..Default::default()
    };
    let result = stitch_scene_videos(tools, &request).expect("stitch");
    let frames = frames_of(tools, &result.final_video_path);
    (result, frames)
}

#[test]
fn a_stitch_with_a_centered_transition_keeps_the_timeline_frames() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    let (result, frames) = stitch(&tools, dir.path(), vec![item(1.75, 0.5)]);

    assert_eq!(result.timeline_frame_count, 96);
    assert_eq!(frames.len(), 96, "the video has the timeline's frames");
    assert_eq!(result.transitions.len(), 1);
    assert_eq!((result.transitions[0].frames, result.transitions[0].cut), (12, Some(0)));

    // The cut is at frame 48 and the window is frames 42..54.
    let (red_before, blue_before) = red_and_blue(&frames[40]);
    assert!(red_before > 0.95 && blue_before < 0.01, "frame 40 is the first scene: {red_before} {blue_before}");
    let (red_after, blue_after) = red_and_blue(&frames[56]);
    assert!(blue_after > 0.95 && red_after < 0.01, "frame 56 is the second scene: {red_after} {blue_after}");
    let (red_mid, blue_mid) = red_and_blue(&frames[48]);
    assert!(red_mid > 0.2 && blue_mid > 0.2, "frame 48 shows both scenes: {red_mid} {blue_mid}");
}

#[test]
fn a_block_that_ends_at_the_cut_leaves_the_second_scene_untouched_after_it() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    let (_, frames) = stitch(&tools, dir.path(), vec![item(1.5, 0.5)]);
    assert_eq!(frames.len(), 96);
    // The window is frames 36..48, so frame 47 is nearly the second scene and everything from the cut on is the second scene alone.
    let (red_cut, blue_cut) = red_and_blue(&frames[48]);
    assert!(blue_cut > 0.95 && red_cut < 0.01, "{red_cut} {blue_cut}");
    let (red_early, blue_early) = red_and_blue(&frames[35]);
    assert!(red_early > 0.95 && blue_early < 0.01, "{red_early} {blue_early}");
}

#[test]
fn a_stitch_without_transitions_is_unchanged() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    let (result, frames) = stitch(&tools, dir.path(), vec![]);
    assert_eq!(frames.len(), 96);
    assert!(result.transitions.is_empty());
    let (red, blue) = red_and_blue(&frames[47]);
    assert!(red > 0.95 && blue < 0.01);
    let (red, blue) = red_and_blue(&frames[48]);
    assert!(blue > 0.95 && red < 0.01);
}

// -- moving pictures: the frames outside a transition are the scenes' own, and nothing marks the transition's frames as different ------

fn make_moving_clip(tools: &Tools, path: &Path, hue: u32) {
    let source = format!("testsrc2=s={W}x{H}:r={FPS}:d=2,hue=h={hue}");
    let out = run(&tools.ffmpeg, &args(&["-y", "-f", "lavfi", "-i", &source, "-c:v", "libx264", "-pix_fmt", "yuv420p", &path.display().to_string()])).unwrap();
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
fn the_frames_around_a_transition_are_the_scenes_own_and_carry_no_different_tag() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    let clips: Vec<_> = (0..3).map(|i| dir.path().join(format!("scene{i}.mp4"))).collect();
    for (i, clip) in clips.iter().enumerate() {
        make_moving_clip(&tools, clip, 100 * (i as u32 + 1));
    }
    let song = dir.path().join("song.wav");
    make_song(&tools, &song);
    let stitch_with = |transitions: Vec<TransitionItem>, prefix: &str| {
        let request = StitchRequest {
            project: dir.path().join("project"),
            scene_paths: clips.clone(),
            audio_path: Some(song.clone()),
            scene_timing: (0..3).map(|i| TimingItem { start: f64::from(i) * 2.0, end: f64::from(i + 1) * 2.0 }).collect(),
            timeline_fps: FPS,
            transitions,
            output_prefix: prefix.into(),
            ..Default::default()
        };
        stitch_scene_videos(&tools, &request).expect("stitch").final_video_path
    };
    let plain = stitch_with(vec![], "PLAIN");
    // A block over the first cut, one ending at the second cut, and an effect inside the first scene.
    let with = stitch_with(vec![item(1.75, 0.5), item(3.5, 0.5), named("FlashColor", 0.5, 0.5)], "WITH");
    let (plain_frames, with_frames) = (luma_frames(&tools, &plain), luma_frames(&tools, &with));
    assert_eq!((plain_frames.len(), with_frames.len()), (144, 144));

    // The windows are frames 12..24 (inside the first scene), 42..54 (over the cut at 48) and 84..96 (ending at the cut at 96).
    let in_window = |n: usize| (12..24).contains(&n) || (42..54).contains(&n) || (84..96).contains(&n);
    for n in (0..144).filter(|n| !in_window(*n)) {
        let error = mean_square(&plain_frames[n], &with_frames[n]);
        assert!(error < 40.0, "frame {n} is not the scene's own frame (mean square error {error:.1})");
    }
    let changed = |n: usize| mean_square(&plain_frames[n], &with_frames[n]);
    assert!(changed(48) > 200.0 && changed(90) > 200.0 && changed(18) > 200.0, "the transitions change the picture inside their windows ({:.1}, {:.1}, {:.1})", changed(48), changed(90), changed(18));

    assert_eq!(colour_tags(&tools, &with), colour_tags(&tools, &plain), "the transition frames carry the same colour tags as the scenes' frames");
}

#[test]
fn a_block_inside_a_scene_flashes_that_scene_and_leaves_the_rest() {
    let tools = Tools::find("", "").expect("ffmpeg and ffprobe on PATH");
    let dir = tempfile::tempdir().unwrap();
    // Frames 12..24 of the first scene: a flash of colour that is white in the middle.
    let (result, frames) = stitch(&tools, dir.path(), vec![named("FlashColor", 0.5, 0.5)]);
    assert_eq!(frames.len(), 96);
    assert_eq!((result.transitions[0].frames, result.transitions[0].cut), (12, None));
    for n in [5, 10, 30, 47] {
        let (red, blue) = red_and_blue(&frames[n]);
        assert!(red > 0.95 && blue < 0.01, "frame {n} is the first scene: {red} {blue}");
    }
    let (red_mid, _) = red_and_blue(&frames[18]);
    assert!(red_mid < 0.5, "the middle of the flash is not the scene's red: {red_mid}");
    let (red_cut, blue_cut) = red_and_blue(&frames[48]);
    assert!(blue_cut > 0.95 && red_cut < 0.01, "the cut itself is unchanged: {red_cut} {blue_cut}");
}
