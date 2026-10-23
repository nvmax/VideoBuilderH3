//! Checks and cleaning of model text. Port of `llm/output_checks.py` (repeat failures, unfilled templates, unusable
//! reference descriptions) and `_clean_visual_gemma_text` in `llm/text_cleaning.py`.

use std::collections::HashMap;

use fancy_regex::Regex;

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_all(re: &Regex, text: &str) -> Vec<String> {
    re.find_iter(text).flatten().map(|m| m.as_str().to_string()).collect()
}

/// `[\w']+` tokens with `_` and `'` trimmed from their ends.
fn word_tokens(sample: &str) -> Vec<String> {
    find_all(&rx(r"[\w']+"), sample).into_iter().map(|t| t.trim_matches(['_', '\'']).to_string()).filter(|t| !t.is_empty()).collect()
}

fn max_count<'a>(items: impl Iterator<Item = &'a str>) -> usize {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for item in items {
        *counts.entry(item).or_insert(0) += 1;
    }
    counts.values().copied().max().unwrap_or(0)
}

/// Text that is a model stuck in a loop, or leaking its thinking.
pub fn looks_like_repeat_failure(text: &str) -> bool {
    let sample = collapse(&text.to_lowercase());
    if sample.is_empty() {
        return false;
    }
    let compact = rx(r"[^a-z0-9_<>\-|]+").replace_all(&sample, "").into_owned();
    const MARKERS: [&str; 15] = [
        "completion-completion-completion", "thought-thought-thought", "de-facto-de-facto-de-facto", "de-fleshed", "cast-cast-cast", "prompt-cast-cast", "thoughtthoughtthought",
        "ownnessownnessownness", "nessnessnessness", "model_model_model", "modelmodelmodel", "end_anow", "thought_turn", "turn_turn", "<|channel>",
    ];
    if MARKERS.iter().any(|m| compact.contains(m) || sample.contains(m)) || compact.contains("<channel|>") || sample.contains("<channel|>") {
        return true;
    }
    let hit = |pattern: &str, on: &str| rx(pattern).is_match(on).unwrap_or(false);
    if hit(r"([a-z]{2,16})\1{5,}", &compact) || hit(r"(?:^|_)([a-z]{2,24})(?:_\1){5,}(?:_|$)", &compact) || hit(r"\b([a-zA-Z_]{3,})(?:[-\s]+\1){5,}\b", &sample) {
        return true;
    }
    let tokens = word_tokens(&sample);
    if tokens.len() >= 16 {
        let top = max_count(tokens.iter().map(String::as_str));
        if top >= 10 && top as f64 / tokens.len() as f64 >= 0.20 {
            return true;
        }
        for size in 2..=4usize {
            if tokens.len() < size * 4 {
                continue;
            }
            let phrases: Vec<String> = (0..=tokens.len() - size).map(|i| tokens[i..i + size].join(" ")).collect();
            if max_count(phrases.iter().map(String::as_str)) >= 8 {
                return true;
            }
        }
    }
    let words = find_all(&rx(r"[a-zA-Z_][a-zA-Z_']{2,}"), &sample);
    if words.len() < 18 {
        return false;
    }
    const COMMON: [&str; 9] = ["the", "and", "with", "that", "this", "from", "into", "while", "during"];
    let top = max_count(words.iter().map(String::as_str).filter(|w| !COMMON.contains(w)));
    if top >= 10 && top as f64 / words.len() as f64 >= 0.25 {
        return true;
    }
    let phrases: Vec<String> = (0..words.len() - 1).map(|i| format!("{} {}", words[i], words[i + 1])).collect();
    phrases.len() >= 12 && max_count(phrases.iter().map(String::as_str)) >= 6
}

