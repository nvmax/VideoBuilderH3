//! Pixel work for the Post Process tools: `.cube` LUTs, color adjust and film grain on float RGB pictures. Ports of the
//! torch code in `post_process/luts.py` and `post_process/lut_video_tools.py`, run on the CPU. Values are 0 to 1 and
//! the layout is `[row][column][r, g, b]`, as in the pack's `[1, height, width, 3]` tensors.

use std::path::Path;
use std::thread;

use h3_core::pyval::{to_float, Obj};
use serde_json::{json, Value};

/// A picture of float RGB values between 0 and 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Rgb {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height * 3] }
    }

    /// From 8 bit RGB bytes.
    pub fn from_bytes(width: usize, height: usize, bytes: &[u8]) -> Self {
        Self { width, height, data: bytes.iter().map(|&b| f32::from(b) / 255.0).collect() }
    }

    /// 8 bit RGB bytes, cut off (not rounded) like the pack's `astype(np.uint8)`.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.iter().map(|&v| (v * 255.0).clamp(0.0, 255.0) as u8).collect()
    }

    /// Write the 8 bit bytes of the picture into `out`, which must be as long as the data.
    pub fn write_bytes(&self, out: &mut [u8]) {
        for (byte, &v) in out.iter_mut().zip(&self.data) {
            *byte = (v * 255.0).clamp(0.0, 255.0) as u8;
        }
    }

    pub fn read_bytes(&mut self, bytes: &[u8]) {
        for (value, &b) in self.data.iter_mut().zip(bytes) {
            *value = f32::from(b) / 255.0;
        }
    }
}

/// Run `work(first_row, rows)` on slices of whole rows across the available cores.
fn for_rows(data: &mut [f32], row_len: usize, work: impl Fn(usize, &mut [f32]) + Sync) {
    let rows = data.len().checked_div(row_len).unwrap_or(0);
    let threads = thread::available_parallelism().map_or(1, |n| n.get()).min(rows.div_ceil(16)).max(1);
    if threads == 1 {
        work(0, data);
        return;
    }
    let per = rows.div_ceil(threads);
    thread::scope(|scope| {
        for (index, part) in data.chunks_mut(per * row_len).enumerate() {
            let work = &work;
            scope.spawn(move || work(index * per, part));
        }
    });
}

// ---------------------------------------------------------------------------------------------------------------------
// .cube LUTs

/// A 3D LUT from a `.cube` file.
#[derive(Debug, Clone)]
pub struct Cube {
    pub size: usize,
    /// `[blue][green][red][rgb]`: red changes fastest in the file.
    table: Vec<f32>,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
}

fn python_float(part: &str) -> Result<f64, String> {
    part.parse::<f64>().map_err(|_| format!("could not convert string to float: '{part}'"))
}

