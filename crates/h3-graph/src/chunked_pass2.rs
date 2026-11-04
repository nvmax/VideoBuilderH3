//! Chunked second pass for the MiniMax H3 2 Pass graph. Port of `runner/minimax_chunked_pass2.py`.
//!
//! A 2 Pass render normally upscales the whole first-pass clip and refines it in one sampler call, which needs memory for
//! every frame of the scene. When the clip is longer than the "Pass 2 chunking" threshold, [`apply_chunked_pass2`] rewires
//! the built graph so the same upscale and refinement run in overlapping windows through `VRGDG_MiniMaxH3ChunkedPass2`
//! (a node of the pack). The first pass, the audio, the references, the latent saves and the decode are untouched. A clip
//! at or under the threshold keeps the normal graph.
//!
//! The windows restart their clock at 0, so a prompt with whole-scene shot times and lyric cues would mislead them. The
//! second pass therefore reads the same prompt with its lyric text and times removed ([`strip_prompt_timing`]).

use fancy_regex::{Captures, Regex};
use h3_core::chunk_plan::{
    chunking_decision, CHUNK_SECONDS, OVERLAP_MODE, OVERLAP_SECONDS, TAIL_TRIM_SECONDS, UPSCALE_MODE,
};
use h3_core::pyval::{fmt_g, Obj};
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::api::{as_link, link};
use crate::env::GraphEnv;

pub const CHUNK_CLASS: &str = "VRGDG_MiniMaxH3ChunkedPass2";
const SUPPORTED_ENCODERS: [&str; 2] = ["MiniMaxH3ReferenceToVideo", "MiniMaxH3RefModTextEncode"];

/// The built graph does not have the shape this rewrite expects.
struct Unsupported(String);

fn unsupported<T>(message: impl Into<String>) -> Result<T, Unsupported> {
    Err(Unsupported(message.into()))
}

struct Patterns {
    cue_with_text: Regex,
    dialogue: Regex,
    cue_time: Regex,
    shot_time: Regex,
    shot_paragraph: Regex,
    section_line: Regex,
    paragraph_break: Regex,
    space_before_comma: Regex,
    double_space: Regex,
}

fn patterns() -> &'static Patterns {
    static CACHE: OnceLock<Patterns> = OnceLock::new();
    CACHE.get_or_init(|| {
        let make = |pattern: &str| Regex::new(pattern).expect("the chunked pass 2 patterns are valid");
        Patterns {
            cue_with_text: make(r"(?s)[ \t]*from \d+(?:\.\d+)? s to \d+(?:\.\d+)? s,?[ \t]*<d>.*?</d>"),
            dialogue: make(r"(?s)[ \t]*<d>.*?</d>"),
            cue_time: make(r"[ \t]*from \d+(?:\.\d+)? s to \d+(?:\.\d+)? s"),
            shot_time: make(r"(?m)^(\[Shot \d+\][ \t]*)At \d\d:\d\d(?:\.\d+)?,[ \t]*(\S)"),
            shot_paragraph: make(r"(?s)^\[Shot (\d+)\][ \t]*(.*)$"),
            section_line: make(r"(?m)^[a-z_]{4,}:[ \t]*$"),
            paragraph_break: make(r"\n[ \t]*\n"),
            space_before_comma: make(r"[ \t]+,"),
            double_space: make(r"(?<=\S)[ \t]{2,}"),
        }
    })
}

