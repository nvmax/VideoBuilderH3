//! Timed cue maps: per-shot singer assignments and their delivery marks. Port of the cue part of
//! `minimax/shot_prompt.py`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::plan::{exact_duration, FALLBACK_SHOT};
use super::rx::{group, is_match, rx, split_sentences, sub, sub_with};
use super::text::normalize_description;
use crate::pyval::{collapse_ws, fmt_g, round_to, text_or_empty, to_float, truthy, Obj};

const CUE_DELIVERY_RULE: &str = concat!(
    "Write each vocal cue as <d>[Language, emotion, emotion, singing] exact words</d>: choose two specific emotions that fit this line's meaning, the story beat and the song, ",
    "wrap the 1–3 words the singer leans on (a rhyme word, a key verb) in <i>...</i>, and put <breath> between two lines where the singer breathes. ",
    "Keep every word unchanged and in order, and follow the cue with that singer's concrete facial acting."
);

/// One row of a scene's timed cue map, scene relative.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CueRow {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub singer_id: String,
    #[serde(default)]
    pub singer_name: String,
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub vocal_start: Option<f64>,
    #[serde(default)]
    pub vocal_end: Option<f64>,
    #[serde(default)]
    pub action_note: String,
    /// The singer's subject label, set by the caller before the row goes into a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn label_or_singer(cue: &CueRow) -> String {
    cue.label.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| "The singer".to_string())
}

/// The emotion header with the language first, else `English`.
pub fn cue_header(header: Option<&Value>) -> String {
    let language = rx!(
        r"(?i)^(?:english|spanish|french|german|italian|portuguese|dutch|swedish|norwegian|danish|finnish|polish|czech|russian|ukrainian|greek|turkish|arabic|hebrew|hindi|bengali|urdu|japanese|korean|chinese|mandarin|cantonese|thai|vietnamese|indonesian|malay|tagalog|filipino)$"
    );
    let source = text_or_empty(header);
    let mut items: Vec<String> = source
        .split(',')
        .map(|item| sub(rx!(r"[^\w '\-]"), item, "").trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if items.is_empty() {
        return "English".to_string();
    }
    if !is_match(language, &items[0]) {
        items.insert(0, "English".to_string());
    }
    items.truncate(5);
    items.join(", ")
}

fn has_word(token: &str) -> bool {
    token.chars().any(char::is_alphanumeric)
}

fn word_key(token: &str) -> String {
    token
        .to_lowercase()
        .replace('’', "'")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'')
        .collect()
}

struct Mark {
    key: String,
    stress: bool,
    breath: bool,
}

/// The cue's own words with the LLM's `<i>` stress and `<breath>` marks, when they line up word for word.
pub fn cue_delivery(cue_text: &str, llm_body: &str) -> String {
    let canonical: Vec<&str> = cue_text.split_whitespace().collect();
    let spaced = sub(rx!(r"(?i)<\s*(/?i|breath)\s*>"), llm_body, " <$1> ");
    let (mut marks, mut italic, mut breath): (Vec<Mark>, bool, bool) = (Vec::new(), false, false);
    for token in spaced.split_whitespace() {
        let tag = rx!(r"(?i)^<(/?i|breath)>$");
        let name = tag
            .captures(token)
            .ok()
            .flatten()
            .map(|c| group(&c, 1).to_lowercase())
            .unwrap_or_default();
        match name.as_str() {
            "i" => italic = true,
            "/i" => italic = false,
            "breath" => breath = true,
            _ => {
                let stripped = sub(rx!(r"<[^>]*>"), token, "");
                if has_word(&stripped) {
                    marks.push(Mark { key: word_key(&stripped), stress: italic, breath });
                    breath = false;
                }
            }
        }
    }
    let words: Vec<&str> = canonical.iter().copied().filter(|t| has_word(t)).collect();
    let joined = canonical.join(" ");
    if canonical.is_empty()
        || marks.len() != words.len()
        || words.iter().zip(&marks).any(|(t, m)| word_key(t) != m.key)
    {
        return joined;
    }
    let stressed = marks.iter().filter(|m| m.stress).count();
    let use_stress = stressed > 0 && stressed <= 4;
    let mut out: Vec<String> = Vec::new();
    let mut word_index: isize = -1;
    for (index, token) in canonical.iter().enumerate() {
        if !has_word(token) {
            out.push(token.to_string());
            continue;
        }
        word_index += 1;
        let mark = &marks[word_index as usize];
        let mut text = token.to_string();
        if use_stress && mark.stress {
            let is_word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '\'' || c == '’';
            let end = token.char_indices().filter(|(_, c)| is_word_char(*c)).map(|(i, c)| i + c.len_utf8()).next_back();
            let (word, tail) = end.map_or((*token, ""), |e| token.split_at(e));
            text = format!("<i>{word}</i>{tail}");
        }
        out.push(format!("{}{}", if mark.breath && index > 0 { "<breath> " } else { "" }, text));
    }
    sub(rx!(r"</i>(\s+)<i>"), &out.join(" "), "$1")
}

