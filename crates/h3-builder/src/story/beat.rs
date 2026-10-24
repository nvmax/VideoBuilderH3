//! The scene story beat: what one scene adds to the story, in plain visible action. Port of `_build_story_layer_scene_beat` in
//! `storyboard/story_layer.py`, with the checks and repairs that keep a beat visual, inside its location and inside its own cast.
//!
//! The FLF endpoint mode belongs to LTX and is not part of H3 Studio.

use fancy_regex::Regex;
use h3_core::pyjson::dumps;
use h3_core::pyval::{round_half_even, text_or_empty, to_float, truthy, Obj};
use h3_core::story::cast_guard::{build_cast_guard, cast_leaks, cast_wall_text, strip_cast_leaks, CastGuard};
use h3_core::story::helpers::{first_truthy, lyric_story_strength_guidance, normalize_story_layer};
use h3_core::story::mode::{is_scene_item_card, is_story_mode_layer, scene_items_text, story_beat_context, text};
use h3_core::story::scene_text::selected_scene;
use h3_llm::beat_prompts::{
    audio_language_repair_instruction, cast_repair_instruction, extra_mapping_repair_instruction, feeling_word_repair_instruction, location_repair_instruction, scene_beat_instruction, BeatInput,
};
use h3_llm::story_prompts::story_mode_lyric_guidance;
use serde_json::{json, Value};

use super::context::story_template;
use crate::prompt_writers::{number, whole, Llm, TextCall};

type Reply = Result<Value, String>;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn is_match(pattern: &str, text: &str) -> bool {
    rx(pattern).is_match(text).unwrap_or(false)
}

fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// `_scene_beat_has_audio_language`: whether a narrative scene beat leaks performance or audio metadata.
pub fn scene_beat_has_audio_language(value: &str) -> bool {
    is_match(r"(?i)\b(?:lip[\s-]?sync(?:ing)?|sings?|singing|sang|lyrics?|lyric|vocals?|vocalizing|vocalizes?|rapping|raps?|music|instrumental|performing vocals?)\b", value)
}

/// `_strip_scene_beat_audio_language`: remove common performance phrasing from a beat as a final safety net. Beats are visual story guidance.
pub fn strip_scene_beat_audio_language(value: &str) -> String {
    let mut cleaned = text(Some(&json!(value)), 1800);
    if cleaned.is_empty() {
        return String::new();
    }
    // Keep the useful visual clause in "Singer visibly sings ... as her fingertips trace ...".
    cleaned = rx(r"(?i)\b(?:the\s+)?(?:singer|performer|vocalist)\s+(?:visibly\s+)?(?:sings?|sang|is\s+singing|performs?)\b(?:\s+(?:through|during|over)\s+[^,.;!?]+)?\s+as\s+").replace_all(&cleaned, "").into_owned();
    cleaned = rx(r"(?i)\b(?:while|as)\s+(?:she|he|they|the\s+(?:singer|performer|vocalist))\s+(?:sings?|sang|is\s+singing|performs?)\b\s*[,]?\s*").replace_all(&cleaned, "while ").into_owned();
    // Drop any residual sentence that is solely an audio or performance aside.
    let sentences: Vec<&str> = rx(r"(?<=[.!?])\s+").split(&cleaned).flatten().filter(|s| !scene_beat_has_audio_language(s)).collect();
    let joined = sentences.join(" ");
    rx(r"\s{2,}").replace_all(&joined, " ").trim_matches([' ', '\t', '\n', ',', ';', ':', '-']).to_string()
}

const FEELING_WORDS: [&str; 16] = [
    "feel", "feels", "feeling", "feelings", "grief", "longing", "yearning", "memory", "memories", "soul", "emotion", "emotions", "emotional", "nostalgia", "nostalgic", "heartbreak",
];

