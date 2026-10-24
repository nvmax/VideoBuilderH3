//! `grade` against pictures the pack's own torch code made (`tools/make_grade_fixtures.py`). The pack saves 8 bit pictures
//! by cutting off the fraction, so a pixel may differ by one where a value sits on a step.

use std::path::PathBuf;

use h3_media::grade::{Adjust, Cube, Rgb};
use serde_json::Value;

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/post")
}

fn load(name: &str) -> Rgb {
    let picture = image::open(folder().join(name)).unwrap().to_rgb8();
    Rgb::from_bytes(picture.width() as usize, picture.height() as usize, picture.as_raw())
}

fn compare(name: &str, got: &Rgb, expected: &Rgb) {
    assert_eq!((got.width, got.height), (expected.width, expected.height), "{name}: size");
    let (a, b) = (got.to_bytes(), expected.to_bytes());
    let mut off_by_one = 0;
    for (index, (x, y)) in a.iter().zip(&b).enumerate() {
        let diff = x.abs_diff(*y);
        assert!(diff <= 2, "{name}: byte {index} is {x}, the pack made {y}");
        off_by_one += usize::from(diff > 0);
    }
    assert!(off_by_one * 100 <= a.len(), "{name}: {off_by_one} of {} bytes differ", a.len());
}

#[test]
fn adjust_and_lut_match_the_pack() {
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(folder().join("cases.json")).unwrap()).unwrap();
    let source = load("source.png");
    let mut checked = 0;
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let expected = load(case["output"].as_str().unwrap());
        let mut got = source.clone();
        match case["kind"].as_str().unwrap() {
            "adjust" => {
                let adjust = Adjust::from_settings(Some(&case["settings"]));
                assert_eq!(adjust.to_json(), case["normalized"], "{name}: normalized settings");
                adjust.apply(&mut got);
            }
            "lut" => {
                let text = std::fs::read_to_string(folder().join(case["cube"].as_str().unwrap())).unwrap();
                Cube::parse(&text, case["cube"].as_str().unwrap()).unwrap().apply(&mut got, case["strength"].as_f64().unwrap());
            }
            other => panic!("unknown case kind {other}"),
        }
        compare(name, &got, &expected);
        checked += 1;
    }
    assert_eq!(checked, 11);
}