/// The scene's timed cue map (`lyric_performance_mode` `cue_map`, every row with a start). Empty otherwise.
pub fn timed_cue_rows(segment: &Obj) -> Vec<CueRow> {
    let mode = match segment.get("lyric_performance_mode") {
        Some(v) if truthy(v) => text_or_empty(Some(v)),
        _ => "together".to_string(),
    };
    if mode != "cue_map" {
        return vec![];
    }
    let number = |cue: &Obj, key: &str| -> Option<f64> {
        let value = cue.get(key)?;
        if value.is_null() || value.as_str() == Some("") {
            return None;
        }
        to_float(value).map(|f| f.max(0.0))
    };
    let mut rows: Vec<CueRow> = Vec::new();
    let entries = segment.get("lyric_cue_map").filter(|v| truthy(v)).and_then(Value::as_array);
    for cue in entries.into_iter().flatten().filter_map(Value::as_object) {
        let kind = if text_or_empty(cue.get("type")).trim() == "instrumental" { "instrumental" } else { "vocal" };
        let text = collapse_ws(&text_or_empty(cue.get("text")));
        if kind == "vocal" && text.is_empty() {
            continue;
        }
        let trimmed = |k: &str| text_or_empty(cue.get(k)).trim().to_string();
        rows.push(CueRow {
            kind: kind.to_string(),
            text,
            singer_id: trimmed("singer_id"),
            singer_name: trimmed("singer_name"),
            start: number(cue, "start"),
            vocal_start: if kind == "vocal" { number(cue, "vocal_start") } else { None },
            vocal_end: if kind == "vocal" { number(cue, "vocal_end") } else { None },
            action_note: trimmed("action_note"),
            label: None,
        });
    }
    if rows.is_empty() || rows.iter().any(|r| r.start.is_none()) {
        return vec![];
    }
    rows
}

