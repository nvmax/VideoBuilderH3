//! The overlay pass: film grain, vignette, a whole-timeline LUT and media overlays (particles, dust, light leaks) laid over a finished
//! video. `plan_overlay` is pure: clips in, an ffmpeg filter graph out. `render_overlay` runs it.
//!
//! The graph is one straight chain. Every clip is a **layer** (a media clip, a grain texture, a white picture with a vignette) that
//! is blended onto the chain with the clip's blend mode, opacity and time window:
//!
//! ```text
//! [media or noise or mask] ... [layerN];   [cur][layerN]blend=all_mode=MODE:all_opacity=OPACITY:enable='between(t,start,end)'[vN]
//! ```
//!
//! Text is the one clip that is not a layer: a `drawtext` step in the same chain, with its words read from a file (so quotes, colons and
//! percent signs in a lyric cannot break the graph) and its font copied next to it.
//!
//! A fade is an `eq` on the layer that pulls it to the neutral value of its blend mode (black for screen, mid gray for overlay and
//! grain, white for multiply), with the time as an expression, so nothing is computed per pixel. The chain is straight on purpose:
//! ffmpeg's cost for a graph that splits the picture and joins it again at every clip doubles with each clip (20 clips took 0.7 s a
//! frame), so the layers come from their own sources and only a LUT clip, which has to grade the picture itself, splits it.
//!
//! Scene clips are never touched: the pass reads the stitched video and writes a new file.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use h3_core::overlay_layers::normalize_overlay_layers;
use h3_core::pyval::Obj;
use serde_json::{json, Value};

use crate::fonts::DEFAULT_FONT;
use crate::post::{has_audio, normalize_encode_preset};
use crate::tools::{args, failure_text, run_in, MediaError, Tools};
use crate::video::{probe_format_duration, probe_output_media, probe_stream_fields};

/// LUT clips grade the picture itself, so each one splits and joins the chain. More than this many are left out.
pub const MAX_LUT_CLIPS: usize = 6;

/// What the pass needs to know about the video it works on.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlaySource {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

/// A media clip the command reads as an extra input.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInput {
    pub path: String,
    pub looped: bool,
    pub seek: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub id: String,
    pub reason: String,
}

/// The result of planning: the filter graph, the extra inputs, the LUT names it reads as `lut0.cube`, `lut1.cube` and so on, which clips
/// it applies and which it left out (with why).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverlayPlan {
    pub graph: String,
    pub media: Vec<MediaInput>,
    pub luts: Vec<String>,
    /// The font files the text clips use, read as `font0.ttf`, `font1.ttf` and so on.
    pub fonts: Vec<String>,
    /// The words of each text clip, read as `text0.txt`, `text1.txt` and so on.
    pub texts: Vec<String>,
    pub applied: Vec<String>,
    pub skipped: Vec<Skipped>,
}

impl OverlayPlan {
    pub fn to_json(&self) -> Value {
        json!({
            "graph": self.graph,
            "media": self.media.iter().map(|m| json!({"path": m.path, "looped": m.looped, "seek": m.seek})).collect::<Vec<_>>(),
            "luts": self.luts,
            "fonts": self.fonts,
            "texts": self.texts,
            "applied": self.applied,
            "skipped": self.skipped.iter().map(|s| json!({"id": s.id, "reason": s.reason})).collect::<Vec<_>>(),
        })
    }
}

/// A number as short text: up to four decimals, no trailing zeros.
fn n(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" { "0".into() } else { text.to_string() }
}

fn num(clip: &Obj, key: &str, default: f64) -> f64 {
    clip.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn text<'a>(clip: &'a Obj, key: &str) -> &'a str {
    clip.get(key).and_then(Value::as_str).unwrap_or("")
}

fn flag(clip: &Obj, key: &str, default: bool) -> bool {
    clip.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// `blend` mode names for the blend modes a clip can ask for. `colordodge` and `colorburn` are not ffmpeg modes, `blend_options` writes them out.
fn blend_mode(name: &str) -> &'static str {
    match name {
        "add" => "addition",
        "lighten" => "lighten",
        "darken" => "darken",
        "overlay" => "overlay",
        "softlight" => "softlight",
        "hardlight" => "hardlight",
        "colordodge" => "colordodge",
        "colorburn" => "colorburn",
        "multiply" => "multiply",
        _ => "screen",
    }
}

