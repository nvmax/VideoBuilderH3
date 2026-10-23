//! The "omit lyrics from video prompts" policy for custom audio: the prompt describes passionate singing in sync with the
//! supplied song and never quotes words or mouth movement. Port of `minimax/lyric_free_performance.py` and
//! `scene_cut_plan` in `minimax/prompt_assembly.py`.

use fancy_regex::{escape, Regex};
use serde_json::Value;

use super::cues::{cue_cut_plan, timed_cue_rows};
use super::emotion::{emotion_expression_input, has_emotion_expression_input};
use super::frame::reference_labels;
use super::performance::strip_singing_articulation;
use super::plan::{apply_cut_style, cut_times};
use super::rx::{find_all, group, is_match, rx, sub, sub_with};
use crate::lyric_scenes::is_instrumental_lyric_text;
use crate::pyval::{py_str, to_float, truthy, Obj};
use crate::scene::ordered_reference_items;
use crate::settings::settings_for_scene;
use crate::style::storyboard_cut_plan_for_duration;

/// Apply only to singing with supplied audio.
pub fn enabled(value: bool, performance_mode: &str, audio_mode: &str) -> bool {
    value && performance_mode == "singing" && audio_mode != "built_in_audio"
}

/// `str(a or b or "")`: the first value that is not empty, as Python prints it.
fn first_text(values: &[Option<&Value>]) -> String {
    values.iter().copied().flatten().find(|v| truthy(v)).map(|v| py_str(Some(v))).unwrap_or_default()
}

