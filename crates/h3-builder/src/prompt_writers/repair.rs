//! Repairing and checking a written prompt. Port of `_repair_builder_gemma_prompt`, `_repair_and_validate_builder_gemma_prompt`
//! and `_clean_visual_only_positive_prompt` in `llm/builder_runner.py`.

use fancy_regex::Regex;
use h3_core::pyval::{text_or_empty, Obj};
use h3_llm::checks::{clean_visual_text, looks_like_repeat_failure, looks_like_unfilled_template};
use h3_llm::prompt_text::{looks_like_id_lora_script_prompt, looks_like_source_lyric_echo, validate_builder_prompt};

use super::{Llm, TextCall};

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn field(payload: &Obj, keys: &[&str], limit: usize) -> String {
    let found = keys.iter().map(|k| text_or_empty(payload.get(*k)).trim().to_string()).find(|t| !t.is_empty()).unwrap_or_default();
    found.chars().take(limit).collect()
}

/// `_repair_builder_gemma_prompt`: a prompt that is a loop, an unfilled template or the scene's lyric is rewritten once by the
/// runner. The original is kept when the rewrite is no better.
pub async fn repair_prompt(llm: &dyn Llm, payload: &Obj, text: &str, label: &str) -> String {
    let original = text.trim().to_string();
    let label_key = label.trim().to_lowercase().replace(' ', "_");
    if matches!(label_key.as_str(), "id-lora" | "id_lora" | "id-lora_i2v") && looks_like_id_lora_script_prompt(&original) {
        return original;
    }
    let needs_repair = looks_like_repeat_failure(&original) || looks_like_unfilled_template(&original) || looks_like_source_lyric_echo(&original, payload);
    if original.is_empty() || !needs_repair {
        return original;
    }
    let broken: String = original.chars().take(5000).collect();
    let label_text = label.trim();
    let instruction = if matches!(label_text.to_lowercase().as_str(), "i2v" | "t2v") {
        let concept = field(payload, &["t2i_prompt"], 3000);
        let motion = field(payload, &["user_notes"], 2000);
        format!(
            "Clean this broken {label_text} video prompt into one usable final video prompt.\n\n\
             The broken text may contain internal thoughts, repeated tokens, markdown, or unfilled square-bracket placeholders.\n\
             Use only the concept prompt and motion notes below as context. Do not use project story, lyrics, agent chat, or any other context.\n\
             Replace placeholders like [Subject], [setting/environment], [time/weather], and [Camera Motion] with concrete details from the concept prompt and motion notes.\n\
             Do not continue the broken text. Do not explain the repair. Do not mention that it was repaired.\n\
             Return exactly one normal video prompt paragraph with no square brackets, labels, markdown, or placeholders.\n\
             Keep it under 120 words.\n\n\
             Concept/T2I prompt:\n{}\n\n\
             I2V/T2V motion notes:\n{}\n\n\
             Broken video prompt:\n{broken}",
            if concept.is_empty() { "[none provided]" } else { &concept },
            if motion.is_empty() { "[none provided]" } else { &motion },
        )
    } else {
        let notes = field(payload, &["user_notes"], 3000);
        let lyric = field(payload, &["lyric_text", "lyrics"], 1200);
        format!(
            "Clean this broken {label_text} image prompt into one usable final prompt.\n\n\
             The broken text may contain internal thoughts, analysis, channel tags, repeated tokens, markdown, unfilled square-bracket placeholders, junk, or the raw scene lyric copied by mistake.\n\
             Do not continue the broken text. Do not explain the repair. Do not mention that it was repaired.\n\
             Return exactly one normal image prompt paragraph.\n\
             Remove all thought, analysis, channel, role, markdown, labels, square brackets, placeholder words, and repeated junk.\n\
             If the broken text is just the scene lyric, do not quote the lyric; create a visual still-image prompt from the user notes and lyric mood.\n\
             Replace placeholders like [Subject], [setting/environment], [time/weather], and [Camera Motion] with concrete details inferred from the broken text and user notes.\n\
             Keep only usable visual image-generation content. If usable details are scarce, create a concise cinematic prompt from the usable fragments.\n\
             Keep it under 120 words.\n\n\
             User notes/context:\n{}\n\n\
             Scene lyric, for mood only:\n{}\n\n\
             Broken text:\n{broken}",
            if notes.is_empty() { "[none provided]" } else { &notes },
            if lyric.is_empty() { "[none provided]" } else { &lyric },
        )
    };
    let call = TextCall::new(instruction, 0.25, 0.85, 350, &format!("{label_text} repair Gemma"));
    match llm.text(call).await {
        Ok((reply, _)) => {
            let repaired = clean_visual_text(&reply);
            if !repaired.is_empty() && !looks_like_repeat_failure(&repaired) && !looks_like_unfilled_template(&repaired) {
                return repaired;
            }
            original
        }
        Err(_) => original,
    }
}