/// Drop a `[Shot n]` paragraph that repeats the one before it word for word, and renumber what is left.
///
/// Shots in one unbroken take often differ only by their lyric and start time. Once those are removed they read the same,
/// and a window would be told the same shot six times. Only directly repeated shots go. A shot that returns after a
/// different one stays, and a prompt with no repeats comes back exactly as it was.
fn merge_repeated_shots(text: &str) -> String {
    let p = patterns();
    // Python's `re.split` with a capture group: the paragraphs and the break between each pair of them.
    let mut paragraphs: Vec<&str> = Vec::new();
    let mut breaks: Vec<&str> = Vec::new();
    let mut last = 0;
    for found in p.paragraph_break.find_iter(text).flatten() {
        paragraphs.push(&text[last..found.start()]);
        breaks.push(found.as_str());
        last = found.end();
    }
    paragraphs.push(&text[last..]);

    let mut kept = String::new();
    let mut previous: Option<String> = None;
    let mut number = 0;
    let mut dropped = 0;
    for (position, paragraph) in paragraphs.iter().enumerate() {
        let separator = if position > 0 { breaks[position - 1] } else { "" };
        let shot = p.shot_paragraph.captures(paragraph).ok().flatten();
        let is_shot = shot.is_some() && !p.section_line.is_match(paragraph).unwrap_or(false);
        let mut rebuilt: Option<String> = None;
        if let (true, Some(shot)) = (is_shot, shot) {
            let body = shot.get(2).map_or("", |m| m.as_str()).trim().to_string();
            if previous.as_deref() == Some(body.as_str()) {
                dropped += 1;
                continue;
            }
            number += 1;
            if dropped > 0 {
                rebuilt = Some(format!("[Shot {number}] {body}"));
            }
            previous = Some(body);
        } else {
            previous = None;
        }
        kept.push_str(separator);
        kept.push_str(rebuilt.as_deref().unwrap_or(paragraph));
    }
    if dropped == 0 {
        return text.to_string();
    }
    let trimmed = kept.trim_end();
    format!("{trimmed}{}", &text[text.trim_end().len()..])
}

/// The prompt with its lyric cues, dialogue text and shot start times removed.
///
/// `sings in sync with the vocals in <Audio 1> from 1.0 s to 4.4 s, <d>[tag] words</d>, delivered softly` becomes
/// `sings in sync with the vocals in <Audio 1>, delivered softly`. A `[Shot 2] At 00:04.600, continuing ...` becomes
/// `[Shot 2] Continuing ...`, and shots that then repeat the one before them are merged into it. Everything else,
/// including the look, the references and the shot descriptions, is kept.
pub fn strip_prompt_timing(text: &str) -> String {
    let p = patterns();
    let result = p.cue_with_text.replace_all(text, "");
    let result = p.dialogue.replace_all(&result, "");
    let result = p.cue_time.replace_all(&result, "");
    let result = p.shot_time.replace_all(&result, |caps: &Captures| {
        format!("{}{}", &caps[1], caps[2].to_uppercase())
    });
    let result = p.space_before_comma.replace_all(&result, ",");
    let result = p.double_space.replace_all(&result, " ");
    merge_repeated_shots(&result)
}

fn source(prompt: &Obj, reference: Option<&Value>, class: &str, what: &str) -> Result<String, Unsupported> {
    let id = reference.and_then(as_link).map(|(id, _)| id);
    match id.as_deref().and_then(|id| prompt.get(id)) {
        Some(node) if node.get("class_type").and_then(Value::as_str) == Some(class) => Ok(id.unwrap_or_default()),
        _ => unsupported(format!("{what} is not a {class} node.")),
    }
}

fn new_id(prompt: &Obj, base: u32) -> String {
    let mut number = base;
    while prompt.contains_key(&number.to_string()) {
        number += 1;
    }
    number.to_string()
}

fn node_inputs<'a>(prompt: &'a Obj, id: &str) -> Option<&'a Obj> {
    prompt.get(id).and_then(|node| node.get("inputs")).and_then(Value::as_object)
}

/// A two item list whose first item is a node id: the shape of a link that points at a missing node.
fn dangling(prompt: &Obj) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (key, node) in prompt {
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else { continue };
        for (name, value) in inputs {
            if let Some([Value::String(id), _]) = value.as_array().map(Vec::as_slice) {
                if !prompt.contains_key(id) {
                    found.push((key.clone(), name.clone()));
                }
            }
        }
    }
    found
}

/// The nodes of the whole-clip second pass.
struct SecondPass {
    sampler: String,
    apply: Option<String>,
    replace: String,
    upscale: String,
    separate: String,
    guider: String,
    noise: String,
    encoder: String,
    text: String,
}