/// `_feeling_word_hits`: the distinct feeling words in a text when it uses at least `minimum` of them, else none.
pub fn feeling_word_hits(value: &str, minimum: usize) -> Vec<String> {
    let words: Vec<&str> = FEELING_WORDS.iter().copied().chain(std::iter::once("heartbroken")).collect();
    let pattern = rx(&format!(r"(?i)\b(?:{})\b", words.join("|")));
    let hits: Vec<String> = pattern.find_iter(value).flatten().map(|m| m.as_str().to_lowercase()).collect();
    if hits.len() < minimum {
        return Vec::new();
    }
    let mut distinct = hits;
    distinct.sort();
    distinct.dedup();
    distinct
}

const DRIFT_PATTERNS: [(&str, &str); 13] = [
    (r"\bwarehouse\b", "warehouse"),
    (r"\bloading\s+dock\b", "loading dock"),
    (r"\bindustrial\b", "industrial"),
    (r"\bbackstage\s+corridor\b", "backstage corridor"),
    (r"\bnarrow,\s*dimly\s*lit\s+corridor\b", "dimly lit corridor"),
    (r"\bdark,\s*narrow\s+corridor\b", "dark corridor"),
    (r"\bheavy\s+(?:metal|steel)\s+door\b", "heavy metal/steel door"),
    (r"\bmassive\s+window\b", "massive window"),
    (r"\bconcrete\b", "concrete"),
    (r"\bmetal\s+pipes?\b", "metal pipes"),
    (r"\bsteel\s+stairs?\b", "steel stairs"),
    (r"\bvast,\s*silent\s+hall\b", "vast hall"),
    (r"\bvast\s+empty\s+space\b", "vast empty space"),
];

/// `_storyboard_scene_location_context`: the name, description and trigger of the scene's location, and its setting text.
fn scene_location_context(scene: &Value) -> String {
    if !scene.is_object() {
        return String::new();
    }
    let location = scene.get("location_ref").and_then(Value::as_object).cloned().unwrap_or_default();
    let trigger = first_truthy(&location, &["trigger_phrase", "trigger", "Trigger"]);
    let parts: Vec<String> = [location.get("name"), location.get("description"), trigger, scene.get("setting"), scene.get("location")]
        .into_iter()
        .map(|v| text_or_empty(v.filter(|x| truthy(x))))
        .filter(|p| !p.trim().is_empty())
        .collect();
    text(Some(&json!(parts.join(" "))), 2400)
}

/// `_storyboard_location_drift_terms`: places in the text that the mapped location does not have.
fn location_drift_terms(value: &str, location_context: &str) -> Vec<String> {
    let (text_lower, location_lower) = (value.to_lowercase(), location_context.to_lowercase());
    if text_lower.is_empty() || location_lower.is_empty() {
        return Vec::new();
    }
    DRIFT_PATTERNS.iter().filter(|(pattern, _)| is_match(&format!("(?i){pattern}"), &text_lower) && !is_match(&format!("(?i){pattern}"), &location_lower)).map(|(_, label)| label.to_string()).collect()
}

/// `_story_mode_beat_rules`: the project's own Story Mode beat rules (`story_mode_scene_beat`), or the built-in rules.
fn story_mode_beat_rules(payload: &Obj) -> String {
    let rules = story_template(payload, "story_mode_scene_beat");
    // Always added in code, so a saved copy of the rules cannot lose it.
    let believable = "- Keep it believable: the cast does ordinary, motivated things a real person would do in that place (sit, walk, pick something up, open a door, glance at someone), using the objects and furniture there. Avoid staged poses, long stares into the distance, standing at windows, and dramatic gestures with no purpose.\n";
    let marker = "- {{output_rules}}";
    if rules.contains(marker) { rules.replacen(marker, &format!("{believable}{marker}"), 1) } else { format!("{}\n{believable}", rules.trim_end()) }
}

fn strip_label(value: &str) -> String {
    rx(r"(?i)^\s*(scene\s+story\s+beat|story\s+beat|beat)\s*:\s*").replace(&text(Some(&json!(value)), 1800), "").into_owned()
}

fn cleaned_names(value: Option<&Value>) -> Vec<String> {
    array(value).iter().map(|i| text(Some(i), 180)).filter(|n| !n.is_empty()).collect()
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items.into_iter().filter(|i| seen.insert(i.clone())).collect()
}

