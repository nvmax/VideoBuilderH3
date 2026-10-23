//! Resolution-driven tile planning for 2 Pass Advanced. Port of `minimax/tile_plan.py`.

use serde::Serialize;

use crate::error::CoreError;
use crate::pyval::round_half_even;

struct VramPreset {
    key: &'static str,
    tile_megapixels: f64,
    chunk: i64,
    overlap: i64,
}

/// 2 Pass Advanced is for cards up to 24 GB. Bigger cards use plain 2 Pass or single pass.
const VRAM_PRESETS: [VramPreset; 4] = [
    VramPreset { key: "8gb", tile_megapixels: 0.2, chunk: 51, overlap: 128 },
    VramPreset { key: "12gb", tile_megapixels: 0.3, chunk: 85, overlap: 128 },
    VramPreset { key: "16gb", tile_megapixels: 0.43, chunk: 119, overlap: 128 },
    VramPreset { key: "24gb", tile_megapixels: 0.65, chunk: 153, overlap: 160 },
];

pub const DEFAULT_VRAM_PRESET: &str = "16gb";
const LATENT_TOKEN_PX: i64 = 16;
const MAX_GRID: i64 = 9;
const DEFAULT_FADE_PX: i64 = 64;
const DEFAULT_MIN_TILE_PX: i64 = 256;
const TEMPORAL_OVERLAP_FRAMES: i64 = 17;

fn unknown_preset(value: &str) -> CoreError {
    CoreError::value(format!("Unknown VRAM preset '{value}'. 2 Pass Advanced supports: 8gb, 12gb, 16gb, 24gb."))
}

fn find_preset(key: &str) -> Option<&'static VramPreset> {
    VRAM_PRESETS.iter().find(|p| p.key == key)
}

/// A supported preset key. Retired values (`32gb`, `custom`) map to `24gb`, empty to the default.
pub fn normalize_vram_preset(value: Option<&str>) -> Result<String, CoreError> {
    let raw = value.unwrap_or("");
    let key = raw.trim().to_lowercase();
    if key.is_empty() {
        return Ok(DEFAULT_VRAM_PRESET.to_string());
    }
    let key = match key.as_str() {
        "32gb" | "custom" => "24gb".to_string(),
        _ => key,
    };
    if find_preset(&key).is_none() {
        return Err(unknown_preset(raw));
    }
    Ok(key)
}

fn ceil_div(a: i64, b: i64) -> i64 {
    -((-a).div_euclid(b))
}

/// `(tile_px, overlap_px)` so `count` equal tiles cover `total_px`.
pub fn solve_equal_tiles(total_px: i64, count: i64, base_overlap_px: i64, granularity: i64) -> (i64, i64) {
    let g = granularity;
    let pg = 2 * g;
    let total = total_px;
    if count <= 1 {
        return (ceil_div(total, pg) * pg, 0);
    }
    let start = ceil_div((total + (count - 1) * base_overlap_px).div_euclid(count), pg) * pg;
    let upper = total - g * (count - 1);
    if start <= upper {
        let mut size = start;
        while size <= upper {
            let num = count * size - total;
            if num.rem_euclid(count - 1) == 0 {
                let overlap = num.div_euclid(count - 1);
                if overlap.rem_euclid(g) == 0 && 0 <= overlap && overlap <= size - g {
                    return (size, overlap);
                }
            }
            size += pg;
        }
    }
    let size = if start <= upper { start } else { total };
    let exact = (count * size - total) as f64 / (count - 1) as f64 / g as f64;
    let overlap = (round_half_even(exact) * g).clamp(0, (size - g).max(0));
    (size, overlap)
}

fn best_grid(width: i64, height: i64, target_area: f64) -> (i64, i64) {
    let mut best: Option<(f64, i64, i64)> = None;
    for rows in 1..=MAX_GRID {
        for cols in 1..=MAX_GRID {
            let tile_w = width as f64 / cols as f64;
            let tile_h = height as f64 / rows as f64;
            let area_error = (tile_w * tile_h / target_area).ln();
            let shape_error = (tile_w / tile_h / (width as f64 / height as f64)).ln();
            // Going over the target area costs double: that is what runs out of memory.
            let cost = (if area_error > 0.0 { 2.0 * area_error } else { -area_error }) + 0.5 * shape_error.abs();
            if best.is_none_or(|b| cost < b.0 - 1e-9) {
                best = Some((cost, rows, cols));
            }
        }
    }
    let (_, rows, cols) = best.unwrap_or((0.0, 1, 1));
    (rows, cols)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TilePlan {
    pub vram_preset: String,
    pub width: i64,
    pub height: i64,
    pub grid_rows: i64,
    pub grid_cols: i64,
    pub spatial_w_overlap: i64,
    pub spatial_h_overlap: i64,
    pub fade_width: i64,
    pub fade_height: i64,
    pub min_tile_size: i64,
    pub chunk_length: i64,
    pub temporal_overlap: i64,
    pub tile_width: i64,
    pub tile_height: i64,
    pub solved_overlap_w: i64,
    pub solved_overlap_h: i64,
}

/// Every resolution-dependent tiling setting for a Pass 2 frame size. The preset is not remapped here, so retired
/// values fail, as in the pack.
pub fn plan_spatial_tiles(width: i64, height: i64, vram_preset: &str) -> Result<TilePlan, CoreError> {
    let key = vram_preset.trim().to_lowercase();
    let preset = find_preset(&key).ok_or_else(|| unknown_preset(vram_preset))?;
    if width <= 0 || height <= 0 || width % 32 != 0 || height % 32 != 0 {
        return Err(CoreError::value(format!("Frame size must be positive multiples of 32 pixels; got {width}x{height}.")));
    }
    let (mut rows, mut cols) = best_grid(width, height, preset.tile_megapixels * 1048576.0);
    let overlap = preset.overlap;
    // The node errors if a solved tile is below min_tile_size, so drop to fewer tiles on an axis until it fits.
    let (tile_w, solved_ow, tile_h, solved_oh) = loop {
        let (tile_w, solved_ow) = solve_equal_tiles(width, cols, overlap, LATENT_TOKEN_PX);
        let (tile_h, solved_oh) = solve_equal_tiles(height, rows, overlap, LATENT_TOKEN_PX);
        if tile_w >= DEFAULT_MIN_TILE_PX && tile_h >= DEFAULT_MIN_TILE_PX {
            break (tile_w, solved_ow, tile_h, solved_oh);
        }
        if tile_w < DEFAULT_MIN_TILE_PX && cols > 1 {
            cols -= 1;
        } else if tile_h < DEFAULT_MIN_TILE_PX && rows > 1 {
            rows -= 1;
        } else {
            break (tile_w, solved_ow, tile_h, solved_oh);
        }
    };
    Ok(TilePlan {
        vram_preset: key,
        width,
        height,
        grid_rows: rows,
        grid_cols: cols,
        spatial_w_overlap: overlap,
        spatial_h_overlap: overlap,
        fade_width: DEFAULT_FADE_PX.min(overlap),
        fade_height: DEFAULT_FADE_PX.min(overlap),
        min_tile_size: DEFAULT_MIN_TILE_PX,
        chunk_length: preset.chunk,
        temporal_overlap: TEMPORAL_OVERLAP_FRAMES,
        tile_width: tile_w,
        tile_height: tile_h,
        solved_overlap_w: solved_ow,
        solved_overlap_h: solved_oh,
    })
}