/// The nodes of the whole-clip second pass: separate, upscale, replace, optional continuation head, sampler.
fn find_second_pass(prompt: &Obj) -> Result<SecondPass, Unsupported> {
    let class_of = |node: &Value| node.get("class_type").and_then(Value::as_str).unwrap_or("").to_string();
    let replaces: Vec<&String> = prompt.iter().filter(|(_, n)| class_of(n) == "VRGDG_MiniMaxH3ReplaceUpscaledVideoLatent").map(|(k, _)| k).collect();
    if replaces.len() != 1 {
        return unsupported("the graph has no single learned-upscale replace node.");
    }
    let replace_id = replaces[0].clone();
    let mut found: Vec<(String, Option<String>)> = Vec::new();
    for (key, node) in prompt {
        if class_of(node) != "SamplerCustomAdvanced" {
            continue;
        }
        let mut latent = node_inputs(prompt, key).and_then(|i| i.get("latent_image")).and_then(as_link);
        let mut apply_id = None;
        if let Some((id, _)) = &latent {
            if prompt.get(id).map(class_of).as_deref() == Some("VRGDG_MiniMaxH3ApplyMaskedContinuation") {
                apply_id = Some(id.clone());
                latent = node_inputs(prompt, id).and_then(|i| i.get("latent")).and_then(as_link);
            }
        }
        if latent.is_some_and(|(id, _)| id == replace_id) {
            found.push((key.clone(), apply_id));
        }
    }
    if found.len() != 1 {
        return unsupported("the graph has no single second-pass sampler.");
    }
    let (sampler_id, apply_id) = found.remove(0);
    let sampler = node_inputs(prompt, &sampler_id).cloned().unwrap_or_default();
    let replace = node_inputs(prompt, &replace_id).cloned().unwrap_or_default();
    let upscale_id = source(prompt, replace.get("upscaled_video_latent"), "VRGDG_MiniMaxH3LearnedLatentUpscale", "the upscaled latent")?;
    let upscale_input = node_inputs(prompt, &upscale_id).and_then(|i| i.get("latent"));
    let separate_id = source(prompt, upscale_input, "MiniMaxH3AVLatentSeparateT8", "the upscaler input")?;
    let first_pass = node_inputs(prompt, &separate_id).and_then(|i| i.get("av_latent"));
    if replace.get("original_av_latent") != first_pass {
        return unsupported("the upscale and the replace node read different first-pass latents.");
    }
    let guider_id = source(prompt, sampler.get("guider"), "BasicGuider", "the second-pass guider")?;
    let noise_id = source(prompt, sampler.get("noise"), "RandomNoise", "the second-pass noise")?;
    let encoder_link = node_inputs(prompt, &guider_id).and_then(|i| i.get("conditioning")).and_then(as_link);
    let encoder = encoder_link.as_ref().and_then(|(id, _)| prompt.get(id));
    let supported = encoder.map(class_of).is_some_and(|class| SUPPORTED_ENCODERS.contains(&class.as_str()));
    let (Some((encoder_id, _)), true) = (encoder_link.clone(), supported) else {
        let class = encoder.map(class_of).unwrap_or_else(|| "an unknown node".to_string());
        return unsupported(format!(
            "the second pass is conditioned by {class}; only Reference to Video and RefMod are chunked."
        ));
    };
    let text_input = node_inputs(prompt, &encoder_id).and_then(|i| i.get("prompt"));
    let text_id = source(prompt, text_input, "PrimitiveStringMultiline", "the prompt")?;
    if !node_inputs(prompt, &text_id).and_then(|i| i.get("value")).is_some_and(Value::is_string) {
        return unsupported("the prompt text is not a plain string.");
    }
    Ok(SecondPass {
        sampler: sampler_id,
        apply: apply_id,
        replace: replace_id,
        upscale: upscale_id,
        separate: separate_id,
        guider: guider_id,
        noise: noise_id,
        encoder: encoder_id,
        text: text_id,
    })
}

/// An input the rewrite copies from an old node, or the reason the graph is not the expected one.
fn required(inputs: &Obj, name: &str, owner: &str) -> Result<Value, Unsupported> {
    match inputs.get(name) {
        Some(value) => Ok(value.clone()),
        None => unsupported(format!("the {owner} has no {name}.")),
    }
}

