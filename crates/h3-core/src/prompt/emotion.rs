//! The acting direction asked of the LLM. Port of `llm/prompts/emotion_expression.py`.

use serde_json::Value;

use crate::pyval::{text_or_empty, truthy, Obj};

fn field(payload: &Obj, key: &str) -> String {
    text_or_empty(payload.get(key)).trim().to_string()
}

/// The emotion tags and facial direction of a scene, with the project's defaults where the scene has none.
pub fn emotion_expression_input(segment: &Obj, session: &Obj) -> Obj {
    let pick = |seg_key: &str, session_key: &str| -> String {
        let own = field(segment, seg_key);
        if !own.is_empty() { own } else { field(session, session_key) }
    };
    let mut out = Obj::new();
    out.insert("emotion_expression_tags".into(), Value::String(field(segment, "emotion_expression_tags")));
    out.insert("facial_performance".into(), Value::String(pick("facial_performance", "default_facial_performance")));
    out.insert("facial_performance_custom".into(), Value::String(pick("facial_performance_custom", "default_facial_performance_custom")));
    out
}

/// Explicit scene tags override a preset, including Off.
pub fn has_emotion_expression_input(payload: &Obj) -> bool {
    let facial = payload.get("facial_performance");
    let facial_off = facial.and_then(Value::as_str) == Some("off");
    payload.get("emotion_expression_tags").is_some_and(truthy)
        || (!facial_off && (facial.is_some_and(truthy) || payload.get("facial_performance_custom").is_some_and(truthy)))
}

const SPEECH: &str = concat!(
    "\n\nBUILT-IN H3 SPEECH DELIVERY:\n",
    "Use the scene beat, storyboard, spoken words and requested facial/emotion direction to stage the speaker's visible acting and generated vocal delivery. ",
    "Put language and fitting emotion descriptors in the dialogue header, for example <d>[English, curious] exact dialogue.</d> ",
    "The says/speaks verb establishes speech; do not substitute a generic speaking descriptor for the actual emotion. ",
    "INLINE DELIVERY TAGS ARE REQUIRED: An emotion header alone is incomplete. In every shot with spoken dialogue, ",
    "include at least one suitable inline delivery cue inside <d>, chosen from the scene, dialogue meaning and requested emotion. ",
    "For several sentences, stage the delivery across relevant phrase boundaries and emphasized words rather than tagging only the header. ",
    "Use pauses for changes of thought, breath cues for a motivated breath, and <i>...</i> for the existing words the speaker stresses. ",
    "Place each cue at the phrase it affects. Choose from: ",
    "<pause>, <long pause>, <breath>, <inhale>, <exhale>, <catches breath>, <deep breath>, ",
    "<laughs>, <chuckle>, <sighs>, <uh>, <stutter>, <gasp>, <coughs>, <clears throat>, <sniff>, ",
    "<pant>, <pants>, <softer>, <mhm>, <phew>, and <smacks lips></smacks lips>. ",
    "For emphasis wrap only 1–4 existing words in <i>...</i>; use <whisper>...</whisper> or <humming>...</humming> for an explicitly requested delivery. ",
    "Preserve the supplied spoken words and their order; tags guide performance without adding, repeating or rewriting dialogue. ",
    "Preserve existing delivery tags. Keep pauses and breathing within the scene's available duration. ",
    "Describe the matching expression and acting in the shot prose, with clear subject ownership. ",
    "This inline vocal-delivery policy applies only to generated built-in speech; supplied audio keeps its existing vocal delivery unchanged."
);

const TAIL: &str = concat!(
    "Interpret the lyric's meaning, scene action, and the requested emotion together. Explicit scene ",
    "emotion input takes priority over the preset; Custom facial text such as Angry is also an emotion ",
    "request. Choose fitting bracket descriptors such as [angry, singing] and write specific visible ",
    "expressions and acting that suit THIS lyric, rather than copying a stock phrase or the raw input. ",
    "The requested emotion remains authoritative: a sad lyric with Angry can be bitter or confrontational. ",
    "For 'start happy, then end sad', stage a readable progression with the appropriate tags at each ",
    "phase; never flatten the request into one simultaneous [happy, sad] state. ",
    "Keep each singer's expressions attached to their assigned cue and subject. Instrumental/B-roll ",
    "shots may have visual emotion but never singing tags or vocal performance. Scenes without people ",
    "use atmosphere and physical visuals rather than inventing a face. ",
    "For speaking scenes use speech emotion descriptors instead of singing descriptors. ",
    "For supplied audio, describe visible acting and preserve the existing vocals; do not request ",
    "changes to the vocal delivery. ",
    "For supplied-audio singing, never describe mouth, lip, or jaw movement, even for one continuous shot. ",
    "Use the assigned singing cue once and only during audible vocals. When lyric omission is enabled, keep the lyrics as ",
    "interpretation context only: use [emotion, singing] next to performance synchronized with <Audio 1>, ",
    "without lyric quotations or <d> tags. When lyrics are included in a single H3 shot, retain the exact ",
    "words and language descriptor and add emotion descriptors inside its cue, for example ",
    "<d>[English, angry, singing] exact supplied words.</d>. Choose the actual lyric language. ",
    "Write the emotion tags and acting inside the shot prose; never output this instruction block."
);

/// Ask the LLM to choose tags and acting from the lyrics and the author's direction.
pub fn emotion_expression_instruction(payload: &Obj) -> String {
    let mode = {
        let m = field(payload, "performance_mode");
        if m.is_empty() { field(payload, "video_type") } else { m }
    };
    let native_speech = mode == "speaking" && payload.get("audio_mode").and_then(Value::as_str) == Some("built_in_audio");
    let speech = if native_speech { SPEECH } else { "" };
    if !has_emotion_expression_input(payload) {
        return speech.to_string();
    }
    let facial = payload.get("facial_performance");
    let preset = {
        let f = text_or_empty(facial);
        if facial.is_some_and(truthy) { f } else { "natural".to_string() }.replace('_', " ")
    };
    let custom = if facial.is_none_or(|f| !truthy(f)) || facial.and_then(Value::as_str) == Some("custom") { field(payload, "facial_performance_custom") } else { String::new() };
    let tags = field(payload, "emotion_expression_tags");
    let lyric = field(payload, "lyric_text");
    format!(
        "EMOTION / EXPRESSION — AUTHOR'S ACTING DIRECTION:\nFacial preset: {preset}\nCustom facial direction: {}\nScene emotion/expression input: {}\nLyrics available for interpretation:\n{}\n\n{TAIL}{speech}",
        if custom.is_empty() { "(none)" } else { &custom },
        if tags.is_empty() { "(inherit facial direction)" } else { &tags },
        if lyric.is_empty() { "(no vocal line assigned)" } else { &lyric },
    )
}