/// The `blend` options of a mode at an opacity. ffmpeg's own `dodge` and `burn` take the first input (the picture) as the light and the second
/// as the base, the reverse of color dodge and color burn, and an `all_expr` ignores `all_opacity`. So those two are written out, with the
/// opacity inside the expression.
fn blend_options(mode: &str, opacity: f64) -> String {
    let opacity = n(opacity);
    match mode {
        "colordodge" => format!("all_expr='A+(if(eq(A,0),0,if(eq(B,255),255,min(255,A*255/(255-B))))-A)*{opacity}'"),
        "colorburn" => format!("all_expr='A+(if(eq(A,255),255,if(eq(B,0),0,max(0,255-(255-A)*255/B)))-A)*{opacity}'"),
        _ => format!("all_mode={mode}:all_opacity={opacity}"),
    }
}

/// The brightness a layer has to reach to leave the picture unchanged in a blend mode, as a fraction of white.
fn neutral(mode: &str) -> f64 {
    match mode {
        "overlay" | "softlight" | "hardlight" | "grainmerge" => 0.5,
        "multiply" | "darken" | "colorburn" => 1.0,
        _ => 0.0,
    }
}

fn even(value: u32) -> u32 {
    (value / 2 * 2).max(2)
}

/// One `#rrggbb` as the three `colorchannelmixer` gains.
fn tint_gains(tint: &str) -> Option<(f64, f64, f64)> {
    let hex = tint.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|v| f64::from(v) / 255.0);
    Some((channel(0)?, channel(2)?, channel(4)?))
}

/// The scaling of a media clip to the frame, by the clip's `scale` mode.
fn fit(mode: &str, w: u32, h: u32) -> String {
    match mode {
        "contain" => format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black"),
        "stretch" => format!("scale={w}:{h}"),
        _ => format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}"),
    }
}

/// An `eq` that pulls a layer to the neutral value of its blend mode while the clip fades in and out, or `None` when it has no fades.
/// The time is an expression, so ffmpeg evaluates it once per frame and never per pixel.
fn fade_amount(start: f64, end: f64, fade_in: f64, fade_out: f64) -> Option<String> {
    let mut terms = Vec::new();
    if fade_in > 0.0 {
        terms.push(format!("(t-{})/{}", n(start), n(fade_in)));
    }
    if fade_out > 0.0 {
        terms.push(format!("({}-t)/{}", n(end), n(fade_out)));
    }
    let amount = match terms.as_slice() {
        [] => return None,
        [one] => one.clone(),
        [a, b] => format!("min({a},{b})"),
        _ => return None,
    };
    Some(format!("clip({amount},0,1)"))
}

fn fade_eq(start: f64, end: f64, fade_in: f64, fade_out: f64, neutral: f64) -> Option<String> {
    let c = fade_amount(start, end, fade_in, fade_out)?;
    let brightness = if neutral == 0.5 { "0".to_string() } else { format!("{}*(1-{c})", n(neutral - 0.5)) };
    Some(format!("eq=contrast='{c}':saturation='{c}':brightness='{brightness}':eval=frame"))
}

/// The clip's window and ramps, read once.
struct Timing {
    start: f64,
    end: f64,
    fade_in: f64,
    fade_out: f64,
    opacity: f64,
}

impl Timing {
    fn of(clip: &Obj) -> Self {
        let start = num(clip, "start", 0.0);
        Self { start, end: num(clip, "end", start), fade_in: num(clip, "fade_in", 0.0), fade_out: num(clip, "fade_out", 0.0), opacity: num(clip, "opacity", 1.0) }
    }

    fn enable(&self) -> String {
        format!("enable='between(t,{},{})'", n(self.start), n(self.end))
    }

    fn fade(&self, neutral: f64) -> Option<String> {
        fade_eq(self.start, self.end, self.fade_in, self.fade_out, neutral)
    }
}

