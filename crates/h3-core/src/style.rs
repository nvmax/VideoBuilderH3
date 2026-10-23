//! The cut plan, video style, temporal effect and FX wording around a MiniMax prompt. Port of
//! `storyboard_cut_plan_for_duration` (`minimax/prompt_assembly.py`), `agent_api/video_style.py` and
//! `agent_api/temporal_effect.py`. The preset wording is data copied from the pack.

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::prompt::rx::{rx, sub_with};
use crate::pyval::{round_half_even, round_to, truthy, Obj};

fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(v) => crate::pyval::py_str(Some(v)).trim().to_string(),
    }
}

/// Shot cuts for a scene length and a cut frequency (0 to 10), at most one cut per second.
pub fn storyboard_cut_plan_for_duration(duration: f64, frequency: i64) -> Obj {
    let dur = if duration == 0.0 || !duration.is_finite() { 4.0 } else { duration }.max(0.1);
    let freq = frequency.clamp(0, 10);
    let maximum_cuts = ((dur - 1e-6).ceil() as i64 - 1).max(0);
    let cut_count = if freq <= 0 || maximum_cuts <= 0 {
        0
    } else if freq >= 10 {
        maximum_cuts
    } else {
        (maximum_cuts - 1).max(1).min(round_half_even(maximum_cuts as f64 * freq as f64 / 10.0).max(1))
    };
    let cut_times: Vec<f64> = if cut_count == 0 {
        vec![]
    } else if cut_count == maximum_cuts {
        (0..cut_count).map(|i| (i + 1) as f64).collect()
    } else {
        (0..cut_count).map(|i| round_to(dur * (i + 1) as f64 / (cut_count + 1) as f64, 3)).collect()
    };
    let mut plan = Obj::new();
    plan.insert("frequency".into(), json!(freq));
    plan.insert("exact_duration_seconds".into(), json!(round_to(dur, 3)));
    plan.insert("maximum_one_per_second_cuts".into(), json!(maximum_cuts));
    plan.insert("cut_count".into(), json!(cut_count));
    plan.insert("shot_count".into(), json!(cut_count + 1));
    plan.insert("cut_times_seconds".into(), json!(cut_times));
    plan.insert("continuous_shot".into(), json!(cut_count == 0));
    plan
}

fn style_data() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("../data/video_style_data.json")).expect("video style data"))
}

fn temporal_data() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("../data/temporal_presets.json")).expect("temporal data"))
}

/// The style key for a scene: its own choice, else the project default.
pub fn video_style_value(card: &Obj, defaults: &Obj) -> String {
    let own = text(card.get("video_style"));
    if own.is_empty() { text(defaults.get("video_style")) } else { own }
}

/// The name written in the prompt's opening line.
pub fn video_style_label(card: &Obj, defaults: &Obj) -> String {
    let value = video_style_value(card, defaults);
    if value == "custom" {
        return if truthy(card.get("video_style").unwrap_or(&Value::Null)) { text(card.get("video_style_custom")) } else { text(defaults.get("video_style_custom")) };
    }
    style_data()["video_styles"].get(&value).and_then(|s| s["label"].as_str()).map(str::to_string).unwrap_or(value)
}

/// The style paragraph (`Label: guidance`, or the custom text), or empty when no style is set.
pub fn video_style_verbiage(card: &Obj, defaults: &Obj) -> String {
    let value = video_style_value(card, defaults);
    if value == "custom" {
        return video_style_label(card, defaults);
    }
    match style_data()["video_styles"].get(&value) {
        Some(preset) if preset["guidance"].as_str().is_some_and(|g| !g.is_empty()) => format!("{}: {}", preset["label"].as_str().unwrap_or(""), preset["guidance"].as_str().unwrap_or("")),
        _ => String::new(),
    }
}

/// The style's look, written after its name in the opening line.
pub fn video_style_guidance(card: &Obj, defaults: &Obj) -> String {
    style_data()["video_styles"].get(video_style_value(card, defaults)).and_then(|s| s["guidance"].as_str()).map(|g| g.trim().to_string()).unwrap_or_default()
}

