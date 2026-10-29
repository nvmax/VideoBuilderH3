//! Overlay data: the layers of the overlay lane (media overlays such as particles and light leaks, film grain, vignette, text, a
//! whole-timeline LUT) and the track switch, saved in the session as `overlay_layers` and `overlay_layer_track`. Port of
//! `web/music_video_builder/overlay_layers.mjs` in the pack. Every value is clamped and every missing one gets its default, so a clip
//! from an old project, an agent or a typo is corrected instead of rejected. Both run `tests/overlay_layer_cases.json`.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::pyval::Obj;

pub const KINDS: [&str; 5] = ["media", "grain", "vignette", "text", "lut"];
pub const MIN_SECONDS: f64 = 0.05;
pub const MAX_SECONDS: f64 = 86400.0;
pub const MAX_CLIPS: usize = 500;
pub const BLEND_MODES: [&str; 11] = ["normal", "screen", "add", "lighten", "darken", "overlay", "softlight", "hardlight", "colordodge", "colorburn", "multiply"];
pub const SCALE_MODES: [&str; 3] = ["cover", "contain", "stretch"];
pub const GRAIN_SIZES: [&str; 3] = ["fine", "medium", "coarse"];
pub const TEXT_ANIMATIONS: [&str; 2] = ["none", "slide_up"];

const COMMON_FIELDS: [&str; 9] = ["id", "kind", "start", "end", "enabled", "locked", "opacity", "fade_in", "fade_out"];

fn kind_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "media" => &["path", "source_start", "blend_mode", "scale", "loop", "speed", "flip_h", "brightness", "contrast", "tint"],
        "grain" => &["strength", "chroma", "size", "animated"],
        "vignette" => &["strength"],
        "text" => &["text", "font", "size", "color", "outline_width", "outline_color", "shadow", "x", "y", "animation"],
        "lut" => &["lut", "strength"],
        _ => &[],
    }
}

/// Half away from zero to three decimals, so the JS, Rust and Python twins agree on every value.
fn round3(value: f64) -> f64 {
    let rounded = value.signum() * (value.abs() * 1000.0).round() / 1000.0;
    if rounded == 0.0 { 0.0 } else { rounded }
}

/// Text that JavaScript's `Number()` reads as a plain decimal: sign, digits with one optional point, optional exponent.
fn plain_decimal(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let whole = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    i += whole;
    let mut fraction = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        fraction = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        i += fraction;
    }
    if whole == 0 && fraction == 0 {
        return false;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exponent = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exponent == 0 {
            return false;
        }
        i += exponent;
    }
    i == bytes.len()
}

fn number(value: Option<&Value>, fallback: f64, low: f64, high: f64) -> f64 {
    let parsed = match value {
        Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()).unwrap_or(fallback),
        Some(Value::String(s)) if plain_decimal(s.trim()) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()).unwrap_or(fallback),
        _ => fallback,
    };
    round3(parsed.max(low).min(high))
}

fn flag(value: Option<&Value>, fallback: bool) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        _ => fallback,
    }
}

fn choice(value: Option<&Value>, allowed: &[&str], fallback: &str) -> String {
    let text = value.and_then(Value::as_str).map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if allowed.contains(&text.as_str()) { text } else { fallback.to_string() }
}