/// A `#rrggbb` color as the `0xrrggbb` ffmpeg reads.
fn ffmpeg_color(color: &str, fallback: &str) -> String {
    let hex = color.strip_prefix('#').filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())).unwrap_or(fallback);
    format!("0x{}", hex.to_lowercase())
}

/// The `drawtext` step of a text clip. Sizes are for a picture 1080 high and grow or shrink with the video, so a title looks the same at any size.
/// The text sits with its middle at (x, y) as fractions of the picture. Fades and opacity go in `alpha`, `slide_up` moves it up a little while it
/// fades in.
fn text_filter(clip: &Obj, source: &OverlaySource, timing: &Timing, current: &str, font_index: usize, text_index: usize, next: &str) -> String {
    let scale = f64::from(source.height) / 1080.0;
    let mut y = format!("h*{}-text_h/2", n(num(clip, "y", 0.78)));
    if text(clip, "animation") == "slide_up" {
        y.push_str(&format!("+h*0.04*(1-clip((t-{})/{},0,1))", n(timing.start), n(timing.fade_in.max(0.4))));
    }
    let alpha = match fade_amount(timing.start, timing.end, timing.fade_in, timing.fade_out) {
        Some(amount) => format!("{}*{amount}", n(timing.opacity)),
        None => n(timing.opacity),
    };
    format!(
        "[{current}]drawtext=fontfile=font{font_index}.ttf:textfile=text{text_index}.txt:expansion=none:fontsize={}:fontcolor={}:borderw={}:bordercolor={}:shadowx={}:shadowy={}:shadowcolor=0x000000@0.6:x='w*{}-text_w/2':y='{y}':alpha='{alpha}':{}[{next}]",
        n(num(clip, "size", 96.0) * scale),
        ffmpeg_color(text(clip, "color"), "ffffff"),
        n(num(clip, "outline_width", 3.0) * scale),
        ffmpeg_color(text(clip, "outline_color"), "000000"),
        n(num(clip, "shadow", 4.0) * scale),
        n(num(clip, "shadow", 4.0) * scale),
        n(num(clip, "x", 0.5)),
        timing.enable(),
    )
}

/// Lines that end in `[l<k>]`: the grain texture. Fine grain is made at full size, medium and coarse on a smaller picture scaled up.
fn grain_layer(clip: &Obj, source: &OverlaySource, timing: &Timing, k: usize) -> Vec<String> {
    let strength = num(clip, "strength", 28.0);
    let chroma = num(clip, "chroma", 0.0);
    let flags = if flag(clip, "animated", true) { "t+u" } else { "u" };
    let factor = match text(clip, "size") {
        "coarse" => 3,
        "medium" => 2,
        _ => 1,
    };
    let (w, h) = (even(source.width / factor), even(source.height / factor));
    // Noise on a flat gray picture: luma noise around the middle, so `grainmerge` adds it without moving the average. The color planes
    // stay neutral unless the clip asks for color grain.
    let mut noise = format!("color=c=gray:s={w}x{h}:r={},format=yuv420p,noise=c0s={}:c0f={flags}", n(source.fps), n(strength));
    if chroma > 0.0 {
        noise.push_str(&format!(":c1s={0}:c1f={flags}:c2s={0}:c2f={flags}", n(chroma)));
    }
    if factor > 1 {
        noise.push_str(&format!(",scale={}:{}:flags=bicubic", source.width, source.height));
    }
    if let Some(fade) = timing.fade(neutral("grainmerge")) {
        noise.push(',');
        noise.push_str(&fade);
    }
    noise.push_str(&format!(",format=gbrp[l{k}]"));
    vec![noise]
}

/// The vignette: a white picture put through ffmpeg's `vignette`, which makes a mask that is white in the middle and dark at the corners,
/// multiplied onto the video.
fn vignette_layer(clip: &Obj, source: &OverlaySource, timing: &Timing, k: usize) -> Vec<String> {
    let angle = 1.2 * num(clip, "strength", 0.7);
    let mut mask = format!("color=c=white:s={}x{}:r={},format=gbrp,vignette=angle={}:eval=init", source.width, source.height, n(source.fps), n(angle));
    if let Some(fade) = timing.fade(neutral("multiply")) {
        mask.push(',');
        mask.push_str(&fade);
    }
    mask.push_str(&format!("[l{k}]"));
    vec![mask]
}