impl Cube {
    /// Read a `.cube` file. `label` is the path the messages name. Port of `VRGDG_LUTS._parse_cube_file`.
    pub fn parse(text: &str, label: &str) -> Result<Self, String> {
        let name = Path::new(label).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| label.to_string());
        let mut size: Option<usize> = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut values: Vec<f32> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let upper = line.to_uppercase();
            let parts: Vec<&str> = line.split_whitespace().collect();
            if upper.starts_with("TITLE ") {
                continue;
            }
            if upper.starts_with("LUT_1D_SIZE") {
                return Err(format!("1D LUTs are not supported: {name}"));
            }
            if upper.starts_with("LUT_3D_SIZE") {
                if parts.len() != 2 {
                    return Err(format!("Invalid LUT_3D_SIZE line in {label}"));
                }
                size = Some(parts[1].parse::<usize>().map_err(|_| format!("invalid literal for int() with base 10: '{}'", parts[1]))?);
                continue;
            }
            for (prefix, target) in [("DOMAIN_MIN", &mut domain_min), ("DOMAIN_MAX", &mut domain_max)] {
                if upper.starts_with(prefix) {
                    if parts.len() != 4 {
                        return Err(format!("Invalid {prefix} line in {label}"));
                    }
                    for (slot, part) in target.iter_mut().zip(&parts[1..]) {
                        *slot = python_float(part)? as f32;
                    }
                }
            }
            if upper.starts_with("DOMAIN_MIN") || upper.starts_with("DOMAIN_MAX") {
                continue;
            }
            if parts.len() != 3 {
                continue;
            }
            for part in parts {
                values.push(python_float(part)? as f32);
            }
        }
        let size = size.ok_or_else(|| format!("Missing LUT_3D_SIZE in {label}"))?;
        let expected = size * size * size * 3;
        if values.len() != expected {
            return Err(format!("Invalid LUT data length in {label}. Expected {expected} floats, got {}.", values.len()));
        }
        Ok(Self { size, table: values, domain_min, domain_max })
    }

    fn entry(&self, b: usize, g: usize, r: usize) -> [f32; 3] {
        let at = ((b * self.size + g) * self.size + r) * 3;
        [self.table[at], self.table[at + 1], self.table[at + 2]]
    }

    /// The graded color of one pixel, trilinear between the nearest table entries. Port of `_apply_cube_lut`.
    fn grade(&self, rgb: [f32; 3]) -> [f32; 3] {
        let max_index = self.size.saturating_sub(1);
        let mut coords = [0.0f32; 3];
        for channel in 0..3 {
            let span = (self.domain_max[channel] - self.domain_min[channel]).max(1e-6);
            coords[channel] = ((rgb[channel] - self.domain_min[channel]) / span).clamp(0.0, 1.0) * max_index as f32;
        }
        let low = coords.map(|c| c.floor() as usize);
        let high = [(low[0] + 1).min(max_index), (low[1] + 1).min(max_index), (low[2] + 1).min(max_index)];
        let (fr, fg, fb) = (coords[0] - low[0] as f32, coords[1] - low[1] as f32, coords[2] - low[2] as f32);
        let (r0, g0, b0, r1, g1, b1) = (low[0], low[1], low[2], high[0], high[1], high[2]);
        let c000 = self.entry(b0, g0, r0);
        let c001 = self.entry(b1, g0, r0);
        let c010 = self.entry(b0, g1, r0);
        let c011 = self.entry(b1, g1, r0);
        let c100 = self.entry(b0, g0, r1);
        let c101 = self.entry(b1, g0, r1);
        let c110 = self.entry(b0, g1, r1);
        let c111 = self.entry(b1, g1, r1);
        let mut out = [0.0f32; 3];
        for k in 0..3 {
            let c00 = c000[k] * (1.0 - fb) + c001[k] * fb;
            let c01 = c010[k] * (1.0 - fb) + c011[k] * fb;
            let c10 = c100[k] * (1.0 - fb) + c101[k] * fb;
            let c11 = c110[k] * (1.0 - fb) + c111[k] * fb;
            let c0 = c00 * (1.0 - fg) + c01 * fg;
            let c1 = c10 * (1.0 - fg) + c11 * fg;
            out[k] = (c0 * (1.0 - fr) + c1 * fr).clamp(0.0, 1.0);
        }
        out
    }

    /// Grade a picture and blend it with the original: `strength` is 0 to 10, 10 is the full LUT.
    pub fn apply(&self, image: &mut Rgb, strength: f64) {
        let blend = (strength.clamp(0.0, 10.0) / 10.0) as f32;
        if blend <= 0.0 {
            return;
        }
        let row_len = image.width * 3;
        for_rows(&mut image.data, row_len.max(1), |_, rows| {
            for pixel in rows.as_chunks_mut::<3>().0 {
                let graded = self.grade([pixel[0], pixel[1], pixel[2]]);
                for k in 0..3 {
                    pixel[k] = if blend < 1.0 { pixel[k] * (1.0 - blend) + graded[k] * blend } else { graded[k] };
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Adjust

/// The Adjust sliders, kept inside their ranges. Port of `_normalize_adjust_settings`.
#[derive(Debug, Clone, PartialEq)]
pub struct Adjust {
    pub enabled: bool,
    pub temperature: f64,
    pub tint: f64,
    pub saturation: f64,
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
    pub sharpen: f64,
    pub clarity: f64,
    pub vignette: f64,
    pub fade: f64,
}

impl Adjust {
    /// `settings` is whatever the request held. Anything that is not an object counts as empty.
    pub fn from_settings(settings: Option<&Value>) -> Self {
        let empty = Obj::new();
        let settings = settings.and_then(Value::as_object).unwrap_or(&empty);
        let read = |key: &str, min: f64, max: f64| -> f64 {
            let value = settings.get(key).and_then(to_float).unwrap_or(0.0);
            // `max(minimum, min(maximum, value))`
            let upper = if value < max { value } else { max };
            if min > upper { min } else { upper }
        };
        Self {
            enabled: settings.get("enabled") != Some(&Value::Bool(false)),
            temperature: read("temperature", -100.0, 100.0),
            tint: read("tint", -100.0, 100.0),
            saturation: read("saturation", -100.0, 100.0),
            exposure: read("exposure", -100.0, 100.0),
            contrast: read("contrast", -100.0, 100.0),
            highlights: read("highlights", -100.0, 100.0),
            shadows: read("shadows", -100.0, 100.0),
            whites: read("whites", -100.0, 100.0),
            blacks: read("blacks", -100.0, 100.0),
            sharpen: read("sharpen", 0.0, 100.0),
            clarity: read("clarity", -100.0, 100.0),
            vignette: read("vignette", 0.0, 100.0),
            fade: read("fade", 0.0, 100.0),
        }
    }

    /// The normalized settings as the pack reports them.
    pub fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "temperature": self.temperature,
            "tint": self.tint,
            "saturation": self.saturation,
            "exposure": self.exposure,
            "contrast": self.contrast,
            "highlights": self.highlights,
            "shadows": self.shadows,
            "whites": self.whites,
            "blacks": self.blacks,
            "sharpen": self.sharpen,
            "clarity": self.clarity,
            "vignette": self.vignette,
            "fade": self.fade,
        })
    }

    /// Port of `_apply_adjust_tensor`.
    pub fn apply(&self, image: &mut Rgb) {
        for value in &mut image.data {
            *value = value.clamp(0.0, 1.0);
        }
        if !self.enabled || image.data.is_empty() {
            return;
        }
        let (width, height) = (image.width, image.height);
        let tone = Tone::new(self);
        for_rows(&mut image.data, width * 3, |_, rows| {
            for pixel in rows.as_chunks_mut::<3>().0 {
                tone.apply(pixel);
            }
        });

        let clarity = (self.clarity / 100.0) as f32;
        let sharpen = (self.sharpen / 100.0) as f32;
        if clarity.abs() > 0.001 || sharpen > 0.001 {
            if clarity.abs() > 0.001 {
                let blurred = box_blur(&image.data, width, height, odd_kernel(9, width, height), Padding::Reflect);
                let source = std::mem::take(&mut image.data);
                image.data = source
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .zip(blurred.as_chunks::<3>().0)
                    .flat_map(|(pixel, blur)| {
                        let luma = pixel[0] * 0.2126 + pixel[1] * 0.7152 + pixel[2] * 0.0722;
                        let midtone = 1.0 - ((luma - 0.5).abs() / 0.5).clamp(0.0, 1.0);
                        let gain = clarity * 1.55 * (0.35 + midtone * 0.65);
                        [pixel[0] + (pixel[0] - blur[0]) * gain, pixel[1] + (pixel[1] - blur[1]) * gain, pixel[2] + (pixel[2] - blur[2]) * gain]
                    })
                    .collect();
            }
            if sharpen > 0.001 {
                let blurred = box_blur(&image.data, width, height, 3, Padding::Replicate);
                for (value, blur) in image.data.iter_mut().zip(&blurred) {
                    *value += (*value - blur) * sharpen * 5.0;
                }
            }
        }

        let fade = (self.fade / 100.0) as f32;
        let vignette = (self.vignette / 100.0) as f32;
        let vignette_rows = linspace(height);
        let vignette_columns = linspace(width);
        for_rows(&mut image.data, width * 3, |first_row, rows| {
            for (row_offset, row) in rows.chunks_exact_mut(width * 3).enumerate() {
                let y = vignette_rows[first_row + row_offset];
                for (x_index, pixel) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let mask = if vignette > 0.0 {
                        let x = vignette_columns[x_index];
                        let distance = (x * x + y * y).sqrt();
                        1.0 - ((distance - 0.35) / 1.05).clamp(0.0, 1.0) * vignette * 0.75
                    } else {
                        1.0
                    };
                    for value in pixel {
                        let faded = if fade > 0.0 { *value * (1.0 - fade * 0.35) + fade * 0.18 } else { *value };
                        *value = (faded * mask).clamp(0.0, 1.0);
                    }
                }
            }
        });
    }
}

/// The per pixel steps of Adjust before the blur based ones: color shift, exposure, contrast, saturation and the
/// highlight, shadow, white and black lifts.
struct Tone {
    shift: [f32; 3],
    exposure: f32,
    contrast: f32,
    saturation: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
}

impl Tone {
    fn new(adjust: &Adjust) -> Self {
        Self {
            shift: [
                (adjust.temperature / 400.0 - adjust.tint / 900.0) as f32,
                (adjust.tint / 450.0) as f32,
                (-adjust.temperature / 400.0 - adjust.tint / 900.0) as f32,
            ],
            exposure: 2.0f64.powf(adjust.exposure / 100.0) as f32,
            contrast: (1.0 + adjust.contrast / 100.0) as f32,
            saturation: (1.0 + adjust.saturation / 100.0) as f32,
            highlights: (adjust.highlights / 220.0) as f32,
            shadows: (adjust.shadows / 220.0) as f32,
            whites: (adjust.whites / 240.0) as f32,
            blacks: (adjust.blacks / 240.0) as f32,
        }
    }

    fn apply(&self, pixel: &mut [f32]) {
        let mut out = [pixel[0] + self.shift[0], pixel[1] + self.shift[1], pixel[2] + self.shift[2]];
        for value in &mut out {
            *value *= self.exposure;
            *value = (*value - 0.5) * self.contrast + 0.5;
        }
        let gray = out[0] * 0.2126 + out[1] * 0.7152 + out[2] * 0.0722;
        for value in &mut out {
            *value = gray + (*value - gray) * self.saturation;
        }
        let luma = out[0] * 0.2126 + out[1] * 0.7152 + out[2] * 0.0722;
        let highlight_mask = ((luma - 0.55) / 0.45).clamp(0.0, 1.0);
        let shadow_mask = ((0.45 - luma) / 0.45).clamp(0.0, 1.0);
        let white_mask = ((luma - 0.75) / 0.25).clamp(0.0, 1.0);
        let black_mask = ((0.25 - luma) / 0.25).clamp(0.0, 1.0);
        for (slot, value) in pixel.iter_mut().zip(out) {
            let mut v = value + highlight_mask * self.highlights;
            v += shadow_mask * self.shadows;
            v += white_mask * self.whites;
            v += black_mask * self.blacks;
            *slot = v;
        }
    }
}

/// `torch.linspace(-1, 1, n)`.
fn linspace(n: usize) -> Vec<f32> {
    if n <= 1 {
        return vec![-1.0; n];
    }
    let step = 2.0 / (n - 1) as f32;
    (0..n).map(|i| if i < n / 2 { -1.0 + step * i as f32 } else { 1.0 - step * (n - 1 - i) as f32 }).collect()
}

/// The blur width Adjust asks for, shrunk to fit small pictures. Below 3 there is no blur.
fn odd_kernel(target: usize, width: usize, height: usize) -> usize {
    let fit = |n: usize| if n % 2 == 1 { n } else { n.saturating_sub(1) };
    target.min(fit(height)).min(fit(width))
}

#[derive(Clone, Copy)]
enum Padding {
    Reflect,
    Replicate,
}

fn padded_index(index: isize, size: usize, padding: Padding) -> usize {
    let last = size as isize - 1;
    let mapped = match padding {
        Padding::Replicate => index.clamp(0, last),
        Padding::Reflect => {
            let mut i = index;
            if i < 0 {
                i = -i;
            }
            if i > last {
                i = 2 * last - i;
            }
            i.clamp(0, last)
        }
    };
    mapped as usize
}

/// A box average of `kernel` by `kernel` pixels after padding the edges, per channel. Below 3 it returns the picture.
/// Port of the pack's `F.avg_pool2d(F.pad(...))`, done as two passes.
fn box_blur(data: &[f32], width: usize, height: usize, kernel: usize, padding: Padding) -> Vec<f32> {
    if kernel < 3 {
        return data.to_vec();
    }
    let radius = (kernel / 2) as isize;
    let inverse = 1.0 / kernel as f32;
    let row_len = width * 3;
    let mut across = vec![0.0f32; data.len()];
    for_rows(&mut across, row_len, |first_row, rows| {
        for (offset, out_row) in rows.chunks_exact_mut(row_len).enumerate() {
            let source = &data[(first_row + offset) * row_len..][..row_len];
            for x in 0..width {
                for channel in 0..3 {
                    let mut sum = 0.0f32;
                    for d in -radius..=radius {
                        sum += source[padded_index(x as isize + d, width, padding) * 3 + channel];
                    }
                    out_row[x * 3 + channel] = sum * inverse;
                }
            }
        }
    });
    let mut result = vec![0.0f32; data.len()];
    for_rows(&mut result, row_len, |first_row, rows| {
        for (offset, out_row) in rows.chunks_exact_mut(row_len).enumerate() {
            let y = (first_row + offset) as isize;
            for (i, slot) in out_row.iter_mut().enumerate() {
                let mut sum = 0.0f32;
                for d in -radius..=radius {
                    sum += across[padded_index(y + d, height, padding) * row_len + i];
                }
                *slot = sum * inverse;
            }
        }
    });
    result
}

// ---------------------------------------------------------------------------------------------------------------------
// Film grain

/// A small seedable generator of normally distributed numbers (xorshift and Box-Muller). The pack uses torch's `randn`,
/// whose numbers cannot be matched, so a seed repeats the grain here and not the pack's grain.
struct Noise {
    state: u64,
    spare: Option<f32>,
}

impl Noise {
    fn new(seed: u64) -> Self {
        // splitmix64 spreads small seeds
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Self { state: (z ^ (z >> 31)) | 1, spare: None }
    }

    fn unit(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        // 24 bits fill an f32 mantissa; the result is in (0, 1]
        ((self.state >> 40) as f32 + 1.0) / 16_777_216.0
    }

    fn normal(&mut self) -> f32 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let (a, b) = (self.unit(), self.unit());
        let radius = (-2.0 * a.ln()).sqrt();
        let angle = std::f32::consts::TAU * b;
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }
}