fn color(value: Option<&Value>, fallback: &str) -> String {
    let text = value.and_then(Value::as_str).map(|s| s.trim().to_lowercase()).unwrap_or_default();
    let hex = text.strip_prefix('#').filter(|h| h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    match hex {
        Some(h) if h.len() == 6 => text,
        Some(h) if h.len() == 3 => format!("#{}", h.chars().flat_map(|c| [c, c]).collect::<String>()),
        _ => fallback.to_string(),
    }
}

/// The first `limit` characters of a text value, or `fallback` when it is not text.
fn words(value: Option<&Value>, limit: usize, fallback: &str) -> String {
    match value {
        Some(Value::String(s)) => s.chars().take(limit).collect(),
        _ => fallback.to_string(),
    }
}

fn trimmed_words(value: Option<&Value>, limit: usize, fallback: &str) -> String {
    let text = words(value, limit, fallback);
    let text = text.trim();
    if text.is_empty() { fallback.to_string() } else { text.to_string() }
}

/// One clip, or `None` when it is not an object or has no kind. `index` is its place in the saved list and `used_ids` the ids already
/// taken, so a missing or repeated id becomes `overlay_<n>` or `<id>_2`, `<id>_3`.
pub fn normalize_overlay_layer(source: &Value, index: usize, used_ids: &mut HashSet<String>) -> Option<Obj> {
    let source = source.as_object()?;
    let kind = source.get("kind").and_then(Value::as_str).map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if kind.is_empty() {
        return None;
    }
    let known: Vec<&str> = COMMON_FIELDS.iter().copied().chain(kind_fields(&kind).iter().copied()).collect();
    let mut clip = Obj::new();
    for (key, value) in source {
        if !known.contains(&key.as_str()) {
            clip.insert(key.clone(), value.clone());
        }
    }
    let base_id = trimmed_words(source.get("id"), 80, &format!("overlay_{}", index + 1));
    let mut id = base_id.clone();
    let mut n = 2;
    while used_ids.contains(&id) {
        id = format!("{base_id}_{n}");
        n += 1;
    }
    used_ids.insert(id.clone());
    let start = number(source.get("start"), 0.0, 0.0, MAX_SECONDS);
    let end = round3(start + MIN_SECONDS).max(number(source.get("end"), start + MIN_SECONDS, 0.0, MAX_SECONDS));
    let duration = round3(end - start);
    let mut fade_in = number(source.get("fade_in"), 0.3, 0.0, 5.0);
    let mut fade_out = number(source.get("fade_out"), 0.3, 0.0, 5.0);
    if fade_in + fade_out > duration {
        let scale = duration / (fade_in + fade_out);
        fade_in = round3(fade_in * scale);
        fade_out = round3(fade_out * scale);
    }
    clip.insert("id".into(), json!(id));
    clip.insert("kind".into(), json!(kind));
    clip.insert("start".into(), json!(start));
    clip.insert("end".into(), json!(end));
    clip.insert("enabled".into(), json!(flag(source.get("enabled"), true)));
    clip.insert("locked".into(), json!(flag(source.get("locked"), true)));
    clip.insert("opacity".into(), json!(number(source.get("opacity"), if kind == "media" { 0.9 } else { 1.0 }, 0.0, 1.0)));
    clip.insert("fade_in".into(), json!(fade_in));
    clip.insert("fade_out".into(), json!(fade_out));
    match kind.as_str() {
        "media" => {
            clip.insert("path".into(), json!(trimmed_words(source.get("path"), 1024, "")));
            clip.insert("source_start".into(), json!(number(source.get("source_start"), 0.0, 0.0, MAX_SECONDS)));
            clip.insert("blend_mode".into(), json!(choice(source.get("blend_mode"), &BLEND_MODES, "screen")));
            clip.insert("scale".into(), json!(choice(source.get("scale"), &SCALE_MODES, "cover")));
            clip.insert("loop".into(), json!(flag(source.get("loop"), true)));
            clip.insert("speed".into(), json!(number(source.get("speed"), 1.0, 0.5, 2.0)));
            clip.insert("flip_h".into(), json!(flag(source.get("flip_h"), false)));
            clip.insert("brightness".into(), json!(number(source.get("brightness"), 0.0, -1.0, 1.0)));
            clip.insert("contrast".into(), json!(number(source.get("contrast"), 1.0, 0.0, 2.0)));
            clip.insert("tint".into(), json!(color(source.get("tint"), "")));
        }
        "grain" => {
            clip.insert("strength".into(), json!(number(source.get("strength"), 28.0, 0.0, 60.0)));
            clip.insert("chroma".into(), json!(number(source.get("chroma"), 0.0, 0.0, 20.0)));
            clip.insert("size".into(), json!(choice(source.get("size"), &GRAIN_SIZES, "fine")));
            clip.insert("animated".into(), json!(flag(source.get("animated"), true)));
        }
        "vignette" => {
            clip.insert("strength".into(), json!(number(source.get("strength"), 0.7, 0.0, 1.0)));
        }
        "text" => {
            clip.insert("text".into(), json!(words(source.get("text"), 500, "")));
            clip.insert("font".into(), json!(trimmed_words(source.get("font"), 80, "Anton")));
            clip.insert("size".into(), json!(number(source.get("size"), 96.0, 8.0, 400.0)));
            clip.insert("color".into(), json!(color(source.get("color"), "#ffffff")));
            clip.insert("outline_width".into(), json!(number(source.get("outline_width"), 3.0, 0.0, 20.0)));
            clip.insert("outline_color".into(), json!(color(source.get("outline_color"), "#000000")));
            clip.insert("shadow".into(), json!(number(source.get("shadow"), 4.0, 0.0, 20.0)));
            clip.insert("x".into(), json!(number(source.get("x"), 0.5, 0.0, 1.0)));
            clip.insert("y".into(), json!(number(source.get("y"), 0.78, 0.0, 1.0)));
            clip.insert("animation".into(), json!(choice(source.get("animation"), &TEXT_ANIMATIONS, "none")));
        }
        "lut" => {
            clip.insert("lut".into(), json!(trimmed_words(source.get("lut"), 260, "")));
            clip.insert("strength".into(), json!(number(source.get("strength"), 1.0, 0.0, 1.0)));
        }
        _ => {}
    }
    Some(clip)
}

/// The saved list, in track order (later clips draw on top). Anything that is not a clip is dropped.
pub fn normalize_overlay_layers(source: &Value) -> Vec<Obj> {
    let Some(items) = source.as_array() else { return Vec::new() };
    let mut used_ids = HashSet::new();
    let mut clips = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if clips.len() >= MAX_CLIPS {
            break;
        }
        if let Some(clip) = normalize_overlay_layer(item, index, &mut used_ids) {
            clips.push(clip);
        }
    }
    clips
}

