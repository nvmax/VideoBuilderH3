//! The app's own transitions: each one translates to WGSL, the uniform buffer the app writes is the collection's own for the collection's transitions,
//! and (when asked) a contact sheet to look at.

use h3_transitions::{custom, library, params_from_json, transition, Origin, Params, TransitionRenderer};
use serde_json::json;

#[test]
fn every_original_translates_to_wgsl() {
    let results = custom::try_entries();
    let errors: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(results.len(), 18);
    for entry in results.iter().flatten() {
        assert_eq!(entry.origin, Origin::Original);
        assert!(entry.wgsl.contains("fn main"), "{}", entry.id);
        assert!(!entry.description.is_empty() && entry.tags.iter().any(|t| t == "original"), "{} has a header", entry.id);
    }
}

#[test]
fn the_originals_are_in_the_catalog_beside_the_collection() {
    let list = h3_transitions::catalog();
    let originals = list.iter().filter(|t| t["origin"] == "original").count();
    let library = list.iter().filter(|t| t["origin"] == "library").count();
    assert_eq!((originals, library), (18, 125));
    let flash = list.iter().find(|t| t["id"] == "FlashColor").unwrap();
    let color = flash["params"].as_array().unwrap().iter().find(|p| p["name"] == "color").unwrap();
    assert_eq!((color["type"].clone(), color["color"].clone(), color["default"].clone()), (json!("vec4"), json!(true), json!([1.0, 1.0, 1.0, 1.0])));
    let zoom = list.iter().find(|t| t["id"] == "ZoomPullIn").unwrap();
    let strength = zoom["params"].as_array().unwrap().iter().find(|p| p["name"] == "strength").unwrap();
    assert_eq!((strength["min"].clone(), strength["max"].clone(), strength["description"].clone()), (json!(0.0), json!(2.0), json!("How far the picture is pulled in")));
}

#[test]
fn our_uniform_buffer_is_the_collections_for_every_collection_transition() {
    for entry in library::entries() {
        let ours = Params::new(transition(&entry.id).unwrap()).uniform_buffer(0.37, 1.7778);
        let theirs = library::reference_buffer(&entry.id, 0.37, 1.7778).expect("the collection has a buffer for it");
        assert_eq!(&ours[..theirs.len()], &theirs[..], "{}", entry.id);
    }
}

#[test]
fn a_changed_setting_lands_at_its_offset() {
    let entry = transition("FlashColor").unwrap();
    let params = params_from_json(entry, &json!({"hold": 0.9, "color": [1.0, 0.5, 0.0, 1.0]})).unwrap();
    let bytes = params.uniform_buffer(0.25, 2.0);
    let float_at = |offset: usize| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    assert_eq!((float_at(0), float_at(4)), (0.25, 2.0));
    let color = entry.param("color").map(|_| entry.offsets()[0]).unwrap();
    let hold = entry.offsets()[1];
    assert_eq!(color % 16, 0, "a vec4 is 16 byte aligned");
    assert_eq!((float_at(color), float_at(color + 4), float_at(hold)), (1.0, 0.5, 0.9));
}

/// `H3_SHEET_DIR=<folder> cargo test -p h3-transitions --release -- --ignored contact_sheet` writes a picture of every original at five points of its
/// progress. `H3_SHEET_A` and `H3_SHEET_B` name two pictures to use (PNG), else two made-up ones.
#[test]
#[ignore]
fn contact_sheet() {
    use image::{imageops, RgbaImage};
    let Some(dir) = std::env::var_os("H3_SHEET_DIR") else { return };
    let (w, h) = (320u32, 180u32);
    let load = |var: &str, seed: u8| -> RgbaImage {
        match std::env::var_os(var).and_then(|p| image::open(p).ok()) {
            Some(img) => imageops::resize(&img.to_rgba8(), w, h, imageops::FilterType::Triangle),
            None => RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 255 / w) as u8 ^ seed, (y * 255 / h) as u8, 120 + seed / 2, 255])),
        }
    };
    let (a, b) = (load("H3_SHEET_A", 0), load("H3_SHEET_B", 90));
    let mut renderer = TransitionRenderer::new().expect("a graphics adapter");
    let originals: Vec<_> = h3_transitions::catalog().into_iter().filter(|t| t["origin"] == "original").map(|t| t["id"].as_str().unwrap().to_string()).collect();
    let steps = [0.0f32, 0.25, 0.5, 0.75, 1.0];
    let (cw, ch) = (w / 2, h / 2);
    let mut sheet = RgbaImage::new(cw * steps.len() as u32, ch * originals.len() as u32);
    let mut frame = Vec::new();
    for (row, id) in originals.iter().enumerate() {
        let params = params_from_json(transition(id).unwrap(), &json!({})).unwrap();
        for (col, progress) in steps.iter().enumerate() {
            renderer.render(&params, w, h, a.as_raw(), b.as_raw(), *progress, &mut frame).unwrap();
            let picture = RgbaImage::from_raw(w, h, frame.clone()).unwrap();
            let small = imageops::resize(&picture, cw, ch, imageops::FilterType::Triangle);
            imageops::replace(&mut sheet, &small, i64::from(cw) * col as i64, i64::from(ch) * row as i64);
        }
        println!("row {row}: {id}");
    }
    let path = std::path::Path::new(&dir).join("originals_sheet.png");
    sheet.save(&path).unwrap();
    println!("{}x{} sheet: {}", sheet.width(), sheet.height(), path.display());
}