/// A media clip's own stream, prepared: speed, size, flip, brightness, padded with black so it starts at time zero like the video, the
/// fade, then RGB with the tint.
fn media_layer(clip: &Obj, source: &OverlaySource, timing: &Timing, input: usize, k: usize, mode: &str) -> Vec<String> {
    let mut prep = format!("[{input}:v]setpts=(PTS-STARTPTS)/{},fps={},{}", n(num(clip, "speed", 1.0)), n(source.fps), fit(text(clip, "scale"), source.width, source.height));
    if flag(clip, "flip_h", false) {
        prep.push_str(",hflip");
    }
    let (brightness, contrast) = (num(clip, "brightness", 0.0), num(clip, "contrast", 1.0));
    if brightness != 0.0 || contrast != 1.0 {
        prep.push_str(&format!(",eq=brightness={}:contrast={}", n(brightness), n(contrast)));
    }
    // The stream starts at time zero like the video, so the fade below, which reads the time, sees the same clock as the timeline. The
    // blend's time window shows the clip only from its start.
    if timing.start > 0.0 {
        prep.push_str(&format!(",tpad=start_duration={}:start_mode=add:color=black", n(timing.start)));
    }
    if mode == "normal" {
        // A plain picture over the video: it fades through its own alpha and is laid on with `overlay`.
        prep.push_str(",format=gbrap");
        if let Some((r, g, b)) = tint_gains(text(clip, "tint")) {
            prep.push_str(&format!(",colorchannelmixer=rr={}:gg={}:bb={}", n(r), n(g), n(b)));
        }
        if timing.opacity < 0.9999 {
            prep.push_str(&format!(",colorchannelmixer=aa={}", n(timing.opacity)));
        }
        if timing.fade_in > 0.0 {
            prep.push_str(&format!(",fade=t=in:st={}:d={}:alpha=1", n(timing.start), n(timing.fade_in)));
        }
        if timing.fade_out > 0.0 {
            prep.push_str(&format!(",fade=t=out:st={}:d={}:alpha=1", n(timing.end - timing.fade_out), n(timing.fade_out)));
        }
    } else {
        if let Some(fade) = timing.fade(neutral(mode)) {
            prep.push(',');
            prep.push_str(&fade);
        }
        prep.push_str(",format=gbrp");
        if let Some((r, g, b)) = tint_gains(text(clip, "tint")) {
            prep.push_str(&format!(",colorchannelmixer=rr={}:gg={}:bb={}", n(r), n(g), n(b)));
        }
    }
    prep.push_str(&format!("[l{k}]"));
    vec![prep]
}