/// `_clean_visual_only_positive_prompt`: sentences about singing, mouths or voices and every negation are left out.
pub fn clean_visual_only_prompt(text: &str) -> String {
    let forbidden = rx(r"(?i)\b(?:lip[ -]?sync(?:ing|s)?|sing(?:s|ing)?|sang|sung|rap(?:s|ping)?|vocal(?:s|ization)?|lyric(?:s)?|speak(?:s|ing)?|say(?:s|ing)?|said|dialogue|mouth(?:s|ed|ing)?|lips?)\b");
    let negative = rx(r"(?i)\b(?:no|not|never|without|avoid|omit|exclude|prevent|don['\u{2019}]t|doesn['\u{2019}]t|isn['\u{2019}]t|aren['\u{2019}]t|cannot|can['\u{2019}]t|do\s+not|does\s+not)\b");
    let parts: Vec<String> = rx(r"(?<=[.!?])\s+|\s*;\s*").split(text).flatten().map(str::to_string).collect();
    let kept: Vec<String> = parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty() && !forbidden.is_match(p).unwrap_or(false) && !negative.is_match(p).unwrap_or(false)).collect();
    rx(r"\s{2,}").replace_all(&kept.join(" "), " ").trim().to_string()
}

/// `_repair_and_validate_builder_gemma_prompt`: repair, keep a no lip sync prompt visual only, and check. Errors read like the
/// pack's.
pub async fn repair_and_validate(llm: &dyn Llm, payload: &Obj, text: &str, label: &str) -> Result<String, String> {
    let mut repaired = repair_prompt(llm, payload, text, label).await;
    let mode = ["performance_mode", "performanceMode", "video_type", "videoType"].iter().map(|k| text_or_empty(payload.get(*k)).trim().to_lowercase().replace(['-', ' '], "_")).find(|m| !m.is_empty()).unwrap_or_default();
    if matches!(mode.as_str(), "no_lip_sync" | "nolipsync" | "no_lipsync" | "no_sync" | "silent" | "visual_only") {
        repaired = clean_visual_only_prompt(&repaired);
    }
    validate_builder_prompt(&repaired, label, Some(payload))?;
    Ok(repaired)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Asked, Canned};
    use super::*;
    use serde_json::json;

    fn payload(value: serde_json::Value) -> Obj {
        value.as_object().cloned().unwrap()
    }

    #[tokio::test]
    async fn a_good_prompt_is_left_alone_and_a_broken_one_is_rewritten_once() {
        let llm = Canned::new(&["A woman waits in a rain soaked alley at night."]);
        let ok = repair_and_validate(&llm, &payload(json!({})), "A quiet street at dusk with warm lamps.", "T2I").await.unwrap();
        assert_eq!(ok, "A quiet street at dusk with warm lamps.");
        assert!(llm.asked.lock().unwrap().is_empty());
        let fixed = repair_and_validate(&llm, &payload(json!({"user_notes": "alley"})), "A shot of [Subject] in [setting/environment] during [time/weather].", "T2I").await.unwrap();
        assert_eq!(fixed, "A woman waits in a rain soaked alley at night.");
        let asked = llm.asked.lock().unwrap().clone();
        match &asked[0] {
            Asked::Text(call) => {
                assert_eq!((call.temperature, call.top_p, call.max_new_tokens, call.label.as_str()), (0.25, 0.85, 350, "T2I repair Gemma"));
                assert!(call.instruction.contains("User notes/context:\nalley\n\nScene lyric, for mood only:\n[none provided]\n\nBroken text:\nA shot of [Subject]"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_rewrite_that_is_no_better_is_not_used_and_the_check_reports_it() {
        let llm = Canned::new(&["[Subject] again [Camera Motion]"]);
        let err = repair_and_validate(&llm, &payload(json!({})), "A shot of [Subject] in [setting/environment] during [time/weather].", "NanoBanana").await.unwrap_err();
        assert_eq!(err, "Gemma returned an unfilled template for the NanoBanana prompt. Try again or add more specific scene/motion notes.");
        let none = Canned::new(&[]);
        assert!(repair_and_validate(&none, &payload(json!({})), "A shot of [Subject] in [setting/environment] during [time/weather].", "T2I").await.is_err());
    }

    #[test]
    fn a_visual_only_prompt_drops_singing_and_negations() {
        assert_eq!(clean_visual_only_prompt("A woman sings softly. A quiet alley at night; no people look at the camera. Warm lamps glow."), "A quiet alley at night Warm lamps glow.");
        assert_eq!(clean_visual_only_prompt("Mouth open. Lips part."), "");
    }
}