/// Replace the whole-clip second pass in `prompt` with the chunk node. Fails on a foreign graph.
fn rewire(prompt: &mut Obj, rendered_seconds: f64) -> Result<Obj, Unsupported> {
    let found = find_second_pass(prompt)?;
    let sampler = node_inputs(prompt, &found.sampler).cloned().unwrap_or_default();
    let upscale = node_inputs(prompt, &found.upscale).cloned().unwrap_or_default();
    let guider = node_inputs(prompt, &found.guider).cloned().unwrap_or_default();
    let first_pass = required(&node_inputs(prompt, &found.separate).cloned().unwrap_or_default(), "av_latent", "first-pass separate node")?;
    let seed = match node_inputs(prompt, &found.noise).and_then(|i| i.get("noise_seed")) {
        Some(Value::Number(n)) if n.is_u64() || n.is_i64() => Value::Number(n.clone()),
        Some(Value::Number(n)) if n.as_f64().is_some_and(|f| f.fract() == 0.0) => json!(n.as_f64().unwrap_or(0.0) as i64),
        _ => return unsupported("the second-pass seed is not a number."),
    };

    let text_id = new_id(prompt, 9500);
    let old_text = node_inputs(prompt, &found.text).and_then(|i| i.get("value")).and_then(Value::as_str).unwrap_or("");
    prompt.insert(
        text_id.clone(),
        json!({
            "class_type": "PrimitiveStringMultiline",
            "inputs": {"value": strip_prompt_timing(old_text)},
            "_meta": {"title": "Pass 2 prompt (no lyric text, no times)"},
        }),
    );
    let encode_id = new_id(prompt, 9501);
    let mut encoder = prompt.get(&found.encoder).cloned().unwrap_or(Value::Null);
    if let Some(inputs) = encoder.get_mut("inputs").and_then(Value::as_object_mut) {
        inputs.insert("prompt".into(), link(&text_id, 0));
    }
    encoder["_meta"] = json!({"title": "Pass 2 conditioning (windows)"});
    prompt.insert(encode_id.clone(), encoder);

    let chunk_id = new_id(prompt, 9502);
    let mut inputs = Obj::new();
    inputs.insert("latent".into(), first_pass);
    inputs.insert("model".into(), required(&guider, "model", "second-pass guider")?);
    inputs.insert("conditioning".into(), link(&encode_id, 0));
    inputs.insert("upscale_model".into(), required(&upscale, "upscale_model", "learned upscale node")?);
    inputs.insert("sampler".into(), required(&sampler, "sampler", "second-pass sampler")?);
    inputs.insert("sigmas".into(), required(&sampler, "sigmas", "second-pass sampler")?);
    inputs.insert("scale".into(), required(&upscale, "scale", "learned upscale node")?);
    inputs.insert("chunk_seconds".into(), json!(CHUNK_SECONDS));
    inputs.insert("overlap_seconds".into(), json!(OVERLAP_SECONDS));
    inputs.insert("overlap_mode".into(), json!(OVERLAP_MODE));
    inputs.insert("seed".into(), seed);
    inputs.insert("tail_trim_seconds".into(), json!(TAIL_TRIM_SECONDS));
    inputs.insert("max_window_tokens".into(), json!(0));
    inputs.insert("upscale".into(), json!(UPSCALE_MODE));
    let head_locked = found.apply.is_some();
    if let Some(apply_id) = &found.apply {
        let apply = node_inputs(prompt, apply_id).cloned().unwrap_or_default();
        inputs.insert("head_latent".into(), required(&apply, "context_latent", "masked continuation node")?);
        if let Some(vae) = apply.get("vae").filter(|v| !v.is_null()) {
            inputs.insert("head_vae".into(), vae.clone());
        }
    }
    prompt.insert(
        chunk_id.clone(),
        json!({
            "class_type": CHUNK_CLASS,
            "inputs": inputs,
            "_meta": {"title": format!("Pass 2 in windows ({} s clip)", fmt_g(rendered_seconds))},
        }),
    );

    for node in prompt.values_mut() {
        let Some(map) = node.get_mut("inputs").and_then(Value::as_object_mut) else { continue };
        for value in map.values_mut() {
            if as_link(value).is_some_and(|(id, _)| id == found.sampler) {
                *value = link(&chunk_id, 0);
            }
        }
    }
    let mut removed = vec![found.sampler.clone(), found.guider.clone(), found.noise.clone(), found.separate.clone(), found.upscale.clone(), found.replace.clone()];
    removed.extend(found.apply.clone());
    for id in &removed {
        prompt.shift_remove(id);
    }
    let broken = dangling(prompt);
    if !broken.is_empty() {
        let first: Vec<String> = broken.iter().take(3).map(|(node, input)| format!("('{node}', '{input}')")).collect();
        return unsupported(format!("another node still reads the second pass it replaces: [{}]", first.join(", ")));
    }
    let mut summary = Obj::new();
    summary.insert("chunk_node_id".into(), json!(chunk_id));
    summary.insert("encode_node_id".into(), json!(encode_id));
    summary.insert("prompt_node_id".into(), json!(text_id));
    summary.insert("removed_nodes".into(), json!(removed));
    summary.insert("head_locked".into(), json!(head_locked));
    Ok(summary)
}