/// The clips of a project timeline as they sit on a video that starts `offset` seconds into the timeline (a render of a few scenes starts
/// where its first scene does). A clip that ends before the video starts is dropped. A clip that began earlier starts at zero and
/// has no fade in, since that fade is already over, and a media clip starts as far into its file as it had played. The clips come back as saved
/// JSON, ready for `normalize_overlay_layers`.
pub fn shift_overlay_layers(source: &Value, offset: f64) -> Value {
    let Some(items) = source.as_array() else { return Value::Array(Vec::new()) };
    if !offset.is_finite() || offset == 0.0 {
        return source.clone();
    }
    let mut shifted = Vec::new();
    for item in items {
        let Some(map) = item.as_object() else {
            shifted.push(item.clone());
            continue;
        };
        let read = |key: &str| map.get(key).and_then(Value::as_f64);
        let (Some(start), Some(end)) = (read("start"), read("end")) else {
            shifted.push(item.clone());
            continue;
        };
        let (start, end) = (start - offset, end - offset);
        if end <= 0.0 {
            continue;
        }
        let mut clip = map.clone();
        clip.insert("start".into(), json!(start.max(0.0)));
        clip.insert("end".into(), json!(end));
        if start < 0.0 {
            clip.insert("fade_in".into(), json!(0));
            // A media clip that began before the video starts is already part way through its file.
            if map.get("kind").and_then(Value::as_str) == Some("media") {
                let speed = map.get("speed").and_then(Value::as_f64).unwrap_or(1.0);
                let before = map.get("source_start").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
                clip.insert("source_start".into(), json!(before + (-start) * speed));
            }
        }
        shifted.push(Value::Object(clip));
    }
    Value::Array(shifted)
}

/// The looks of lyric captions.
pub const TEXT_STYLES: [&str; 3] = ["clean", "bold", "lower"];

