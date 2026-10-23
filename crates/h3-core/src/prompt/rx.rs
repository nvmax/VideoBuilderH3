//! Small regex helpers over `fancy-regex` (the pack's patterns use lookahead and lookbehind). Patterns are fixed
//! literals, so a bad one is a programming error caught by the tests. A runtime backtrack failure counts as no match.

use fancy_regex::{Captures, Regex};

/// A compiled pattern, built once per call site.
macro_rules! rx {
    ($pattern:expr) => {{
        static RE: std::sync::LazyLock<fancy_regex::Regex> =
            std::sync::LazyLock::new(|| fancy_regex::Regex::new($pattern).expect("valid pattern"));
        &*RE
    }};
}
pub(crate) use rx;

pub fn is_match(re: &Regex, text: &str) -> bool {
    re.is_match(text).unwrap_or(false)
}

/// `re.sub(pattern, replacement, text)` with `$1` style groups.
pub fn sub(re: &Regex, text: &str, replacement: &str) -> String {
    re.replace_all(text, replacement).into_owned()
}

/// `re.sub(pattern, lambda m: ..., text)`.
pub fn sub_with(re: &Regex, text: &str, f: impl Fn(&Captures) -> String) -> String {
    re.replace_all(text, |caps: &Captures| f(caps)).into_owned()
}

/// Only the first match, like `re.sub(..., count=1)`.
pub fn sub_first(re: &Regex, text: &str, replacement: &str) -> String {
    re.replace(text, replacement).into_owned()
}

/// Whole text of every match, like `re.findall` for a pattern without groups.
pub fn find_all(re: &Regex, text: &str) -> Vec<String> {
    re.find_iter(text).filter_map(Result::ok).map(|m| m.as_str().to_string()).collect()
}

/// Group `n` of every match, like `re.findall` for a pattern with one group.
pub fn find_all_group(re: &Regex, text: &str, group: usize) -> Vec<String> {
    re.captures_iter(text)
        .filter_map(Result::ok)
        .map(|c| c.get(group).map_or(String::new(), |m| m.as_str().to_string()))
        .collect()
}

pub fn group(caps: &Captures, n: usize) -> String {
    caps.get(n).map_or(String::new(), |m| m.as_str().to_string())
}

/// Sentences the way the pack splits them: `[^.!?…]+[.!?…]+` runs, and a trailing run without a stop.
pub fn split_sentences(text: &str) -> Vec<String> {
    find_all(rx!(r"[^.!?…]+[.!?…]+|[^.!?…]+$"), text)
}

/// Upper case the first lowercase ASCII letter after optional space and opening quotes.
pub fn clean_sentence_start(text: &str) -> String {
    sub_with(rx!(r#"^(\s*["'“‘(]*)([a-z])"#), text, |c| format!("{}{}", group(c, 1), group(c, 2).to_uppercase()))
}