/// Plan the pass. `clips` are normalized overlays in track order (later ones draw on top). `file_exists` says whether a media path is
/// a file, `lut_exists` whether a LUT name resolves and `font_file` turns a font name into its file name (`None` when there is no such font, and
/// the text then uses the default font).
pub fn plan_overlay(clips: &[Obj], source: &OverlaySource, file_exists: &dyn Fn(&str) -> bool, lut_exists: &dyn Fn(&str) -> bool, font_file: &dyn Fn(&str) -> Option<String>) -> OverlayPlan {
    let mut plan = OverlayPlan::default();
    let mut lines = vec!["[0:v]format=gbrp[v0]".to_string()];
    let mut current = "v0".to_string();
    let skip = |plan: &mut OverlayPlan, id: &str, reason: &str| plan.skipped.push(Skipped { id: id.to_string(), reason: reason.to_string() });
    for clip in clips {
        let id = text(clip, "id").to_string();
        if !flag(clip, "enabled", true) {
            continue;
        }
        let timing = Timing::of(clip);
        if timing.start >= source.duration {
            skip(&mut plan, &id, "starts after the end of the video");
            continue;
        }
        let k = plan.applied.len() + 1;
        let next = format!("v{k}");
        match text(clip, "kind") {
            "grain" => {
                lines.extend(grain_layer(clip, source, &timing, k));
                lines.push(format!("[{current}][l{k}]blend=all_mode=grainmerge:all_opacity={}:{}[{next}]", n(timing.opacity), timing.enable()));
            }
            "vignette" => {
                lines.extend(vignette_layer(clip, source, &timing, k));
                lines.push(format!("[{current}][l{k}]blend=all_mode=multiply:all_opacity={}:{}[{next}]", n(timing.opacity), timing.enable()));
            }
            "media" => {
                let path = text(clip, "path");
                if path.is_empty() {
                    skip(&mut plan, &id, "no clip file was chosen");
                    continue;
                }
                if !file_exists(path) {
                    skip(&mut plan, &id, "the clip file was not found");
                    continue;
                }
                let input = plan.media.len() + 1;
                plan.media.push(MediaInput { path: path.to_string(), looped: flag(clip, "loop", true), seek: num(clip, "source_start", 0.0) });
                let mode = text(clip, "blend_mode");
                lines.extend(media_layer(clip, source, &timing, input, k, if mode == "normal" { "normal" } else { blend_mode(mode) }));
                if mode == "normal" {
                    lines.push(format!("[{current}][l{k}]overlay=format=gbrp:{}[{next}]", timing.enable()));
                } else {
                    lines.push(format!("[{current}][l{k}]blend={}:{}[{next}]", blend_options(blend_mode(mode), timing.opacity), timing.enable()));
                }
            }
            "lut" => {
                let name = text(clip, "lut");
                if name.is_empty() || !lut_exists(name) {
                    skip(&mut plan, &id, "the LUT file was not found");
                    continue;
                }
                if plan.luts.len() >= MAX_LUT_CLIPS {
                    skip(&mut plan, &id, &format!("only {MAX_LUT_CLIPS} LUT clips can be applied in one render"));
                    continue;
                }
                plan.luts.push(name.to_string());
                // The one clip that splits the picture: it grades a copy and lays it back with the clip's alpha, ramps and window.
                let alpha = timing.opacity * num(clip, "strength", 1.0);
                lines.push(format!("[{current}]split=2[a{k}][b{k}]"));
                lines.push(format!("[b{k}]lut3d=file=lut{}.cube:interp=trilinear[eff{k}]", plan.luts.len() - 1));
                let mut ramp = format!("[eff{k}]format=gbrap");
                if alpha < 0.9999 {
                    ramp.push_str(&format!(",colorchannelmixer=aa={}", n(alpha)));
                }
                if timing.fade_in > 0.0 {
                    ramp.push_str(&format!(",fade=t=in:st={}:d={}:alpha=1", n(timing.start), n(timing.fade_in)));
                }
                if timing.fade_out > 0.0 {
                    ramp.push_str(&format!(",fade=t=out:st={}:d={}:alpha=1", n(timing.end - timing.fade_out), n(timing.fade_out)));
                }
                ramp.push_str(&format!("[al{k}]"));
                lines.push(ramp);
                lines.push(format!("[a{k}][al{k}]overlay=format=gbrp:{}[{next}]", timing.enable()));
            }
            "text" => {
                let words = text(clip, "text");
                if words.trim().is_empty() {
                    skip(&mut plan, &id, "the text is empty");
                    continue;
                }
                let Some(file) = font_file(text(clip, "font")).or_else(|| font_file(DEFAULT_FONT)) else {
                    skip(&mut plan, &id, "no font is available for the text");
                    continue;
                };
                let font_index = plan.fonts.iter().position(|f| f == &file).unwrap_or_else(|| {
                    plan.fonts.push(file);
                    plan.fonts.len() - 1
                });
                plan.texts.push(words.to_string());
                lines.push(text_filter(clip, source, &timing, &current, font_index, plan.texts.len() - 1, &next));
            }
            other => {
                skip(&mut plan, &id, &format!("{other} is not a kind this version can draw"));
                continue;
            }
        }
        current = next;
        plan.applied.push(id);
    }
    lines.push(format!("[{current}]format=yuv420p[v]"));
    plan.graph = lines.join(";\n");
    plan
}