/// Switch the second pass of a built 2 Pass graph to windows when the clip is long enough.
///
/// Returns a summary with `enabled`. A clip at or under the threshold, a turned-off setting, a ComfyUI without the chunk
/// node, or a graph this rewrite does not recognise leaves `prompt` exactly as it was and says why in `reason`.
pub fn apply_chunked_pass2(env: &dyn GraphEnv, prompt: &mut Obj, payload: &Obj, h3_frame_count: Option<i64>) -> Obj {
    let mut decision = chunking_decision(payload.get("pass2_chunking"), payload.get("pass2_chunking_min_seconds"), h3_frame_count);
    if decision.get("enabled") != Some(&json!(true)) {
        return decision;
    }
    if env.node_info(CHUNK_CLASS).is_none() {
        decision.insert("enabled".into(), json!(false));
        decision.insert(
            "reason".into(),
            json!("Pass 2 runs in one piece because ComfyUI does not have the VRGDG H3 Chunked Pass 2 node. Update the comfyui-vrgamedevgirl pack and restart ComfyUI."),
        );
        return decision;
    }
    let rendered = decision.get("rendered_seconds").and_then(Value::as_f64).unwrap_or(0.0);
    let mut work = prompt.clone();
    match rewire(&mut work, rendered) {
        Ok(summary) => decision.extend(summary),
        Err(Unsupported(why)) => {
            decision.insert("enabled".into(), json!(false));
            decision.insert("reason".into(), json!(format!("Pass 2 runs in one piece because {why}")));
            return decision;
        }
    }
    *prompt = work;
    decision.insert("chunk_seconds".into(), json!(CHUNK_SECONDS));
    decision.insert("overlap_seconds".into(), json!(OVERLAP_SECONDS));
    decision.insert("overlap_mode".into(), json!(OVERLAP_MODE));
    decision.insert("tail_trim_seconds".into(), json!(TAIL_TRIM_SECONDS));
    decision.insert("upscale".into(), json!(UPSCALE_MODE));
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_dialogue_and_times_are_removed() {
        let text = "[Shot 1] At 00:01.000, a singer sings in sync with the vocals in <Audio 1> from 1.0 s to 4.4 s, <d>[soft] words here</d>, delivered softly";
        assert_eq!(strip_prompt_timing(text), "[Shot 1] A singer sings in sync with the vocals in <Audio 1>, delivered softly");
    }

    #[test]
    fn repeated_shots_merge_and_renumber() {
        let text = "look: dark\n\n[Shot 1] A singer stands.\n\n[Shot 2] A singer stands.\n\n[Shot 3] The camera pulls back.\n\n[Shot 4] A singer stands.\n";
        let merged = strip_prompt_timing(text);
        assert_eq!(merged, "look: dark\n\n[Shot 1] A singer stands.\n\n[Shot 2] The camera pulls back.\n\n[Shot 3] A singer stands.\n");
    }

    #[test]
    fn a_prompt_without_repeats_comes_back_untouched() {
        let text = "[Shot 1] One.\n\n[Shot 2] Two.\n";
        assert_eq!(strip_prompt_timing(text), text);
    }
}
