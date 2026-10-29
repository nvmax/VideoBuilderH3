//! The app's effects and filters: every file translates to WGSL, the catalog lists them with what a picker needs, and every one draws on the GPU:
//! untouched at intensity 0, changed at intensity 1.

use std::time::Instant;

use h3_transitions::{look, look_catalog, looks, params_from_json, Kind, TransitionRenderer};
use serde_json::json;

const W: u32 = 480;
const H: u32 = 270;

/// A picture with colour, brightness steps, texture and detail in both directions, so a blur, a warp or a grade changes it.
fn picture() -> Vec<u8> {
    let mut out = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let cell = ((x / 20) + (y / 20)) % 2;
            let r = (x * 255 / (W - 1)) as u8;
            let g = if cell == 0 { 60 } else { 190 };
            let b = (y * 255 / (H - 1)) as u8;
            let bright = if (x / 40 + y / 30) % 5 == 0 { 255 } else { 0 };
            // A little texture, so a look that smooths detail has detail to smooth.
            let noise = ((x * 7919 + y * 104_729) % 41) as i32 - 20;
            let texture = |v: u8| (i32::from(v) + noise).clamp(0, 255) as u8;
            out.extend_from_slice(&[texture(r.max(bright)), texture(g.max(bright)), texture(b.max(bright)), 255]);
        }
    }
    out
}

fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a.chunks(4).zip(b.chunks(4)).map(|(p, q)| (0..3).map(|i| u64::from(p[i].abs_diff(q[i]))).sum::<u64>()).sum();
    sum as f64 / (a.len() / 4 * 3) as f64 / 255.0
}

fn max_difference(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0)
}

#[test]
fn the_app_has_a_large_library_of_effects_and_filters() {
    for (kind, least) in [(Kind::Effect, 70), (Kind::Filter, 120)] {
        let results = looks::try_entries(kind);
        let errors: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(results.len() >= least, "only {} {}s", results.len(), kind.name());
    }
}

#[test]
fn the_catalog_describes_every_look() {
    for kind in [Kind::Effect, Kind::Filter] {
        let list = look_catalog(kind);
        let mut seen = std::collections::HashSet::new();
        for entry in &list {
            let id = entry["id"].as_str().unwrap();
            assert!(seen.insert(id.to_string()), "{id} is listed twice");
            assert_eq!(entry["kind"], kind.name(), "{id}");
            assert_eq!(entry["origin"], "original", "{id}");
            for key in ["author", "license", "description", "category"] {
                assert!(!entry[key].as_str().unwrap_or("").is_empty(), "{id} has no {key}");
            }
            assert!(!entry["tags"].as_array().unwrap().is_empty(), "{id} has no tags");
            for p in entry["params"].as_array().unwrap() {
                assert!(!p["description"].as_str().unwrap_or("").is_empty(), "{id}: `{}` has no description", p["name"]);
            }
        }
        let categories: std::collections::BTreeSet<&str> = list.iter().map(|e| e["category"].as_str().unwrap()).collect();
        assert!(categories.len() >= 6, "{kind:?}: only {categories:?}");
    }
}

#[test]
fn a_look_writes_its_header_then_its_settings_into_the_uniform_buffer() {
    let entry = look(Kind::Filter, "TealAndOrange").unwrap();
    assert_eq!(entry.offsets()[0], 16, "the settings start after progress, ratio, intensity and time");
    let params = params_from_json(entry, &json!({"exposure": 0.5})).unwrap();
    let bytes = params.look_buffer(0.25, 2.0, 0.75, 1.5);
    let float_at = |offset: usize| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    assert_eq!([float_at(0), float_at(4), float_at(8), float_at(12), float_at(entry.offsets()[0])], [0.25, 2.0, 0.75, 1.5, 0.5]);
    assert!(params_from_json(entry, &json!({"intensity": 1.0})).is_err(), "intensity is the block's, not a setting");
    assert!(look(Kind::Effect, "NoSuchEffect").is_err());
}