/// A seed from the clock for grain that should differ every time.
pub fn clock_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

/// Add film grain: red noise twice and blue noise three times as strong as green, mixed toward gray by
/// `saturation_mix`. Port of `_apply_film_grain_tensor`.
pub fn apply_film_grain(image: &mut Rgb, grain_intensity: f64, saturation_mix: f64, seed: u64) {
    let intensity = grain_intensity.clamp(0.0, 1.0) as f32;
    let saturation = saturation_mix.clamp(0.0, 1.0) as f32;
    let mut noise = Noise::new(seed);
    for pixel in image.data.as_chunks_mut::<3>().0 {
        let r = noise.normal() * 2.0;
        let g = noise.normal();
        let b = noise.normal() * 3.0;
        let mixed = [saturation * r + (1.0 - saturation) * g, g, saturation * b + (1.0 - saturation) * g];
        for k in 0..3 {
            pixel[k] = (pixel[k] + mixed[k] * intensity).clamp(0.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: usize, height: usize) -> Rgb {
        let mut image = Rgb::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let at = (y * width + x) * 3;
                image.data[at] = x as f32 / (width - 1).max(1) as f32;
                image.data[at + 1] = y as f32 / (height - 1).max(1) as f32;
                image.data[at + 2] = ((x + y) % 7) as f32 / 6.0;
            }
        }
        image
    }

    fn identity_cube(size: usize) -> String {
        let mut text = format!("TITLE \"identity\"\nLUT_3D_SIZE {size}\n");
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let n = (size - 1) as f32;
                    text.push_str(&format!("{:.6} {:.6} {:.6}\n", r as f32 / n, g as f32 / n, b as f32 / n));
                }
            }
        }
        text
    }

    #[test]
    fn an_identity_lut_keeps_the_picture() {
        let cube = Cube::parse(&identity_cube(9), "identity.cube").unwrap();
        let mut image = gradient(20, 13);
        let before = image.clone();
        cube.apply(&mut image, 10.0);
        for (a, b) in image.data.iter().zip(&before.data) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }

    #[test]
    fn lut_errors_name_the_problem() {
        assert!(Cube::parse("LUT_1D_SIZE 4\n", "C:\\luts\\a.cube").unwrap_err().contains("1D LUTs are not supported: a.cube"));
        assert!(Cube::parse("0 0 0\n", "a.cube").unwrap_err().contains("Missing LUT_3D_SIZE in a.cube"));
        assert!(Cube::parse("LUT_3D_SIZE 2\n0 0 0\n", "a.cube").unwrap_err().contains("Expected 24 floats, got 3."));
    }

    #[test]
    fn strength_zero_leaves_the_picture_alone() {
        let cube = Cube::parse("LUT_3D_SIZE 2\n1 1 1\n1 1 1\n1 1 1\n1 1 1\n1 1 1\n1 1 1\n1 1 1\n1 1 1\n", "white.cube").unwrap();
        let mut image = gradient(5, 5);
        let before = image.clone();
        cube.apply(&mut image, 0.0);
        assert_eq!(image, before);
        cube.apply(&mut image, 10.0);
        assert!(image.data.iter().all(|&v| v == 1.0));
    }

    #[test]
    fn adjust_settings_are_kept_in_range() {
        let settings = json!({"temperature": 500, "sharpen": -4, "tint": "12", "enabled": false, "fade": null});
        let adjust = Adjust::from_settings(Some(&settings));
        assert_eq!((adjust.temperature, adjust.sharpen, adjust.tint, adjust.fade, adjust.enabled), (100.0, 0.0, 12.0, 0.0, false));
        assert!(Adjust::from_settings(Some(&json!("nope"))).enabled);
    }

    #[test]
    fn adjust_at_zero_keeps_the_picture() {
        let adjust = Adjust::from_settings(None);
        let mut image = gradient(17, 11);
        let before = image.clone();
        adjust.apply(&mut image);
        for (a, b) in image.data.iter().zip(&before.data) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn grain_is_repeatable_with_a_seed_and_centered() {
        let mut one = Rgb { width: 64, height: 64, data: vec![0.5; 64 * 64 * 3] };
        let mut two = one.clone();
        apply_film_grain(&mut one, 0.1, 1.0, 7);
        apply_film_grain(&mut two, 0.1, 1.0, 7);
        assert_eq!(one, two);
        let mean = one.data.iter().sum::<f32>() / one.data.len() as f32;
        assert!((mean - 0.5).abs() < 0.01, "{mean}");
        let mut flat = Rgb { width: 4, height: 4, data: vec![0.5; 48] };
        apply_film_grain(&mut flat, 0.0, 1.0, 1);
        assert!(flat.data.iter().all(|&v| v == 0.5));
    }

    #[test]
    fn small_pictures_do_not_break_the_blur() {
        let adjust = Adjust::from_settings(Some(&json!({"clarity": 80, "sharpen": 60, "vignette": 50, "fade": 20})));
        for (w, h) in [(1, 1), (2, 3), (3, 2), (5, 1)] {
            let mut image = gradient(w, h);
            adjust.apply(&mut image);
            assert!(image.data.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
        }
    }
}