/// The cue driven plan: a hard cut at every later cue's start.
pub fn cue_cut_plan(plan: &Obj, cues: &[CueRow]) -> Obj {
    let duration = exact_duration(plan);
    let mut cuts: Vec<f64> = Vec::new();
    for cue in cues.iter().skip(1) {
        let start = cue.start.unwrap_or(0.0);
        if 0.04 < start && start < duration - 0.04 {
            let rounded = round_to(start, 3);
            if !cuts.contains(&rounded) {
                cuts.push(rounded);
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    if cues.len() < 2 || cuts.is_empty() {
        return plan.clone();
    }
    let mut out = plan.clone();
    out.insert("cut_times_seconds".into(), json!(cuts));
    out.insert("cut_count".into(), json!(cuts.len()));
    out.insert("shot_count".into(), json!(cuts.len() + 1));
    out.insert("continuous_shot".into(), json!(false));
    out.insert("cue_driven".into(), json!(true));
    out
}

/// A cue's words with a closing stop and a capital first letter.
pub fn cue_text(text: &str) -> String {
    let mut clean = text.trim().to_string();
    let plain = sub(rx!(r"<[^>]+>"), &clean, "");
    if !clean.is_empty() && !is_match(rx!(r#"[.!?…]["')\]]?$"#), plain.trim()) {
        clean.push('.');
    }
    sub_with(rx!(r#"^(\s*["'“‘(]*)([a-z])"#), &clean, |c| format!("{}{}", group(c, 1), group(c, 2).to_uppercase()))
}

fn cue_window(cue: &CueRow) -> String {
    match (cue.vocal_start, cue.vocal_end) {
        (Some(start), Some(end)) if end > start => {
            format!(" from {} s to {} s", fmt_g(round_to(start, 2)), fmt_g(round_to(end, 2)))
        }
        _ => String::new(),
    }
}

/// The timed singer and lyric shot contract. Each row needs `label`, the singer's subject label.
pub fn cue_shot_contract(cues: &[CueRow]) -> String {
    let mut lines = vec!["Timed singer/lyric shot contract — authoritative:".to_string()];
    for (index, cue) in cues.iter().enumerate() {
        if cue.kind == "instrumental" {
            let note =
                if cue.action_note.is_empty() { String::new() } else { format!(" Visual action note: {}", cue.action_note) };
            lines.push(format!("[Shot {}]: use only the assigned visual action and camera direction.{note}", index + 1));
            continue;
        }
        let window = cue_window(cue);
        let sync = if window.is_empty() { String::new() } else { format!(" The singer sings{window}.") };
        lines.push(format!(
            "[Shot {}]: {} is the only performer singing <d>[English] {}</d> from <Audio 1>.{} Other visible performers remain silent, mouth closed or naturally reacting.",
            index + 1,
            label_or_singer(cue),
            cue_text(&cue.text),
            sync
        ));
    }
    lines.push(
        "Each listed cue is its own shot. Do not swap singers, merge lyric cues, or anticipate a later vocal cue. Apply vocal direction only to the assigned vocal row and its exact timing."
            .to_string(),
    );
    lines.push(CUE_DELIVERY_RULE.to_string());
    lines.join("\n")
}

/// An instrumental shot loses its vocal sentences. A vocal shot keeps the LLM's prose and ends with the one canonical
/// cue, carrying the LLM's emotion header and delivery marks.
pub fn enforce_cue_shot(description: &str, cue: &CueRow) -> String {
    let mut text = normalize_description(description);
    if cue.kind == "instrumental" {
        let marker = rx!(
            r"(?i)<d>|</d>|\b(?:sings?|singing|sung|speaks?|speaking|says?|saying|performs?|performing|dialogue|lip[ -]?sync(?:s|ing|ed)?|lyrics?|vocals?)\b"
        );
        let clean = split_sentences(&text)
            .into_iter()
            .filter(|s| !is_match(marker, s))
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let clean = clean.trim();
        return normalize_description(if clean.is_empty() { FALLBACK_SHOT } else { clean });
    }
    let canonical = cue_text(&cue.text);
    let llm_cue = rx!(r"(?i)<d>\s*(?:\[([^\]]*)\])?\s*([\s\S]*?)</d>").captures(&text).ok().flatten();
    let header_source = llm_cue.as_ref().and_then(|c| c.get(1)).map(|m| Value::String(m.as_str().to_string()));
    let header = cue_header(header_source.as_ref());
    let body = llm_cue.as_ref().map_or(String::new(), |c| group(c, 2));
    let delivered = cue_delivery(&canonical, &body);
    text = sub(
        rx!(r"(?i)<Subject\s+\d+>[^.\n]*?is the only visible performer singing[\s\S]*?every other visible performer remains silent\.?"),
        &text,
        "",
    );
    let variants: BTreeSet<String> = [cue.text.trim().to_string(), canonical.trim_end_matches(['.', '!', '?', '…']).trim().to_string()]
        .into_iter()
        .filter(|v| !v.is_empty())
        .collect();
    let mut ordered: Vec<String> = variants.into_iter().collect();
    ordered.sort_by_key(|v| std::cmp::Reverse(v.chars().count()));
    for variant in ordered {
        let pattern = format!(r#"(?i)[“"']\s*{}[.!?…]*\s*[”"']"#, fancy_regex::escape(&variant));
        if let Ok(re) = fancy_regex::Regex::new(&pattern) {
            text = re.replace_all(&text, fancy_regex::NoExpand("the assigned vocal cue")).into_owned();
        }
    }
    let mut clean = sub(rx!(r"(?i)<d>[\s\S]*?</d>"), &text, "the assigned vocal cue");
    clean = sub(rx!(r"\s+([,.;!?])"), &clean, "$1");
    clean = sub(rx!(r"\s{2,}"), &clean, " ").trim().to_string();
    clean = sub(rx!(r"[,;:]\s*([.!?…])"), &clean, "$1").trim_end_matches(['.', '!', '?', '…', ' ']).to_string();
    if clean.is_empty() {
        clean = FALLBACK_SHOT.trim_end_matches('.').to_string();
    }
    let contract = format!(
        "{} is the only visible performer singing, in sync with <Audio 1>{}, <d>[{}] {}</d>; every other visible performer remains silent.",
        label_or_singer(cue),
        cue_window(cue),
        header,
        delivered
    );
    normalize_description(&format!("{clean}. {contract}"))
}