fn rx_new(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

/// `re.split(pattern, text)` for a pattern without groups: the pieces between matches, empty ones kept.
fn py_split(re: &Regex, text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut last = 0;
    for m in re.find_iter(text).flatten() {
        pieces.push(text[last..m.start()].to_string());
        last = m.end();
    }
    pieces.push(text[last..].to_string());
    pieces
}

fn sentences_of(text: &str) -> Vec<String> {
    py_split(rx!(r"(?<=[.!?])\s+"), text)
}

fn protect_decimals(value: &str) -> String {
    sub(rx!(r"(\d)\.(?=\d)"), value, "${1}DECIMAL")
}

/// The text from char offset `from`.
fn tail_from(text: &str, from: usize) -> String {
    text.chars().skip(from).collect()
}

fn head_to(text: &str, to: usize) -> String {
    text.chars().take(to).collect()
}

fn char_offset(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

/// Merge named or unambiguously pronoun-owned singing into the description, keeping tags and action. Returns the
/// description and what is left of the contract.
pub fn merge_contract(description: &str, contract: &str) -> (String, String) {
    let protected_contract = protect_decimals(contract);
    let mut parts = sentences_of(&protected_contract).into_iter();
    let singing = parts.next().unwrap_or_default();
    let rest: Vec<String> = parts.collect();
    let Some(actor) = rx!(r"(?i)^(.*?) sings\b").captures(&singing).ok().flatten().map(|c| group(&c, 1)) else {
        return (description.to_string(), contract.to_string());
    };
    let suffix = r"(?:\s*\[[^\]]+\])*\s+sings(?:\s+[a-z]+ly|\s+with passion)?\b";
    let owned = rx_new(&format!("(?i){}{suffix}", escape(&actor)));
    let pronoun = rx_new(&format!(r"(?i)\b(?:she|he|they){suffix}"));
    let mut sentences = sentences_of(&protect_decimals(description));
    let requested_time = rx!(r" during (.+)\.$").captures(&singing).ok().flatten().map(|c| group(&c, 1));
    let mut preceding = String::new();
    let mut found: Option<(usize, usize)> = None;
    for (i, sentence) in sentences.iter().enumerate() {
        let visible = sub_with(rx!(r#"(?i)<d>[\s\S]*?</d>|“[^”]*”|"[^"]*""#), sentence, |m| " ".repeat(group(m, 0).chars().count()));
        let named = owned.find(&visible).ok().flatten().map(|m| (m.start(), m.end()));
        let inferred = pronoun.find(&visible).ok().flatten().map(|m| (m.start(), m.end()));
        let before = format!("{preceding}{}", inferred.map_or(String::new(), |(s, _)| visible[..s].to_string()));
        let labels: std::collections::BTreeSet<String> = find_all(rx!(r"(?i)<Subject\s+\d+>"), &before).into_iter().collect();
        let matched = named.or(if labels.len() == 1 && labels.contains(&actor) { inferred } else { None });
        let existing_time = rx!(r" during (\d+(?:DECIMAL\d+)?s\s*[–—-]\s*\d+(?:DECIMAL\d+)?s)").captures(sentence).ok().flatten().map(|c| group(&c, 1));
        if let Some((_, end)) = matched {
            let same_time = match (&requested_time, &existing_time) {
                (Some(r), Some(e)) => r == e,
                _ => true,
            };
            if same_time {
                found = Some((i, char_offset(&visible, end)));
                break;
            }
        }
        preceding.push_str(&visible);
        preceding.push(' ');
    }
    let Some((index, end)) = found else { return (description.to_string(), contract.to_string()) };
    let sync = sub(rx!(r"(?i)^.*? sings(?: with passion)?\s*"), &singing, "").trim_end_matches('.').to_string();
    let mut tail = tail_from(&sentences[index], end);
    tail = sub(rx!(r"(?i)^(\s*(?:\[[^\]]+\]\s*)*)in sync with\s+(?:the\s+)?(?:audible\s+)?(?:vocals(?:\s+in)?\s*(?:<Audio 1>)?|<Audio 1>)"), &tail, "${1}");
    tail = sub(rx!(r"(?i)^(\s*(?:\[[^\]]+\]\s*)*)during\s+\d+(?:DECIMAL\d+)?s\s*[–—-]\s*\d+(?:DECIMAL\d+)?s"), &tail, "${1}");
    tail = sub(rx!(r"^\s+([.,;!?])"), &tail, "${1}");
    tail = sub(rx!(r"^\s+"), &tail, " ");
    sentences[index] = format!("{} {sync}{tail}", head_to(&sentences[index], end));
    let canonical = |v: &str| sub(rx!(r"\s+"), &sub(rx!(r"(?i) sings with passion"), v, " sings"), " ").trim().to_string();
    let target = canonical(&singing);
    let kept: Vec<&String> = sentences.iter().enumerate().filter(|(i, s)| *i == index || canonical(s) != target).map(|(_, s)| s).collect();
    (kept.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ").replace("DECIMAL", "."), rest.join(" ").replace("DECIMAL", "."))
}

/// Remove stale lyric and vocal movement sentences before adding timed direction.
pub fn clean_description(text: &str, lyrics: &[String], preserve_performance: bool) -> String {
    let mut clean = sub(rx!(r"(?i)<d>[\s\S]*?</d>"), text, "");
    let mut ordered: Vec<&String> = lyrics.iter().filter(|l| !l.is_empty()).collect();
    ordered.sort_by_key(|a| std::cmp::Reverse(a.chars().count()));
    for lyric in ordered {
        let parts: Vec<String> = lyric.split_whitespace().map(escape).map(|p| p.into_owned()).collect();
        if parts.is_empty() {
            continue;
        }
        let pattern = format!("(?i){}", parts.join(r"\s+"));
        clean = rx_new(&pattern).replace_all(&clean, "").into_owned();
    }
    clean = sub(rx!(r#"["“”']\s*["“”']"#), &clean, "");
    let protected = sub(rx!(r"(\d)\.(?=\d)"), &clean, "${1}VRGDGDECIMALTOKEN");
    if preserve_performance {
        let stripped = strip_singing_articulation(&protected);
        return sub(rx!(r"\s+"), &stripped, " ").replace("VRGDGDECIMALTOKEN", ".").trim().to_string();
    }
    let vocal = rx!(r"(?i)\b(?:sing\w*|sang|sung|lip[ -]?sync\w*|lyrics?|vocals?|dialogue|mouth\w*|lips?|jaw)\b");
    let sentences = find_all(rx!(r"[^.!?…]+[.!?…]+|[^.!?…]+$"), &protected);
    let kept: Vec<String> = sentences.into_iter().filter(|s| !is_match(vocal, s)).collect();
    sub(rx!(r"\s+"), &kept.join(" "), " ").replace("VRGDGDECIMALTOKEN", ".").trim().to_string()
}

/// One audio-based singing direction without explicit articulation.
pub fn shot_direction(performer: &str, timing: &str, include_expression: bool) -> String {
    let mut out = format!("{performer} sings with passion in sync with the vocals in <Audio 1>");
    if !timing.is_empty() {
        out.push_str(&format!(" during {timing}"));
    }
    out.push('.');
    if include_expression {
        out.push_str(&format!(" {performer}'s engaged eyes and expressive brows convey the song's intensity."));
    }
    out
}

fn facial_preset(key: &str) -> &'static str {
    match key {
        "" => "Use natural expressive facial performance: engaged eyes, subtle natural eye movement, active brows, subtle cheek and jaw movement, visible emotion that fits the lyric or scene, and occasional natural blinking.",
        "pop_polished" => "Use polished pop-star facial performance: bright eyes, subtle natural eye movement, direct camera gaze, soft confident smile, playful smirk, relaxed brows, slight head tilts, lips slightly parted while singing, charming camera-ready expression, and occasional natural blinking.",
        "pop_flirty" => "Use playful pop facial performance: flirty smile, coy glance, subtle natural eye movement, light pout, glossy pout, raised brows, charming direct gaze, playful smirk, subtle head tilt, lips slightly parted while singing, and occasional natural blinking.",
        "love_tender" => "Use tender love-song facial performance: softened eyes, subtle natural eye movement, warm smile, affectionate gaze, raised inner brows, gentle head tilt, relaxed cheeks, subtle vulnerable emotion, and occasional natural blinking.",
        "sad_wounded" => "Use wounded sad-song facial performance: lowered gaze, heavy or watery eyes, subtle natural eye movement, raised inner brows, pinched brows, downturned mouth, trembling lips or chin when appropriate, defeated expression, and occasional natural blinking.",
        "happy_joyful" => "Use joyful facial performance: bright smile, smiling eyes, subtle natural eye movement, raised cheeks, delighted expression, playful gaze, lifted mouth corners, relaxed brows, head tilt with smile, and occasional natural blinking.",
        "rock_intense" => "Use intense rock facial performance: focused stare, subtle natural eye movement, furrowed brows, defiant smirk, clenched jaw, gritty emotional strain, sharp eye contact, forceful singing expression, and occasional natural blinking.",
        "metal_rage" => "Use aggressive heavy metal facial performance: fierce stare, subtle natural eye movement, furrowed brows, wild eyes, clenched jaw, snarling mouth shapes during vocals, bared teeth on powerful notes, flared nostrils, strained neck intensity, raw emotional scream expression, and occasional natural blinking.",
        "rap_high_intensity" => "Use high-intensity rap facial performance: intense stare, sharp eye contact, subtle natural eye movement, furrowed brows, animated eyes, confident smirk, tight jaw, mouth open mid-verse, fast-moving mouth during delivery, challenging look, victory grin, and occasional natural blinking.",
        _ => "",
    }
}

/// The selected facial emotion without vocal articulation.
pub fn facial_text(segment: &Obj, session: &Obj) -> String {
    let key = first_text(&[segment.get("facial_performance"), session.get("default_facial_performance")]);
    if key == "off" {
        return String::new();
    }
    let custom = first_text(&[segment.get("facial_performance_custom"), session.get("default_facial_performance_custom")]);
    let text = if key == "custom" { custom } else { [facial_preset(&key).to_string(), custom].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ") };
    let banned = rx!(r"(?i)\b(?:mouth\w*|lips?|jaw|sing\w*|sang|sung|vocals?|lyrics?|teeth|notes)\b");
    let clauses: Vec<String> = py_split(rx!(r"[,.;]"), &text).into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty() && !is_match(banned, c)).collect();
    let joined = sub(rx!(r",\s*and\s+"), &clauses.join(", "), ", ");
    if clauses.is_empty() { joined } else { format!("{joined}.") }
}

fn performance_of(session: &Obj, segment: &Obj) -> String {
    let value = first_text(&[segment.get("performance_mode"), session.get("video_type")]);
    if value.is_empty() { "singing".into() } else { value }
}

/// Whether the project option and the scene's audio mode ask for lyric-free prompts.
pub fn scene_enabled(session: &Obj, segment: &Obj) -> bool {
    let settings = settings_for_scene(&Value::Object(session.clone()), Some(&Value::Object(segment.clone())));
    let audio_mode = match settings.get("audio_mode") {
        Some(v) => py_str(Some(v)),
        None => "input_audio".to_string(),
    };
    enabled(session.get("omit_lyrics_from_video_prompts").is_some_and(truthy), &performance_of(session, segment), &audio_mode)
}

/// Renderer labels by singer name or id, in the order they were found.
#[derive(Debug, Clone, Default)]
pub struct Labels(pub Vec<(String, String)>);

impl Labels {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn set(&mut self, key: String, value: String) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }
}

/// The performer label for the selected singers and the label of every subject.
pub fn scene_performers(session: &Obj, segment: &Obj, mode: &str) -> (String, Labels) {
    let mut ordered: Vec<Value> = session.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
    ordered.sort_by(|a, b| to_float(a.get("start").unwrap_or(&Value::Null)).unwrap_or(0.0).total_cmp(&to_float(b.get("start").unwrap_or(&Value::Null)).unwrap_or(0.0)));
    let index = ordered.iter().position(|s| s.get("id") == segment.get("id")).unwrap_or(0);
    let items = ordered_reference_items(&Value::Object(session.clone()), segment, mode, index);
    let mut labels = Labels::default();
    for item in reference_labels(&items) {
        if item.kind == "subject" {
            labels.set(item.name, item.label);
        }
    }
    for item in &items {
        let name = first_text(&[item.get("label"), item.get("name")]);
        let subject_id = first_text(&[item.get("source_id"), item.get("id")]);
        if !subject_id.is_empty() {
            if let Some(found) = labels.get(&name).map(str::to_string) {
                labels.set(subject_id, found);
            }
        }
    }
    let mut singers: Vec<String> = Vec::new();
    for name in segment.get("lyric_singers").and_then(Value::as_array).into_iter().flatten() {
        if let Some(label) = labels.get(&py_str(Some(name))) {
            if !singers.iter().any(|s| s == label) {
                singers.push(label.to_string());
            }
        }
    }
    let performer = if singers.is_empty() { labels.0.first().map_or("The assigned performer".to_string(), |(_, v)| v.clone()) } else { singers.join(" and ") };
    (performer, labels)
}

fn num_of(value: Option<&Value>) -> Option<f64> {
    value.filter(|v| !v.is_null()).and_then(to_float)
}

/// Singing assigned only to the vocal intervals that overlap shot `index` (`-1` for the whole scene).
pub fn directions(segment: &Obj, cut_plan: &Obj, performer: &str, index: i64, labels: &Labels) -> Vec<String> {
    let lyric = first_text(&[segment.get("lyric_text")]);
    if segment.get("no_character_present").is_some_and(truthy) || segment.get("lyric_no_lip_sync").is_some_and(truthy) || is_instrumental_lyric_text(&lyric) {
        return vec![];
    }
    let mut times = vec![0.0];
    times.extend(cut_times(cut_plan));
    let single = times.len() == 1;
    let cues: Vec<&Value> = if segment.get("lyric_performance_mode").and_then(Value::as_str) == Some("cue_map") { segment.get("lyric_cue_map").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default() } else { vec![] };
    let mut result = Vec::new();
    for (cue_index, cue) in cues.iter().enumerate() {
        if cue.get("type").and_then(Value::as_str) == Some("instrumental") {
            continue;
        }
        let pick = |primary: &str, fallback: &str| -> Option<&Value> { if cue.get(primary).is_some() { cue.get(primary) } else { cue.get(fallback) } };
        let mut start = pick("vocal_start", "start");
        let mut end = pick("vocal_end", "end");
        if start.is_none_or(Value::is_null) {
            start = cue.get("start");
        }
        if end.is_none_or(Value::is_null) {
            end = cue.get("end");
        }
        let (start_n, end_n) = (num_of(start), num_of(end));
        if index >= 0 {
            let i = index as usize;
            match (start_n, end_n) {
                (Some(s), Some(e)) => {
                    let shot_end = if i + 1 < times.len() { times[i + 1] } else { num_of(cut_plan.get("exact_duration_seconds")).unwrap_or(0.0) };
                    if s >= shot_end || e <= times[i] {
                        continue;
                    }
                }
                _ => {
                    if !single && cue_index != i {
                        continue;
                    }
                }
            }
        }
        let who = labels.get(&py_str(cue.get("singer_id"))).filter(|l| !l.is_empty()).or_else(|| labels.get(&py_str(cue.get("singer_name"))).filter(|l| !l.is_empty())).unwrap_or(performer);
        let timing = match (start.filter(|v| !v.is_null()), end.filter(|v| !v.is_null())) {
            (Some(s), Some(e)) => format!("{}s–{}s", py_str(Some(s)), py_str(Some(e))),
            _ => String::new(),
        };
        result.push(shot_direction(who, &timing, true));
    }
    if cues.is_empty() && !lyric.trim().is_empty() {
        result.push(shot_direction(performer, "", true));
    }
    result
}

fn split_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(str::to_string).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn lyric_strings(segment: &Obj) -> Vec<String> {
    let lyric = if segment.get("lyric_text").is_none_or(|v| !truthy(v)) { String::new() } else { py_str(segment.get("lyric_text")) };
    let mut lyrics = vec![lyric.clone()];
    lyrics.extend(split_lines(&lyric));
    for cue in segment.get("lyric_cue_map").and_then(Value::as_array).into_iter().flatten() {
        lyrics.push(if cue.get("text").is_none_or(|v| !truthy(v)) { String::new() } else { py_str(cue.get("text")) });
    }
    lyrics
}

/// The final safety net over the shot descriptions, independent of the LLM following the rules.
pub fn apply_shots(descriptions: &[String], segment: &Obj, cut_plan: &Obj, performer: &str, labels: &Labels, session: &Obj) -> Vec<String> {
    let lyrics = lyric_strings(segment);
    let interpret_emotion = has_emotion_expression_input(&emotion_expression_input(segment, session));
    let mut result = Vec::new();
    for (index, text) in descriptions.iter().enumerate() {
        let mut contracts = directions(segment, cut_plan, performer, index as i64, labels);
        let mut clean = clean_description(text, &lyrics, !contracts.is_empty() && interpret_emotion);
        if interpret_emotion {
            let expression = rx!(r"(?i) (?:[^.]+?'s )?engaged eyes and expressive brows convey the song's intensity\.");
            contracts = contracts.iter().map(|c| sub(expression, &c.replace("sings with passion", "sings"), "")).collect();
            let mut remaining = Vec::new();
            for contract in &contracts {
                let (merged, extra) = merge_contract(&clean, contract);
                clean = merged;
                remaining.push(extra);
            }
            contracts = remaining;
        }
        let mut parts: Vec<String> = vec![if clean.is_empty() { "The camera follows continuous physical action through the scene.".to_string() } else { clean }];
        parts.push(if interpret_emotion { String::new() } else { facial_text(segment, session) });
        parts.extend(contracts);
        result.push(parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" "));
    }
    result
}

/// `re.sub(r"[^\n.!?]*\b(?:mouth\w*|lips?|jaw)\b[^\n.!?]*[.!?]?", "", text, flags=re.I)`: drop every clause (the text
/// between `.`, `!`, `?` and line breaks) that mentions a mouth, lips or a jaw, with its closing `.`, `!` or `?`. Done by
/// hand because the pattern backtracks badly on a long task text.
fn remove_mouth_clauses(text: &str) -> String {
    let word = rx!(r"(?i)\b(?:mouth\w*|lips?|jaw)\b");
    let mut out = String::new();
    let mut clause = String::new();
    let flush = |clause: &mut String, closing: Option<char>, out: &mut String| {
        if is_match(word, clause) {
            // The clause goes, with its `.`, `!` or `?`. A line break stays.
            if closing == Some('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(clause);
            if let Some(c) = closing {
                out.push(c);
            }
        }
        clause.clear();
    };
    for c in text.chars() {
        if matches!(c, '\n' | '.' | '!' | '?') {
            flush(&mut clause, Some(c), &mut out);
        } else {
            clause.push(c);
        }
    }
    flush(&mut clause, None, &mut out);
    out
}

/// Replace the literal vocal contracts in the task with audio-based, timed performance instructions.
pub fn prompt_context(text: &str, segment: &Obj, cut_plan: &Obj, performer: &str, labels: &Labels, session: &Obj) -> String {
    let vocal_contract = rx!(r"(?i)^(?:MANDATORY VOCAL PERFORMANCE|Vocal performance|Exact lyric|Timed singer|Performer / vocal|Multi-performer rule|AUTHORITATIVE PERFORMER)");
    let kept: Vec<&str> = text.split("\n\n").filter(|p| !is_match(vocal_contract, p.trim())).collect();
    let mut clean = kept.join("\n\n");
    let mut lyrics: Vec<String> = lyric_strings(segment).into_iter().filter(|l| !l.is_empty()).collect();
    lyrics.sort_by_key(|a| std::cmp::Reverse(a.chars().count()));
    for lyric in lyrics {
        clean = clean.replace(&lyric, "[audio vocal cue]");
    }
    clean = remove_mouth_clauses(&clean);
    let rule = "Do not mention mouth, lip, or jaw movement anywhere in this prompt. Write each assigned singing cue once, only during its audible vocals in Audio 1.";
    format!(
        "{clean}\n\nLYRIC-FREE CUSTOM AUDIO — MANDATORY:\nDo not quote, invent, or add lyric lines or <d> tags. {rule} Instrumental intervals use only visual action and camera direction; do not mention singing in them. Preserve the supplied audio unchanged. Use selected facial direction through eyes, brows and expression.\n{}\n{}",
        facial_text(segment, session),
        directions(segment, cut_plan, performer, -1, labels).join("\n"),
    )
}

/// The scene's shot plan from its length and the project's saved cut frequency and cut style.
pub fn scene_cut_plan(segment: &Obj, session: &Obj) -> Obj {
    let defaults: Obj = session.get("builder_storyboard_defaults").and_then(Value::as_object).cloned().unwrap_or_default();
    let duration = (num_of(segment.get("end")).filter(|n| *n != 0.0).unwrap_or(0.0) - num_of(segment.get("start")).filter(|n| *n != 0.0).unwrap_or(0.0)).max(0.1);
    let frequency = defaults.get("minimax_h3_cut_frequency").filter(|v| truthy(v)).and_then(to_float).map_or(0, |f| f as i64);
    let mut plan = storyboard_cut_plan_for_duration(duration, frequency);
    if scene_enabled(session, segment) {
        if segment.get("location_continuous_shot").is_some_and(truthy) {
            plan = storyboard_cut_plan_for_duration(duration, 0);
        } else if segment.get("lyric_performance_mode").and_then(Value::as_str) == Some("cue_map") {
            let mut starts: Vec<f64> = Vec::new();
            for cue in segment.get("lyric_cue_map").and_then(Value::as_array).into_iter().flatten().skip(1) {
                if let Some(s) = num_of(cue.get("start")) {
                    if 0.04 < s && s < duration - 0.04 && !starts.contains(&s) {
                        starts.push(s);
                    }
                }
            }
            starts.sort_by(f64::total_cmp);
            if !starts.is_empty() {
                plan.insert("cut_times_seconds".into(), serde_json::json!(starts));
                plan.insert("cut_count".into(), serde_json::json!(starts.len()));
                plan.insert("shot_count".into(), serde_json::json!(starts.len() + 1));
                plan.insert("continuous_shot".into(), serde_json::json!(false));
                plan.insert("cue_driven".into(), serde_json::json!(true));
            }
        }
    } else if !segment.get("location_continuous_shot").is_some_and(truthy) {
        let settings = settings_for_scene(&Value::Object(session.clone()), Some(&Value::Object(segment.clone())));
        let singing = performance_of(session, segment) == "singing";
        let audio_mode = match settings.get("audio_mode") {
            Some(v) => py_str(Some(v)),
            None => "input_audio".to_string(),
        };
        if singing && audio_mode != "built_in_audio" {
            let cues = timed_cue_rows(segment);
            if !cues.is_empty() {
                plan = cue_cut_plan(&plan, &cues);
            }
        }
    }
    apply_cut_style(&plan, defaults.get("minimax_h3_cut_style"))
}