/// A line of lyrics with the time it is sung.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionLine {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The settings a caption style gives every caption clip (the same fields a text clip has). An unknown name is `clean`.
pub fn text_style(name: &str) -> Obj {
    let fields = match name {
        "bold" => json!({"font": "Anton", "size": 100, "color": "#ffffff", "outline_width": 5, "outline_color": "#000000", "shadow": 6, "x": 0.5, "y": 0.82, "animation": "none", "fade_in": 0.1, "fade_out": 0.15}),
        "lower" => json!({"font": "Oswald", "size": 60, "color": "#ffffff", "outline_width": 0, "outline_color": "#000000", "shadow": 5, "x": 0.5, "y": 0.9, "animation": "slide_up", "fade_in": 0.3, "fade_out": 0.2}),
        _ => json!({"font": "Montserrat", "size": 54, "color": "#ffffff", "outline_width": 2, "outline_color": "#000000", "shadow": 3, "x": 0.5, "y": 0.88, "animation": "none", "fade_in": 0.12, "fade_out": 0.15}),
    };
    fields.as_object().cloned().unwrap_or_default()
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The sung lines of the project's word-timed lyrics (`project_context/timestamped_lyrics.json`): every segment of type `vocal` that has words.
/// A line that ends after the next one starts is cut where the next starts.
pub fn caption_lines_from_timestamped(document: &Value) -> Vec<CaptionLine> {
    let mut lines: Vec<CaptionLine> = document
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|segment| segment.get("type").and_then(Value::as_str) == Some("vocal"))
        .filter_map(|segment| {
            let start = segment.get("start").and_then(Value::as_f64)?;
            let end = segment.get("end").and_then(Value::as_f64)?;
            let text = collapse_spaces(segment.get("text").and_then(Value::as_str).unwrap_or(""));
            (end > start && !text.is_empty()).then_some(CaptionLine { start, end, text })
        })
        .collect();
    lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    for index in 1..lines.len() {
        let next = lines[index].start;
        if lines[index - 1].end > next {
            lines[index - 1].end = next;
        }
    }
    lines.retain(|line| line.end - line.start >= 0.2);
    lines
}

/// The scenes that have lyrics, for a project without word timing: one line for the whole scene.
pub fn caption_lines_from_scenes(session: &Value) -> Vec<CaptionLine> {
    session
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|scene| scene.get("lyric_instrumental").and_then(Value::as_bool) != Some(true))
        .filter_map(|scene| {
            let start = scene.get("start").and_then(Value::as_f64)?;
            let end = scene.get("end").and_then(Value::as_f64)?;
            let text = collapse_spaces(scene.get("lyric_text").and_then(Value::as_str).unwrap_or(""));
            (end > start && !text.is_empty()).then_some(CaptionLine { start, end, text })
        })
        .collect()
}

/// One text clip for each line, in the style, marked as part of the `lyric_captions` group so a second run can replace them. Only lines that
/// overlap `range` (seconds from, to) are used when it is given.
pub fn caption_clips(lines: &[CaptionLine], style: &str, range: Option<(f64, f64)>) -> Vec<Value> {
    let fields = text_style(style);
    let mut clips = Vec::new();
    for line in lines {
        if let Some((from, to)) = range {
            if line.end <= from || line.start >= to {
                continue;
            }
        }
        let mut clip = fields.clone();
        clip.insert("kind".into(), json!("text"));
        clip.insert("id".into(), json!(format!("caption_{}", clips.len() + 1)));
        clip.insert("start".into(), json!(line.start));
        clip.insert("end".into(), json!(line.end));
        clip.insert("text".into(), json!(line.text));
        clip.insert("locked".into(), json!(false));
        clip.insert("group".into(), json!("lyric_captions"));
        clips.push(Value::Object(clip));
    }
    clips
}

/// The switch for all overlays. They are on until it is turned off, since adding one means it should show.
pub fn normalize_overlay_layer_track(source: &Value) -> Obj {
    let mut track = source.as_object().cloned().unwrap_or_default();
    let enabled = flag(track.get("enabled"), true);
    track.insert("enabled".into(), json!(enabled));
    track
}