/// The exact temporal / world effect block for one scene, or empty when it has none.
pub fn temporal_world_effect_verbiage(card: &Obj, defaults: &Obj) -> String {
    let override_ = Some(text(card.get("temporal_world_effect_override"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "global".into());
    let key = if override_ == "off" {
        String::new()
    } else if override_ != "global" {
        override_.clone()
    } else {
        text(defaults.get("temporal_world_effect"))
    };
    let data = temporal_data();
    let Some(preset) = data["presets"].get(&key) else { return String::new() };
    let own_custom = key == "custom" && override_ != "global";
    let custom = text(if own_custom { card.get("temporal_world_effect_custom") } else { defaults.get("temporal_world_effect_custom") });
    let base = if key == "custom" { custom } else { preset["guidance"].as_str().unwrap_or("").to_string() };
    if base.is_empty() {
        return String::new();
    }
    let mode = text(defaults.get("temporal_protected_characters"));
    let protected_custom = text(defaults.get("temporal_protected_custom"));
    let protected = if mode == "lead_only" {
        "Protect only the first mapped/reference character at natural 1x real-time speed; other mapped characters may receive the selected temporal effect.".to_string()
    } else if mode == "custom" && !protected_custom.is_empty() {
        format!("Protect only these named mapped/reference characters at natural 1x real-time speed: {protected_custom}.")
    } else {
        "Protect every mapped/reference character in the scene at natural 1x real-time speed, including secondary referenced characters. Never accelerate, freeze, reverse, echo, duplicate, or temporally distort any protected character.".to_string()
    };
    format!(
        "Temporal / World Effect — {} — Mandatory:\n\n{base}\n\n{protected}\n\nOnly the unprotected background/world may receive this effect, including location-appropriate environmental motion or anonymous extras when permitted. Keep the effect secondary; never alter, duplicate, replace, or obscure a mapped/reference character.\n\nTimestamp staging requirement: every shot must visibly show at least one concrete, readable effect affecting only the unprotected background/world while protected characters continue moving naturally at normal 1x speed.\n\n{}",
        preset["label"].as_str().unwrap_or(""),
        data["audio_direction"].as_str().unwrap_or("")
    )
}

/// Put the contract once, last. A copy the LLM wrote inside a shot is removed first.
pub fn append_temporal_world_effect(prompt: &str, verbiage: &str) -> String {
    let body = prompt.trim();
    if body.is_empty() || verbiage.is_empty() {
        return body.to_string();
    }
    let mut cleaned = body.replace(verbiage, "");
    while cleaned.contains("\n\n\n") {
        cleaned = cleaned.replace("\n\n\n", "\n\n");
    }
    format!("{}\n\n{verbiage}", cleaned.trim()).trim().to_string()
}

fn custom_fx(raw: Option<&Value>) -> Option<(Vec<String>, String)> {
    let parsed: Value = match raw {
        Some(Value::String(s)) => serde_json::from_str(s).ok()?,
        Some(v) => v.clone(),
        None => return None,
    };
    let source = match parsed {
        Value::Array(items) => json!({"cues": items}),
        other => other,
    };
    let source = source.as_object()?;
    let items = source.get("cues").and_then(Value::as_array).or_else(|| source.get("effects").and_then(Value::as_array)).cloned().unwrap_or_default();
    let mut cues: Vec<String> = items
        .iter()
        .map(|i| match i {
            Value::String(s) => s.trim().to_string(),
            Value::Object(o) => ["text", "cue", "description"].iter().filter_map(|k| o.get(*k)).find(|v| truthy(v)).map(|v| text(Some(v))).unwrap_or_default(),
            _ => String::new(),
        })
        .filter(|c| !c.is_empty())
        .take(12)
        .collect();
    let pick = |a: &str, b: &str| -> String { Some(text(source.get(a))).filter(|s| !s.is_empty()).unwrap_or_else(|| text(source.get(b))) };
    let (primary, secondary) = (pick("primary", "primary_effect"), pick("secondary", "secondary_effect"));
    if !primary.is_empty() {
        cues.insert(0, primary);
    }
    if !secondary.is_empty() {
        cues.push(secondary);
    }
    if cues.is_empty() {
        return None;
    }
    let mut unique: Vec<String> = Vec::new();
    for c in cues {
        if !unique.contains(&c) {
            unique.push(c);
        }
    }
    unique.truncate(12);
    let timing = Some(text(source.get("timing"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "on the strongest musical or action accent".into());
    Some((unique, timing.chars().take(240).collect()))
}

/// The FX sentence for one shot, or empty when FX are off.
pub fn fx_accent(defaults: &Obj, shot_index: usize) -> String {
    let preset = text(defaults.get("fx_preset"));
    if preset.is_empty() {
        return String::new();
    }
    let custom = if preset == "custom" { custom_fx(defaults.get("fx_custom_json")) } else { None };
    let (cues, timing): (Vec<String>, String) = match custom {
        Some((cues, timing)) => (cues, timing),
        None => (
            style_data()["fx"].get(&preset).and_then(|p| p["cues"].as_array()).map(|a| a.iter().map(|c| text(Some(c))).collect()).unwrap_or_default(),
            "on a readable musical, lyric, or action accent when one is present".to_string(),
        ),
    };
    if cues.is_empty() {
        return String::new();
    }
    format!("FX accent: {} Timing: {timing}. Keep the mapped subject stable and readable.", cues[shot_index % cues.len()])
}

/// Characters the FX sentences add to a prompt of `shot_count` shots.
pub fn fx_reserve(defaults: &Obj, shot_count: usize) -> usize {
    (0..shot_count).map(|i| fx_accent(defaults, i)).filter(|a| !a.is_empty()).map(|a| a.chars().count() + 1).sum()
}

/// Add each shot's FX sentence at the end of that shot. `core` holds only the shots.
pub fn apply_fx(core: &str, defaults: &Obj) -> String {
    if core.trim().is_empty() || fx_accent(defaults, 0).is_empty() {
        return core.to_string();
    }
    let counter = std::cell::Cell::new(0usize);
    sub_with(rx!(r"(?i)(\[\s*Shot\s+\d+[^\]]*\]\s*\n?)([\s\S]*?)(?=\n\s*\[\s*Shot\s+\d+[^\]]*\]|$)"), core, |caps| {
        let accent = fx_accent(defaults, counter.get());
        counter.set(counter.get() + 1);
        let whole = caps.get(0).map_or("", |m| m.as_str()).to_string();
        let head = caps.get(1).map_or("", |m| m.as_str());
        let body = caps.get(2).map_or("", |m| m.as_str());
        let cue = accent.split("FX accent: ").nth(1).and_then(|r| r.split(" Timing:").next()).unwrap_or("");
        if body.contains(cue) { whole } else { format!("{head}{} {accent}", body.trim()) }
    })
}