/// A reply that kept the template's `[placeholders]`.
pub fn looks_like_unfilled_template(text: &str) -> bool {
    let sample = text.trim();
    if sample.is_empty() {
        return false;
    }
    if find_all(&rx(r"\[([^\]]{2,80})\]"), sample).len() >= 2 {
        return true;
    }
    const TERMS: [&str; 12] = ["subject", "setting", "environment", "time", "weather", "camera motion", "dynamic performance", "subject visibility", "framing", "reacts dynamically", "clothing", "hair"];
    let lowered = sample.to_lowercase();
    TERMS.iter().any(|t| lowered.contains(&format!("[{t}]"))) || rx(r"\[[^\]]*(?:subject|setting|environment|camera|motion|weather|lighting|dynamic|framing)[^\]]*\]").is_match(&lowered).unwrap_or(false)
}

/// A reference description that is empty, repeated, a template or too short to use.
pub fn looks_like_bad_reference_description(text: &str) -> bool {
    let sample = collapse(text);
    if sample.is_empty() || looks_like_repeat_failure(&sample) || looks_like_unfilled_template(&sample) {
        return true;
    }
    let tokens: Vec<String> = word_tokens(&sample.to_lowercase());
    if tokens.is_empty() {
        return true;
    }
    let unique: std::collections::HashSet<&String> = tokens.iter().collect();
    if tokens.len() < 6 && unique.len() <= 2 {
        return true;
    }
    let top = max_count(tokens.iter().map(String::as_str));
    if top >= 3 && top as f64 / tokens.len() as f64 >= 0.45 {
        return true;
    }
    for size in 1..=3usize {
        if tokens.len() < size * 3 {
            continue;
        }
        for index in 0..=tokens.len() - size * 3 {
            let chunk = &tokens[index..index + size];
            if (size..size * 3).step_by(size).all(|offset| &tokens[index + offset..index + offset + size] == chunk) {
                return true;
            }
        }
    }
    sample.chars().filter(|c| c.is_ascii_alphabetic()).count() < 18
}

/// A model's reply as one clean paragraph: no thinking, control tokens or leading labels.
pub fn clean_visual_text(text: &str) -> String {
    let mut cleaned = rx(r"(?is)<think>.*?</think>").replace_all(text.trim(), "").trim().to_string();
    let label = rx(r"(?i)^(?:Assistant|Answer|Final prompt)\s*:\s*");
    cleaned = label.replace(&cleaned, "").trim().to_string();
    let patterns: Vec<Regex> = [
        r"(?is)^\s*_?(?:user|assistant|model)?_?\s*(?:thought|analysis|reasoning)\s*[:=\-]?\s*",
        r"(?is)^\s*_?(?:start_of_)?turn\s*",
        r"(?is)^\s*<\|?start_of_turn\|?>\s*(?:model|assistant)?\s*",
        r"(?is)\s*<\|?end_of_turn\|?>\s*",
        r"(?is)_?\s*<\|channel>\s*(?:thought|analysis|reasoning)?\s*",
        r"(?is)_?\s*<\|?channel\|?>\s*(?:thought|analysis|reasoning)?\s*",
        r"(?is)_?\s*<channel\|>\s*(?:thought|analysis|reasoning)?\s*",
        r"(?is)^\s*<?/?end[_\-][a-z0-9_\-]*>?\s*",
        r"(?is)^\s*_?name\s*[:=]\s*",
        r"(?is)^\s*\d+\s*(?:thought|analysis|reasoning)\s*[:\-]?\s*",
        r"(?is)^\s*[-_]*\s*(?:thought|analysis|reasoning)\s*",
        r"(?is)^\s*(?:thought|analysis|reasoning)\s*[:\-]?\s*",
    ]
    .iter()
    .map(|p| rx(p))
    .collect();
    let mut previous = String::new();
    while !cleaned.is_empty() && previous != cleaned {
        previous = cleaned.clone();
        for pattern in &patterns {
            cleaned = pattern.replace_all(&cleaned, "").trim().to_string();
        }
    }
    cleaned = label.replace(&cleaned, "").trim().to_string();
    let paragraphs: Vec<String> = rx(r"\n\s*\n").split(&cleaned).flatten().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    paragraphs.into_iter().next().unwrap_or(cleaned)
}