/// How the graph reaches ffmpeg. A short graph goes in the command line. A long one (Windows allows about 32,000 characters) goes in a
/// file, and the option that reads a file changed with ffmpeg 7: `-/filter_complex <file>` is new, `-filter_complex_script <file>` is
/// gone from ffmpeg 8. So a file is tried one way and then the other.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphArg {
    Inline,
    Slash(String),
    Script(String),
}

/// A graph longer than this many characters is passed as a file.
pub const INLINE_GRAPH_LIMIT: usize = 16_000;

/// The extra inputs: each media clip, looped and started where it should.
fn media_args(plan: &OverlayPlan) -> Vec<String> {
    let mut a = Vec::new();
    for media in &plan.media {
        if media.looped {
            a.extend(args(&["-stream_loop", "-1"]));
        }
        if media.seek > 0.0 {
            a.extend(args(&["-ss", &n(media.seek)]));
        }
        a.extend(args(&["-i", &media.path]));
    }
    a
}

fn graph_args(plan: &OverlayPlan, graph: &GraphArg) -> Vec<String> {
    match graph {
        GraphArg::Inline => args(&["-filter_complex", &plan.graph.replace('\n', "")]),
        GraphArg::Slash(file) => args(&["-/filter_complex", file]),
        GraphArg::Script(file) => args(&["-filter_complex_script", file]),
    }
}

/// The ffmpeg arguments: the video, the media clips, the graph, and an H.264 encode that keeps the video's audio as it is.
#[allow(clippy::too_many_arguments)]
pub fn ffmpeg_args(plan: &OverlayPlan, input: &str, output: &str, graph: &GraphArg, source: &OverlaySource, crf: i64, preset: &str, keep_audio: bool) -> Vec<String> {
    let mut a = args(&["-y", "-loglevel", "error", "-i", input]);
    a.extend(media_args(plan));
    a.extend(graph_args(plan, graph));
    a.extend(args(&["-map", "[v]"]));
    if keep_audio {
        a.extend(args(&["-map", "0:a:0", "-c:a", "copy"]));
    } else {
        a.push("-an".into());
    }
    a.extend(args(&["-c:v", "libx264", "-preset", preset, "-crf", &crf.to_string(), "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-t", &n(source.duration), output]));
    a
}

/// The arguments that make one picture: the video from `seek` seconds in, the media clips, the graph, one PNG.
pub fn ffmpeg_frame_args(plan: &OverlayPlan, input: &str, seek: f64, output: &str, graph: &GraphArg) -> Vec<String> {
    let mut a = args(&["-y", "-loglevel", "error"]);
    if seek > 0.0 {
        a.extend(args(&["-ss", &n(seek)]));
    }
    a.extend(args(&["-i", input]));
    a.extend(media_args(plan));
    a.extend(graph_args(plan, graph));
    a.extend(args(&["-map", "[v]", "-frames:v", "1", "-pix_fmt", "rgb24", output]));
    a
}

/// A looped media clip that starts after its file has already played through (a render that begins part way into the timeline) starts where
/// the loop would be: the first pass runs to the end of the file and every pass after it starts again at the start of the file.
fn wrap_media_starts(tools: &Tools, clips: &mut [Obj]) {
    for clip in clips.iter_mut() {
        if text(clip, "kind") != "media" || !flag(clip, "loop", true) {
            continue;
        }
        let start = num(clip, "source_start", 0.0);
        if start <= 0.0 {
            continue;
        }
        let duration = probe_format_duration(tools, Path::new(text(clip, "path")));
        if duration > 0.0 && start >= duration {
            clip.insert("source_start".into(), json!(((start - duration) % duration * 1000.0).round() / 1000.0));
        }
    }
}