#[test]
fn every_look_is_untouched_at_intensity_zero_and_changes_the_picture_at_one() {
    let source = picture();
    let mut renderer = TransitionRenderer::new().expect("a graphics adapter");
    println!("adapter: {}", renderer.adapter_name());
    let mut frame = Vec::new();
    let mut problems = Vec::new();
    let mut unchanged = Vec::new();
    let started = Instant::now();
    let mut drawn = 0;
    for kind in [Kind::Effect, Kind::Filter] {
        for entry in look_catalog(kind) {
            let id = entry["id"].as_str().unwrap();
            let params = params_from_json(look(kind, id).unwrap(), &json!({})).unwrap();
            if let Err(e) = renderer.render_look(&params, W, H, &source, 0.3, 0.0, 0.45, &mut frame) {
                problems.push(format!("{id}: {e}"));
                continue;
            }
            let off = max_difference(&frame, &source);
            if off > 1 {
                problems.push(format!("{id}: intensity 0 changed the picture by up to {off} levels"));
            }
            if let Err(e) = renderer.render_look(&params, W, H, &source, 0.3, 1.0, 0.45, &mut frame) {
                problems.push(format!("{id}: {e}"));
                continue;
            }
            drawn += 1;
            if frame.chunks(4).any(|p| p[3] != 255) {
                problems.push(format!("{id}: the picture is not opaque"));
            }
            if mean_difference(&frame, &source) < 0.002 {
                unchanged.push(id.to_string());
            }
        }
    }
    println!("{drawn} looks drawn in {:.1?}", started.elapsed());
    // The neutral Adjust filter changes nothing until a setting moves.
    unchanged.retain(|id| id != "Adjust");
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(unchanged.is_empty(), "these looks did not change the picture at intensity 1: {unchanged:?}");
}

#[test]
fn a_filter_setting_changes_the_grade_and_a_look_is_fast_enough_for_video() {
    let source = picture();
    let mut renderer = TransitionRenderer::new().expect("a graphics adapter");
    let entry = look(Kind::Filter, "Adjust").unwrap();
    let mut neutral = Vec::new();
    renderer.render_look(&params_from_json(entry, &json!({})).unwrap(), W, H, &source, 0.5, 1.0, 0.0, &mut neutral).unwrap();
    assert!(max_difference(&neutral, &source) <= 1, "Adjust at its defaults is the picture");
    let mut brighter = Vec::new();
    renderer.render_look(&params_from_json(entry, &json!({"exposure": 0.7, "saturation": -1.0})).unwrap(), W, H, &source, 0.5, 1.0, 0.0, &mut brighter).unwrap();
    assert!(mean_difference(&brighter, &source) > 0.05);
    // Grey: with saturation at -1 the three channels agree.
    assert!(brighter.chunks(4).all(|p| p[0].abs_diff(p[1]) <= 2 && p[1].abs_diff(p[2]) <= 2));

    // 1080p, one heavy effect.
    let (w, h) = (1920u32, 1080u32);
    let big: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();
    let heavy = params_from_json(look(Kind::Effect, "Painterly").unwrap(), &json!({})).unwrap();
    let mut frame = Vec::new();
    renderer.render_look(&heavy, w, h, &big, 0.5, 1.0, 0.0, &mut frame).unwrap();
    let started = Instant::now();
    for _ in 0..5 {
        renderer.render_look(&heavy, w, h, &big, 0.5, 1.0, 0.0, &mut frame).unwrap();
    }
    let each = started.elapsed().as_secs_f64() / 5.0;
    println!("Painterly at 1080p: {:.1} ms a frame", each * 1000.0);
    assert!(each < 0.25, "{each} s a frame");
}

/// `H3_SHEET_DIR=<folder> H3_SHEET_A=<picture.png> cargo test -p h3-transitions --release -- --ignored look_sheet` writes every effect and filter
/// drawn on a picture (at progress 0.5, 0.45 s into its block) as `<kind>_<id>.png`, for a contact sheet.
#[test]
#[ignore]
fn look_sheet() {
    use image::{imageops, RgbaImage};
    let Some(dir) = std::env::var_os("H3_SHEET_DIR") else { return };
    let (w, h) = (640u32, 360u32);
    let picture = std::env::var_os("H3_SHEET_A")
        .and_then(|p| image::open(p).ok())
        .map(|img| imageops::resize(&img.to_rgba8(), w, h, imageops::FilterType::Triangle))
        .expect("H3_SHEET_A names a picture");
    let mut renderer = TransitionRenderer::new().expect("a graphics adapter");
    let mut frame = Vec::new();
    let only = std::env::var("H3_SHEET_ONLY").ok();
    for kind in [Kind::Effect, Kind::Filter] {
        for entry in look_catalog(kind) {
            let id = entry["id"].as_str().unwrap();
            if only.as_deref().is_some_and(|o| !o.split(',').any(|x| x == id)) {
                continue;
            }
            let params = params_from_json(look(kind, id).unwrap(), &json!({})).unwrap();
            renderer.render_look(&params, w, h, picture.as_raw(), 0.5, 1.0, 0.45, &mut frame).unwrap();
            RgbaImage::from_raw(w, h, frame.clone()).unwrap().save(std::path::Path::new(&dir).join(format!("{}_{id}.png", kind.name()))).unwrap();
        }
    }
}
