//! Resolution and tile plan against `fixtures/core`.

mod common;

use h3_core::pyval::Obj;
use h3_core::resolution::{frame_size, migrate_resolution, preset_megapixels};
use h3_core::tiles::{normalize_vram_preset, plan_spatial_tiles, solve_equal_tiles};
use serde_json::{json, Value};

fn check<T: serde::Serialize>(got: Result<T, h3_core::CoreError>, c: &Value) {
    match (got, c.get("error")) {
        (Ok(v), None) => assert_eq!(serde_json::to_value(v).unwrap(), c["output"], "{c}"),
        (Err(e), Some(err)) => assert_eq!(json!(e.message), err["message"], "{c}"),
        (got, _) => panic!("ok={} vs {c}", got.is_ok()),
    }
}

#[test]
fn frame_sizes() {
    for c in common::cases("core/frame_size.json") {
        let i = &c["input"];
        let got = frame_size(i["megapixels"].as_f64().unwrap(), i["aspect_ratio"].as_str().unwrap());
        assert_eq!(json!([got.0, got.1]), c["output"], "{c}");
    }
}

#[test]
fn preset_sizes() {
    for c in common::cases("core/preset_megapixels.json") {
        let i = &c["input"];
        let got = preset_megapixels(i["preset"].as_str().unwrap_or(""), i["aspect_ratio"].as_str().unwrap_or(""));
        assert_eq!(json!(got), c["output"], "{c}");
    }
}

#[test]
fn legacy_resolution_migration() {
    for c in common::cases("core/migrate_resolution.json") {
        let raw: Obj = c["input"]["raw"].as_object().unwrap().clone();
        let (preset, mp) = migrate_resolution(&raw, c["input"]["render_pass"].as_str().unwrap());
        assert_eq!(json!([preset, mp]), c["output"], "{c}");
    }
}

#[test]
fn vram_presets() {
    for c in common::cases("core/normalize_vram_preset.json") {
        check(normalize_vram_preset(c["input"]["value"].as_str()), &c);
    }
}

#[test]
fn equal_tile_solver() {
    for c in common::cases("core/solve_equal_tiles.json") {
        let i = &c["input"];
        let got = solve_equal_tiles(
            i["total_px"].as_i64().unwrap(),
            i["count"].as_i64().unwrap(),
            i["base_overlap_px"].as_i64().unwrap(),
            i.get("granularity").and_then(Value::as_i64).unwrap_or(16),
        );
        assert_eq!(json!([got.0, got.1]), c["output"], "{c}");
    }
}

#[test]
fn tile_plans() {
    for c in common::cases("core/tile_plan.json") {
        let i = &c["input"];
        check(
            plan_spatial_tiles(i["width"].as_i64().unwrap(), i["height"].as_i64().unwrap(), i["vram_preset"].as_str().unwrap()),
            &c,
        );
    }
}
