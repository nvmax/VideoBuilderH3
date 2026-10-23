//! Scene images and reference card pictures: archive names, the history lifecycle and the card preview.


use h3_project::images::*;
use h3_project::Stamp;
use serde_json::{json, Value};


fn stamp(compact: &str) -> Stamp {
    Stamp { compact: compact.into(), iso: String::new(), epoch: 0.0 }
}

fn session() -> h3_core::pyval::Obj {
    json!({
        "segments": [{"id": "a", "custom_image_path": "old.png", "approved_image_path": "x.png"}, {"id": "b"}],
        "flux_reference_builder": {"subjects": [{"id": "s1", "name": "Ava / Lead", "image": {}}], "locations": []}
    })
    .as_object()
    .unwrap()
    .clone()
}

fn write_png(path: &std::path::Path) {
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30])).save(path).unwrap();
}

#[test]
fn previews_are_archived_with_unique_names() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("pic.PNG");
    write_png(&src);
    let first = archive_scene_image(dir.path(), 3, &src, &stamp("20260101_000000")).unwrap();
    let second = archive_scene_image(dir.path(), 3, &src, &stamp("20260101_000000")).unwrap();
    assert!(first.ends_with("scene_image_previews/scene_0003/preview_20260101_000000.png") || first.to_string_lossy().ends_with("scene_0003\\preview_20260101_000000.png"), "{first:?}");
    assert_eq!(second.file_name().unwrap(), "preview_20260101_000000_02.png");
    assert!(archive_scene_image(dir.path(), 1, &dir.path().join("missing.png"), &stamp("x")).is_err());
    assert!(decode_image_data("data:image/png;base64,@@@").is_err());
}

#[test]
fn a_new_image_joins_the_history_and_clears_the_choice() {
    let mut s = session();
    let info = json!({"filename": "a.png"});
    let (shown, count) = record_scene_image(&mut s, 0, &info, std::path::Path::new("p1.png")).unwrap();
    assert_eq!((shown, count), (0, 1));
    let (shown, count) = record_scene_image(&mut s, 0, &info, std::path::Path::new("p2.png")).unwrap();
    assert_eq!((shown, count), (1, 2));
    let seg = &s["segments"][0];
    assert_eq!(seg["approved_image_path"], "");
    assert_eq!(seg["custom_image_path"], "");
    assert_eq!(seg["preview_mode"], "image");
    // Stepping back and forward stays inside the history.
    assert_eq!(revert_scene_image(&mut s, 0, -1, None).unwrap().0, 0);
    assert_eq!(revert_scene_image(&mut s, 0, -5, None).unwrap().0, 0);
    assert_eq!(revert_scene_image(&mut s, 0, 0, Some(9)).unwrap().0, 1);
    assert!(revert_scene_image(&mut s, 1, -1, None).is_err());
    assert!(record_scene_image(&mut s, 7, &info, std::path::Path::new("p.png")).is_err());
}

#[test]
fn approving_needs_a_file_and_clearing_keeps_the_history() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keep.png");
    write_png(&file);
    let mut s = session();
    record_scene_image(&mut s, 0, &json!({}), &file).unwrap();
    let approved = approve_scene_image(&mut s, 0, None).unwrap();
    assert!(approved.ends_with("keep.png"));
    assert!(approve_scene_image(&mut s, 0, Some("nope.png")).is_err());
    clear_scene_image(&mut s, 0).unwrap();
    assert_eq!(s["segments"][0]["approved_image_path"], "");
    assert_eq!(s["segments"][0]["image_history"].as_array().unwrap().len(), 1);
    assert_eq!(s["segments"][0]["image_assignment_cleared"], true);
}

#[test]
fn a_reference_picture_is_saved_under_the_cards_name_with_a_jpeg_preview() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("gen.png");
    write_png(&src);
    let image = save_reference_image(dir.path(), "subjects", "Ava / Lead", &src).unwrap();
    assert_eq!(image["name"], "Ava _ Lead.png");
    assert!(dir.path().join("subject_location/subject/Ava _ Lead.png").is_file());
    assert!(image["data"].as_str().unwrap().starts_with("data:image/jpeg;base64,"), "preview missing: {}", image["data"]);
    // A second picture replaces the first and the old file stays as a numbered copy.
    save_reference_image(dir.path(), "subjects", "Ava / Lead", &src).unwrap();
    assert!(dir.path().join("subject_location/subject/Ava _ Lead_previous_02.png").is_file());
    assert!(save_reference_image(dir.path(), "things", "x", &src).is_err());

    let mut s = session();
    assert_eq!(set_reference_image(&mut s, "subjects", "s1", image).unwrap(), "Ava / Lead");
    assert!(reference_card(&s, "subjects", "s1").unwrap().1.ends_with("Ava _ Lead.png"));
    assert!(set_reference_image(&mut s, "subjects", "zz", Value::Null).is_err());
}