/// The file name of the font a name resolves to.
fn font_name(resolve_font: &dyn Fn(&str) -> Option<PathBuf>, name: &str) -> Option<String> {
    resolve_font(name).and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// `r_frame_rate` text such as `24/1` as a number.
fn parse_rate(text: &str) -> Option<f64> {
    let (top, bottom) = text.split_once('/').unwrap_or((text, "1"));
    let (top, bottom) = (top.trim().parse::<f64>().ok()?, bottom.trim().parse::<f64>().ok()?);
    (bottom != 0.0 && top > 0.0).then(|| top / bottom)
}

/// Read what the pass needs from the video.
pub fn probe_source(tools: &Tools, video: &Path) -> Result<OverlaySource, MediaError> {
    let media = probe_output_media(tools, video);
    let rate = probe_stream_fields(tools, video, "v:0", &["r_frame_rate"], false)?.get("r_frame_rate").and_then(|r| parse_rate(r));
    let duration = probe_format_duration(tools, video);
    if media.width == 0 || media.height == 0 || duration <= 0.0 {
        return Err(MediaError::Failed(format!("Could not read the size and length of {}", video.display())));
    }
    Ok(OverlaySource { width: media.width, height: media.height, fps: rate.unwrap_or(24.0), duration })
}

/// The files a graph reads by plain names: its LUTs, fonts and texts, in `dir`.
fn write_graph_files(dir: &Path, plan: &OverlayPlan, resolve_lut: &dyn Fn(&str) -> Option<PathBuf>, resolve_font: &dyn Fn(&str) -> Option<PathBuf>) -> Result<(), MediaError> {
    for (index, name) in plan.luts.iter().enumerate() {
        let cube = resolve_lut(name).ok_or_else(|| MediaError::Failed(format!("LUT file was not found: {name}")))?;
        fs::copy(&cube, dir.join(format!("lut{index}.cube")))?;
    }
    for (index, name) in plan.fonts.iter().enumerate() {
        let font = resolve_font(name).ok_or_else(|| MediaError::Failed(format!("Font file was not found: {name}")))?;
        fs::copy(&font, dir.join(format!("font{index}.ttf")))?;
    }
    for (index, words) in plan.texts.iter().enumerate() {
        fs::write(dir.join(format!("text{index}.txt")), words)?;
    }
    fs::write(dir.join("graph.txt"), &plan.graph)?;
    Ok(())
}

/// Apply the overlays to `input` and write `output`. Returns `changed: false` (and writes nothing) when no clip could be applied.
/// `resolve_lut` turns a LUT name into its `.cube` file and `resolve_font` a font name into its font file.
#[allow(clippy::too_many_arguments)]
pub fn render_overlay(tools: &Tools, input: &Path, output: &Path, clips: &Value, resolve_lut: &dyn Fn(&str) -> Option<PathBuf>, resolve_font: &dyn Fn(&str) -> Option<PathBuf>, crf: i64, preset: &str) -> Result<Value, MediaError> {
    if !input.is_file() {
        return Err(MediaError::Failed(format!("Input video was not found: {}", input.display())));
    }
    let mut clips = normalize_overlay_layers(clips);
    wrap_media_starts(tools, &mut clips);
    let source = probe_source(tools, input)?;
    let plan = plan_overlay(&clips, &source, &|path| Path::new(path).is_file(), &|name| resolve_lut(name).is_some(), &|name| font_name(resolve_font, name));
    let skipped: Vec<Value> = plan.skipped.iter().map(|s| json!({"id": s.id, "reason": s.reason})).collect();
    if plan.applied.is_empty() {
        return Ok(json!({"changed": false, "applied": [], "skipped": skipped, "output": input.display().to_string()}));
    }
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let work = tempfile::Builder::new().prefix("vrgdg_overlay_").tempdir()?;
    write_graph_files(work.path(), &plan, resolve_lut, resolve_font)?;
    let keep_audio = has_audio(tools, input)?;
    let temp_output = parent.join(format!(".vrgdg_overlay_{}_{}.mp4", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())));
    let (input_text, output_text, preset) = (input.display().to_string(), temp_output.display().to_string(), normalize_encode_preset(preset));
    let ways = if plan.graph.len() <= INLINE_GRAPH_LIMIT {
        vec![GraphArg::Inline]
    } else {
        vec![GraphArg::Slash("graph.txt".into()), GraphArg::Script("graph.txt".into())]
    };
    let started = Instant::now();
    let mut failure = String::new();
    let mut done = false;
    for way in &ways {
        let command = ffmpeg_args(&plan, &input_text, &output_text, way, &source, crf.clamp(16, 35), &preset, keep_audio);
        let out = run_in(&tools.ffmpeg, &command, work.path())?;
        if out.status.success() {
            done = true;
            break;
        }
        failure = failure_text(&out, "ffmpeg failed.");
        // Only an ffmpeg that does not know the option is worth another try with the other one.
        if !failure.contains("Unrecognized option") && !failure.contains("Option not found") {
            break;
        }
    }
    if !done {
        let _ = fs::remove_file(&temp_output);
        return Err(MediaError::Failed(format!("ffmpeg could not apply the overlays: {failure}")));
    }
    fs::rename(&temp_output, output).or_else(|_| fs::copy(&temp_output, output).and_then(|_| fs::remove_file(&temp_output)))?;
    Ok(json!({
        "changed": true,
        "applied": plan.applied,
        "skipped": skipped,
        "output": output.display().to_string(),
        "width": source.width,
        "height": source.height,
        "duration": source.duration,
        "audio_kept": keep_audio,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
    }))
}

