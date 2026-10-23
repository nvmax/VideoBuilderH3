//! Port of `strip_singing_articulation` in `minimax/lyric_free_performance.py`: with supplied audio the prompt never
//! describes mouth, lip or jaw movement, only action, emotion and the quoted words.

use super::rx::{rx, sub, sub_with};

/// Split at whitespace that follows `.`, `!`, `?` or an ellipsis, like `re.split(r"(?<=[.!?…])\s+", text)`.
fn split_sentences_keep(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut last = 0;
    for m in rx!(r"(?<=[.!?…])\s+").find_iter(text).flatten() {
        out.push(text[last..m.start()].to_string());
        last = m.end();
    }
    out.push(text[last..].to_string());
    out
}

/// Split at `,` or `;` and keep the separator, like `re.split(r"([,;])\s*", text)`: text, separator, text, ...
fn split_clauses(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut last = 0;
    for caps in rx!(r"([,;])\s*").captures_iter(text).flatten() {
        let whole = caps.get(0).expect("match");
        out.push(text[last..whole.start()].to_string());
        out.push(caps.get(1).map_or(String::new(), |g| g.as_str().to_string()));
        last = whole.end();
    }
    out.push(text[last..].to_string());
    out
}

/// Remove oral movement directions while keeping action, emotion and quoted words.
pub fn strip_singing_articulation(text: &str) -> String {
    let saved = std::cell::RefCell::new(Vec::<String>::new());
    let protect = rx!(
        r#"(?i)<d>[\s\S]*?</d>|“[^”]*”|"[^"]*"|\[[^\]]*\]|\blips\s+(?:(?:stay|stays|staying|remain|remains|remaining|still|kept|firmly|tightly)\s+){0,2}pressed\s+(?:together|shut|closed)\b"#
    );
    let mut clean = sub_with(protect, text, |caps| {
        let mut saved = saved.borrow_mut();
        saved.push(caps.get(0).map_or("", |m| m.as_str()).to_string());
        format!("VRGDGPROTECTED{}TOKEN", saved.len() - 1)
    });
    clean = sub(rx!(r"(\d)\.(?=\d)"), &clean, "${1}VRGDGDECIMALTOKEN");
    clean = sub(rx!(r"(?i)\bwith\s+(?:natural\s+)?(?:mouth|lips?|jaw)\b[^,;.!?]*,\s*"), &clean, "with ");
    let mut result: Vec<String> = Vec::new();
    for sentence in split_sentences_keep(&clean) {
        let parts = split_clauses(&sentence);
        let mut rebuilt = String::new();
        for i in (0..parts.len()).step_by(2) {
            let clause = sub(rx!(r"(?i)\s+(?:with|and|while)\s+(?:(?:her|his|their|natural)\s+)*(?:mouth|lips?|jaw)\b[^.!?]*"), &parts[i], "");
            let clause = clause.trim();
            if !clause.is_empty() && !rx!(r"(?i)\b(?:mouth\w*|lips?|jaw)\b").is_match(clause).unwrap_or(false) {
                if !rebuilt.is_empty() {
                    let sep = if parts[i - 1].is_empty() { ",".to_string() } else { parts[i - 1].clone() };
                    rebuilt.push_str(&format!("{sep} "));
                }
                rebuilt.push_str(clause);
            }
        }
        let mut rebuilt = sub(rx!(r"[,;]\s*([.!?])"), &rebuilt, "$1").trim_end_matches([' ', ',', ';']).to_string();
        let ends = |s: &str| rx!(r"[.!?…]$").is_match(s).unwrap_or(false);
        if !rebuilt.is_empty() && ends(&sentence) && !ends(&rebuilt) {
            rebuilt.push(sentence.chars().last().unwrap_or('.'));
        }
        result.push(rebuilt);
    }
    let joined = result.iter().filter(|r| !r.is_empty()).cloned().collect::<Vec<_>>().join(" ").replace("VRGDGDECIMALTOKEN", ".");
    let saved = saved.into_inner();
    let restored = sub_with(rx!(r"VRGDGPROTECTED(\d+)TOKEN"), &joined, |caps| {
        caps.get(1).and_then(|m| m.as_str().parse::<usize>().ok()).and_then(|i| saved.get(i).cloned()).unwrap_or_default()
    });
    sub(rx!(r"\s+"), &restored, " ").trim().to_string()
}