/// `_build_story_layer_scene_beat`: one scene's story beat for its video prompt. In Story Mode the beat follows the scene's planned story slice.
pub async fn build_scene_beat(llm: &dyn Llm, payload: &Obj) -> Reply {
    let bundle = first_truthy(payload, &["storyboard_payload", "scene_bundle", "gpt_payload"]).filter(|v| v.is_object()).cloned();
    let Some(bundle) = bundle else { return Err("Storyboard scene-card payload is missing.".into()) };
    let scene = selected_scene(&bundle)?;
    let given_layer = first_truthy(payload, &["story_layer"]).cloned().or_else(|| bundle.get("story_layer").filter(|v| truthy(v)).cloned()).unwrap_or_else(|| json!({}));
    let story_layer = normalize_story_layer(Some(&given_layer));
    let previous_beat = text(payload.get("previous_beat"), 1200);
    let previous_lyrics = text(payload.get("previous_lyrics"), 800);
    let previous_end_state = text(payload.get("previous_end_state"), 1800);
    let previous_carry_forward = text(payload.get("previous_carry_forward"), 1800);
    let scene_map = scene.as_object().cloned().unwrap_or_default();
    let current_lyrics = {
        let own = first_truthy(payload, &["current_lyrics"]).or_else(|| first_truthy(&scene_map, &["lyrics", "lyric_text"]));
        text(own, 1200)
    };
    let next_lyrics = text(payload.get("next_lyrics"), 800);
    let flf_mode = payload.get("flf_mode").is_some_and(truthy) || text_or_empty(scene.get("video_prompt_type")).trim().to_lowercase() == "flf";
    if flf_mode {
        return Err("FLF endpoint beats are not part of H3 Studio.".into());
    }
    let vocal_status = scene.get("vocal_status").and_then(Value::as_object).cloned().unwrap_or_default();
    // Scene beat generation is also called from Image Prep, where the normal prompt payload clears vocal_status.singers. The scene's
    // lyric_singers is the performer assignment that stays valid in both Image Prep and Video Prep.
    let mut assigned = match scene.get("lyric_singers") {
        Some(Value::Array(items)) => cleaned_names(Some(&Value::Array(items.clone()))),
        _ => Vec::new(),
    };
    if assigned.is_empty() {
        if let Some(Value::Array(items)) = vocal_status.get("singers") {
            assigned = cleaned_names(Some(&Value::Array(items.clone())));
        }
    }
    let performer_assignment = scene.get("performer_assignment").and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(Value::Array(singing)) = performer_assignment.get("singing") {
        assigned = cleaned_names(Some(&Value::Array(singing.clone())));
    }
    if assigned.is_empty() {
        let mut mapped: Vec<String> = Vec::new();
        for item in array(scene.get("subject_refs")) {
            if let Some(map) = item.as_object() {
                let name = text(map.get("name"), 180);
                if !name.is_empty() {
                    mapped.push(name);
                }
            }
        }
        for item in array(scene.get("subjects")) {
            let name = match &item {
                Value::Object(map) => text(map.get("name"), 180),
                other => text(Some(other), 180),
            };
            if !name.is_empty() {
                mapped.push(name);
            }
        }
        let singers: Vec<String> = dedupe(mapped).into_iter().filter(|name| is_match(r"(?i)\b(?:singer|performer|vocalist|rapper)\b", name)).collect();
        if singers.len() == 1 {
            assigned = singers;
        }
    }
    let mut silent: Vec<String> = match performer_assignment.get("silent") {
        Some(Value::Array(items)) => cleaned_names(Some(&Value::Array(items.clone()))),
        _ => Vec::new(),
    };
    let pick = |keys: &[&str], limit: usize| text(first_truthy(&scene_map, keys), limit);
    let scene_defaults = json!({
        "shot_type": pick(&["shot_type", "shot"], 240),
        "camera_motion": pick(&["camera_motion", "camera_motion_preset"], 500),
        "camera_flow": pick(&["camera_flow", "cameraFlow"], 120),
        "camera_flow_guidance": pick(&["camera_flow_guidance"], 1200),
        "character_motion": pick(&["character_motion", "character_motion_preset"], 700),
        "performance_direction": pick(&["performance_direction", "performance_style"], 1000),
        "facial_performance_direction": pick(&["facial_performance_direction", "facial_performance_custom", "facial_performance"], 1200),
        "emotion_expression_tags": pick(&["emotion_expression_tags"], 1200),
    });
    let mut extra_subjects: Vec<Value> = Vec::new();
    if let Some(Value::Array(raw)) = first_truthy(&scene_map, &["extra_subjects", "extraSubjects"]) {
        for (index, item) in raw.iter().take(100).enumerate() {
            let Some(item) = item.as_object() else { continue };
            let name = match first_truthy(item, &["name", "title"]) {
                Some(v) => text(Some(v), 180),
                None => text(Some(&json!(format!("Extra {}", index + 1))), 180),
            };
            if name.is_empty() {
                continue;
            }
            let interaction = first_truthy(item, &["interaction"]).map(|v| text_or_empty(Some(v))).unwrap_or_else(|| "background".into()).trim().to_string();
            let interaction = if matches!(interaction.as_str(), "background" | "background_dancing" | "alongside" | "dancing_with" | "direct") { interaction } else { "background".into() };
            let count = item.get("count").filter(|v| truthy(v)).map_or(Some(1.0), to_float).filter(|f| f.is_finite()).map_or(1, |f| round_half_even(f).clamp(1, 100));
            extra_subjects.push(json!({"name": name, "count": count, "interaction": interaction, "identity": text(first_truthy(item, &["identity", "description"]), 240)}));
        }
    }
    if ["no_character_present", "noCharacterPresent", "no_visible_subject", "no_subject"].iter().any(|k| scene.get(*k).is_some_and(truthy)) {
        extra_subjects = Vec::new();
    }
    let beat_word_limit = if extra_subjects.is_empty() { 80 } else { 100 };
    // Hard wall: characters not selected for this scene may not appear in its beat. Clothing and prop cards mapped to the scene are things
    // the people wear or use, so they are listed apart and never banned or counted as people.
    let mapped_refs: Vec<Value> = if scene.get("no_character_present").is_some_and(truthy) || scene.get("noCharacterPresent").is_some_and(truthy) {
        Vec::new()
    } else {
        array(scene.get("subject_refs")).into_iter().filter(|item| item.as_object().is_some_and(|m| !text_or_empty(m.get("name")).trim().is_empty())).collect()
    };
    let scene_cast: Vec<Value> = mapped_refs.iter().filter(|i| !is_scene_item_card(i)).cloned().collect();
    let scene_items: Vec<Value> = mapped_refs.iter().filter(|i| is_scene_item_card(i)).cloned().collect();
    let item_names: Vec<String> = scene_items.iter().map(|i| text(i.get("name"), 180).to_lowercase()).collect();
    silent.retain(|name| !item_names.contains(&name.to_lowercase()));
    let mut all_subjects: Vec<Value> = array(payload.get("all_subjects")).into_iter().filter(Value::is_object).collect();
    if let Some(builder) = first_truthy(payload, &["reference_builder", "referenceBuilder"]).and_then(Value::as_object) {
        all_subjects.extend(array(builder.get("subjects")).into_iter().filter(Value::is_object));
    }
    all_subjects.retain(|i| !is_scene_item_card(i));
    let everyone: Vec<Value> = all_subjects.iter().cloned().chain(scene_cast.iter().cloned()).collect();
    let extra_names: Vec<String> = extra_subjects.iter().map(|i| text_or_empty(i.get("name"))).collect();
    let guard: CastGuard = build_cast_guard(&everyone, &scene_cast, &extra_names);
    let location = scene.get("location_ref").and_then(Value::as_object).cloned().unwrap_or_default();
    let scene_override = text(scene.get("scene_override"), 800);
    let override_block = if scene_override.is_empty() {
        String::new()
    } else {
        format!("SCENE OVERRIDE, set by the user and always true for this scene: {scene_override}\nIt replaces anything in the story, the arc, the lyric, the location, the notes or the previous beat that disagrees with it. Change every conflicting detail to match it, and make it clearly visible in the beat. For example, an override of night time means the scene happens at night and is lit like night even if the story says morning; an override of raining means it rains through the whole scene.")
    };
    let location_block = if location.get("name").is_some_and(truthy) { format!("Location: {} \u{2014} {}", text(location.get("name"), 120), text(location.get("description"), 400)) } else { String::new() };
    let scene_facts = [
        override_block,
        cast_wall_text(&guard),
        scene_items_text(&scene_items, &scene_cast),
        location_block,
        if previous_end_state.is_empty() { String::new() } else { format!("Previous end state: {previous_end_state}") },
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    // Story Mode: the beat follows this scene's planned story slice instead of the song-section arc.
    let story_context = if is_story_mode_layer(&story_layer) { story_beat_context(&story_layer, &text_or_empty(scene.get("id")), scene.get("scene_number")) } else { None };
    let story_mode = story_context.is_some();
    let (arc_text, brief_text, lyric_guidance) = match &story_context {
        Some((arc, brief)) => (arc.clone(), brief.clone(), story_mode_lyric_guidance(Some(story_layer.get("lyric_story_strength").unwrap_or(&json!(5))))),
        None => (text_or_empty(story_layer.get("user_story_arc")), text_or_empty(story_layer.get("song_story_brief")), lyric_story_strength_guidance(&story_layer)),
    };
    let rules_template = if story_mode { Some(story_mode_beat_rules(payload)) } else { None };
    let performance_json = dumps(&json!({"singing": assigned, "silent": silent}));
    let extras_json = if extra_subjects.is_empty() { "[none]".to_string() } else { dumps(&Value::Array(extra_subjects.clone())) };
    let instruction = scene_beat_instruction(&BeatInput {
        flf_mode,
        beat_word_limit,
        lyric_story_strength_guidance_text: &lyric_guidance,
        user_story_arc: &arc_text,
        song_story_brief: &brief_text,
        previous_beat: &previous_beat,
        previous_lyrics: &previous_lyrics,
        previous_end_state: &previous_end_state,
        previous_carry_forward: &previous_carry_forward,
        current_lyrics: &current_lyrics,
        next_lyrics: &next_lyrics,
        scene_defaults_json: &dumps(&scene_defaults),
        performance_assignment_json: &performance_json,
        extra_subjects_json: &extras_json,
        scene_json: &dumps(&scene),
        cast_wall: &scene_facts,
        story_mode,
        rules_template: rules_template.as_deref(),
    });
    let temperature = number(payload, "temperature", 0.35)?;
    let top_p = number(payload, "top_p", 0.90)?;
    let tokens = whole(payload, "max_new_tokens", 360)?;
    let ask = |instruction: String, temperature: f64, top_p: f64, max_new_tokens: i64, label: &str| llm.text(TextCall::new(instruction, temperature, top_p, max_new_tokens, label).paragraphs());
    let (reply, info) = ask(instruction, temperature, top_p, tokens, "Storyboard Scene Beat Gemma").await?;
    let mut beat = strip_label(&reply);
    if beat.is_empty() {
        return Err("Gemma returned an empty scene story beat.".into());
    }
    if scene_beat_has_audio_language(&beat) {
        let (repaired, _) = ask(audio_language_repair_instruction(&beat), 0.10, 0.80, 300, "Storyboard Scene Beat Audio Language Repair Gemma").await?;
        let repaired = strip_label(&repaired);
        if !repaired.is_empty() && !scene_beat_has_audio_language(&repaired) {
            beat = repaired;
        } else {
            beat = strip_scene_beat_audio_language(&beat);
        }
    } else {
        beat = strip_scene_beat_audio_language(&beat);
    }
    let location_context = scene_location_context(&scene);
    let drift = location_drift_terms(&beat, &location_context);
    if !drift.is_empty() {
        let (repaired, _) = ask(location_repair_instruction(&location_context, &drift, &beat), 0.20, 0.85, 300, "Storyboard Scene Beat Location Repair Gemma").await?;
        let repaired = strip_label(&repaired);
        if !repaired.is_empty() && location_drift_terms(&repaired, &location_context).is_empty() {
            beat = repaired;
        } else {
            let first = array(scene.get("subject_refs")).into_iter().next();
            let hint = match scene.get("subject_refs").filter(|v| truthy(v)) {
                None => "The scene subject".to_string(),
                Some(_) => text(Some(&first.as_ref().and_then(|f| f.get("name")).filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("The scene subject"))), 120),
            };
            beat = format!("{hint} moves through {location_context}, with a steady camera move following the action. The beat stays inside the mapped location and shows the action through posture, hands, and gaze.");
        }
    }
    let missing: Vec<String> = extra_subjects.iter().map(|i| text_or_empty(i.get("name"))).filter(|n| !beat.to_lowercase().contains(&n.to_lowercase())).collect();
    if !missing.is_empty() {
        let (repaired, _) = ask(extra_mapping_repair_instruction(&dumps(&Value::Array(extra_subjects.clone())), &beat), 0.15, 0.82, 360, "Storyboard Scene Beat Extra Mapping Repair Gemma").await?;
        let repaired = strip_label(&repaired);
        let still: Vec<String> = extra_subjects.iter().map(|i| text_or_empty(i.get("name"))).filter(|n| !repaired.to_lowercase().contains(&n.to_lowercase())).collect();
        if repaired.is_empty() || !still.is_empty() {
            return Err(format!("Gemma omitted mapped extras from the scene story beat after repair: {}", if still.is_empty() { missing.join(", ") } else { still.join(", ") }));
        }
        beat = repaired;
    }
    beat = strip_scene_beat_audio_language(&beat);
    if beat.is_empty() {
        return Err("Scene beat became empty after removing audio/performance language.".into());
    }
    let leaked = cast_leaks(&beat, &guard, true);
    if !leaked.is_empty() {
        let (repaired, _) = ask(cast_repair_instruction(&cast_wall_text(&guard), &leaked, &beat), 0.15, 0.80, 300, "Storyboard Scene Beat Cast Repair Gemma").await?;
        let repaired = strip_label(&repaired);
        // The filter is the guarantee: whatever still refers to a person outside the cast is removed. Invented-person words are a softer
        // signal, so if removing them would leave too little, only named off-cast characters go.
        let candidate = if !repaired.is_empty() && cast_leaks(&repaired, &guard, true).is_empty() { repaired } else { beat.clone() };
        let mut stripped = strip_cast_leaks(&candidate, &guard, true);
        if stripped.split_whitespace().count() < 30 {
            stripped = strip_cast_leaks(&candidate, &guard, false);
        }
        beat = stripped;
        if beat.is_empty() {
            let names = if guard.cast_names.is_empty() { "The scene".to_string() } else { guard.cast_names.join(", ") };
            beat = format!("The scene stays focused on {names}, shown through posture, hands, and the mapped location.");
        }
    }
    let feeling = feeling_word_hits(&beat, 2);
    if !feeling.is_empty() {
        let (rewritten, _) = ask(feeling_word_repair_instruction(&feeling, &beat, beat_word_limit), 0.15, 0.80, 300, "Storyboard Scene Beat Feeling Word Repair").await?;
        let rewritten = strip_label(&rewritten);
        // Keep the original if the rewrite is worse: empty, still full of feeling words, or bringing back off-cast people.
        if !rewritten.is_empty() && feeling_word_hits(&rewritten, 2).is_empty() && cast_leaks(&rewritten, &guard, true).is_empty() {
            let cleaned = strip_scene_beat_audio_language(&rewritten);
            if !cleaned.is_empty() {
                beat = cleaned;
            }
        }
    }
    let mut out = Obj::new();
    out.insert("story_beat".into(), json!(beat));
    out.insert("runner".into(), info.get("runner").cloned().unwrap_or_else(|| json!("builtin")));
    out.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    out.insert("unloaded".into(), info.get("unloaded").cloned().unwrap_or_else(|| json!(true)));
    Ok(Value::Object(out))
}