/// One frame of the video with the overlays on it, as PNG bytes: the exact picture the render makes at that moment, for a look before a long render.
/// `seek` is how far into `input` the frame is. `clips` are already moved onto the video's own clock (see `shift_overlay_layers`). Returns the plan's
/// answer (which clips it applied and which it left out) and the picture.
pub fn render_overlay_frame(tools: &Tools, input: &Path, seek: f64, clips: &Value, resolve_lut: &dyn Fn(&str) -> Option<PathBuf>, resolve_font: &dyn Fn(&str) -> Option<PathBuf>) -> Result<(Value, Vec<u8>), MediaError> {
    if !input.is_file() {
        return Err(MediaError::Failed(format!("Input video was not found: {}", input.display())));
    }
    let mut clips = normalize_overlay_layers(clips);
    wrap_media_starts(tools, &mut clips);
    let mut source = probe_source(tools, input)?;
    let seek = seek.max(0.0).min((source.duration - 0.05).max(0.0));
    // The plan leaves out clips that start after the video ends, and the picture is taken `seek` seconds in.
    source.duration = (source.duration - seek).max(0.05);
    let plan = plan_overlay(&clips, &source, &|path| Path::new(path).is_file(), &|name| resolve_lut(name).is_some(), &|name| font_name(resolve_font, name));
    let work = tempfile::Builder::new().prefix("vrgdg_overlayframe_").tempdir()?;
    write_graph_files(work.path(), &plan, resolve_lut, resolve_font)?;
    let ways = if plan.graph.len() <= INLINE_GRAPH_LIMIT { vec![GraphArg::Inline] } else { vec![GraphArg::Slash("graph.txt".into()), GraphArg::Script("graph.txt".into())] };
    let picture = work.path().join("frame.png");
    let started = Instant::now();
    let mut failure = String::new();
    let mut done = false;
    for way in &ways {
        let command = ffmpeg_frame_args(&plan, &input.display().to_string(), seek, &picture.display().to_string(), way);
        let out = run_in(&tools.ffmpeg, &command, work.path())?;
        if out.status.success() && picture.is_file() {
            done = true;
            break;
        }
        failure = failure_text(&out, "ffmpeg made no picture.");
        if !failure.contains("Unrecognized option") && !failure.contains("Option not found") {
            break;
        }
    }
    if !done {
        return Err(MediaError::Failed(format!("ffmpeg could not make the picture: {failure}")));
    }
    let skipped: Vec<Value> = plan.skipped.iter().map(|s| json!({"id": s.id, "reason": s.reason})).collect();
    let bytes = fs::read(&picture)?;
    Ok((json!({"applied": plan.applied, "skipped": skipped, "width": source.width, "height": source.height, "seek": seek, "elapsed_seconds": started.elapsed().as_secs_f64()}), bytes))
}
