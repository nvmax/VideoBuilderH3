//! A hard wall that keeps characters who are not selected for a scene out of that scene's text. Port of `storyboard/cast_guard.py`.
//!
//! The story arc, story brief and neighbouring beats describe the whole song, so an LLM can carry a character into a scene that does not
//! include them, or invent an unnamed person ("a woman", "a stranger", "a crowd"). These helpers build a matcher for the characters left out
//! of a scene (names, generic nouns, and gendered pronouns when that cannot be confused with a cast member) plus a separate matcher for
//! invented people, and remove or flag any sentence that refers to them.

use std::collections::{BTreeSet, HashSet};

use fancy_regex::Regex;
use serde_json::Value;

use crate::pyval::text_or_empty;

const MALE_PRONOUNS: [&str; 4] = ["he", "him", "his", "himself"];
const FEMALE_PRONOUNS: [&str; 4] = ["she", "her", "hers", "herself"];
const MALE_NOUNS: [&str; 13] = ["man", "men", "boy", "boys", "gentleman", "guy", "husband", "father", "brother", "son", "groom", "king", "male"];
const FEMALE_NOUNS: [&str; 12] = ["woman", "women", "girl", "girls", "lady", "wife", "mother", "sister", "daughter", "bride", "queen", "female"];
/// Plural wording that implies a second person when the scene has a single cast member.
const PLURAL_TERMS: [&str; 8] = ["they", "them", "their", "theirs", "themselves", "both", "couple", "pair"];
const PLURAL_PHRASES: [&str; 2] = ["duo", "side by side"];
/// People an LLM tends to invent. Allowed only when a selected cast member or extra is named with the word.
const INVENTED_SINGLE_TERMS: [&str; 28] = [
    "man", "men", "woman", "women", "girl", "girls", "boy", "boys", "lady", "gentleman", "guy", "stranger", "strangers", "figure", "figures", "silhouette", "silhouettes", "someone", "somebody",
    "companion", "companions", "lover", "lovers", "partner", "partners", "child", "children", "kid",
];
const INVENTED_MORE_TERMS: [&str; 5] = ["kids", "friend", "friends", "person", "people"];
/// Group wording. Also allowed when the scene has mapped extras, since extras are groups of people.
const INVENTED_GROUP_TERMS: [&str; 10] = ["crowd", "crowds", "onlookers", "bystanders", "passersby", "passers-by", "audience", "others", "dancers", "fans"];
const GROUP_EXTRA: [&str; 1] = ["spectators"];
const GENERIC_TERMS: [&str; 8] = ["the", "a", "an", "and", "of", "subject", "character", "person"];
const GENERIC_MORE: [&str; 2] = ["singer", "performer"];

fn generic(term: &str) -> bool {
    GENERIC_TERMS.contains(&term) || GENERIC_MORE.contains(&term)
}

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

fn tokens(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    rx(r"[a-z']+").find_iter(&lowered).flatten().map(|m| m.as_str().to_string()).collect()
}

fn subject_name(subject: &Value) -> String {
    match subject {
        Value::Object(map) => text_or_empty(map.get("name").filter(|v| crate::pyval::truthy(v))).trim().to_string(),
        other => text_or_empty(Some(other)).trim().to_string(),
    }
}

/// The subject as a card: a plain name becomes `{name}`.
fn as_card(subject: &Value) -> Value {
    if subject.is_object() { subject.clone() } else { serde_json::json!({"name": text_or_empty(Some(subject))}) }
}

/// `subject_gender`: `m`, `f` or nothing, from the subject's name and description wording.
pub fn subject_gender(subject: &Value) -> &'static str {
    let words = tokens(&format!("{} {}", text_or_empty(subject.get("name")), text_or_empty(subject.get("description"))));
    let male = words.iter().filter(|w| MALE_PRONOUNS.contains(&w.as_str()) || MALE_NOUNS.contains(&w.as_str())).count();
    let female = words.iter().filter(|w| FEMALE_PRONOUNS.contains(&w.as_str()) || FEMALE_NOUNS.contains(&w.as_str())).count();
    if male > female {
        "m"
    } else if female > male {
        "f"
    } else {
        ""
    }
}

fn name_terms(name: &str) -> BTreeSet<String> {
    let text = rx(r"\s+").replace_all(name.trim(), " ").to_lowercase();
    if text.is_empty() {
        return BTreeSet::new();
    }
    let base = rx(r"(?i)^(?:the|a|an)\s+").replace(&text, "").into_owned();
    let mut terms: BTreeSet<String> = [text.clone(), base.clone()].into_iter().collect();
    if text == base {
        let first = base.split(' ').next().unwrap_or("").to_string();
        if first.chars().count() >= 3 && !generic(&first) {
            terms.insert(first);
        }
    }
    terms.into_iter().filter(|t| !t.is_empty() && !generic(t)).collect()
}

fn compile(terms: &HashSet<String>) -> Option<Regex> {
    if terms.is_empty() {
        return None;
    }
    let mut ordered: Vec<&String> = terms.iter().collect();
    ordered.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then_with(|| a.cmp(b)));
    let alternatives: Vec<String> = ordered.iter().map(|t| fancy_regex::escape(t).replace(' ', r"\s+")).collect();
    Some(rx(&format!(r"(?i)(?<![A-Za-z'])(?:{})(?![A-Za-z])", alternatives.join("|"))))
}

/// The matchers for the people who must not appear in a scene.
#[derive(Debug, Clone)]
pub struct CastGuard {
    /// Matches characters left out of the scene.
    pub pattern: Option<Regex>,
    /// Matches unnamed people the scene does not contain.
    pub invented_pattern: Option<Regex>,
    pub cast_names: Vec<String>,
    pub excluded_names: Vec<String>,
    pub extra_names: Vec<String>,
}

/// `build_cast_guard`: a guard for the people who must not appear in a scene. `all_subjects` and `cast_subjects` are cards with a name and
/// description (plain names are accepted). `extra_names` lists mapped extras, which are allowed in the scene.
pub fn build_cast_guard(all_subjects: &[Value], cast_subjects: &[Value], extra_names: &[String]) -> CastGuard {
    let everyone: Vec<Value> = all_subjects.iter().map(as_card).collect();
    let cast: Vec<Value> = cast_subjects.iter().map(as_card).collect();
    let extras: Vec<String> = extra_names.iter().map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect();
    let cast_keys: HashSet<String> = cast.iter().map(subject_name).filter(|n| !n.is_empty()).map(|n| n.to_lowercase()).collect();
    let excluded: Vec<&Value> = everyone.iter().filter(|item| {
        let name = subject_name(item);
        !name.is_empty() && !cast_keys.contains(&name.to_lowercase())
    }).collect();
    let mut cast_terms: BTreeSet<String> = BTreeSet::new();
    for item in &cast {
        cast_terms.extend(name_terms(&subject_name(item)));
    }
    for name in &extras {
        cast_terms.extend(name_terms(name));
    }
    let cast_words: HashSet<String> = cast_terms.iter().flat_map(|term| term.split(' ').map(str::to_string)).collect();
    let mut terms: HashSet<String> = HashSet::new();
    for item in &excluded {
        terms.extend(name_terms(&subject_name(item)).into_iter().filter(|t| !cast_terms.contains(t) && !cast_words.contains(t)));
    }
    let cast_genders: HashSet<&str> = cast.iter().map(subject_gender).collect();
    if !cast.is_empty() && !cast_genders.contains("") {
        for item in &excluded {
            let gender = subject_gender(item);
            if !gender.is_empty() && !cast_genders.contains(gender) {
                let words: Vec<&str> = if gender == "f" { FEMALE_PRONOUNS.iter().chain(FEMALE_NOUNS.iter()).copied().collect() } else { MALE_PRONOUNS.iter().chain(MALE_NOUNS.iter()).copied().collect() };
                terms.extend(words.into_iter().map(str::to_string));
            }
        }
    }
    if cast.len() <= 1 {
        terms.extend(PLURAL_TERMS.iter().chain(PLURAL_PHRASES.iter()).map(|t| t.to_string()));
    }
    if cast.is_empty() && extras.is_empty() {
        terms.extend(MALE_PRONOUNS.iter().chain(FEMALE_PRONOUNS.iter()).map(|t| t.to_string()));
    }
    let mut invented: HashSet<String> = INVENTED_SINGLE_TERMS.iter().chain(INVENTED_MORE_TERMS.iter()).map(|t| t.to_string()).collect();
    if extras.is_empty() {
        invented.extend(INVENTED_GROUP_TERMS.iter().chain(GROUP_EXTRA.iter()).map(|t| t.to_string()));
    }
    invented.retain(|t| !cast_words.contains(t));
    CastGuard {
        pattern: compile(&terms),
        invented_pattern: compile(&invented),
        cast_names: cast.iter().map(subject_name).filter(|n| !n.is_empty()).collect(),
        excluded_names: excluded.iter().map(|i| subject_name(i)).collect(),
        extra_names: extras,
    }
}

fn patterns(guard: &CastGuard, invented: bool) -> Vec<&Regex> {
    let mut found: Vec<&Regex> = guard.pattern.iter().collect();
    if invented {
        found.extend(guard.invented_pattern.iter());
    }
    found
}

/// `cast_leaks`: distinct lowercase terms in the text that refer to a person outside the scene cast.
pub fn cast_leaks(text: &str, guard: &CastGuard, invented: bool) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut found: BTreeSet<String> = BTreeSet::new();
    for pattern in patterns(guard, invented) {
        found.extend(pattern.find_iter(text).flatten().map(|m| m.as_str().to_lowercase()));
    }
    found.into_iter().collect()
}

/// `strip_cast_leaks`: drop every sentence that refers to a person outside the scene cast.
pub fn strip_cast_leaks(text: &str, guard: &CastGuard, invented: bool) -> String {
    let found = patterns(guard, invented);
    if found.is_empty() || text.is_empty() {
        return text.trim().to_string();
    }
    let split = rx(r"(?<=[.!?])\s+");
    let mut kept_lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            kept_lines.push(String::new());
            continue;
        }
        let kept: Vec<&str> = split.split(line.trim()).flatten().filter(|sentence| !found.iter().any(|p| p.is_match(sentence).unwrap_or(false))).filter(|s| !s.is_empty()).collect();
        kept_lines.push(kept.join(" ").trim().to_string());
    }
    rx(r"\n{3,}").replace_all(&kept_lines.join("\n"), "\n\n").trim().to_string()
}

/// `cast_wall_text`: the positive cast statement that tells the model exactly who is in the scene.
pub fn cast_wall_text(guard: &CastGuard) -> String {
    let (cast, extras) = (&guard.cast_names, &guard.extra_names);
    if cast.is_empty() && extras.is_empty() {
        return "PEOPLE IN THIS SCENE: none. No people appear in this scene. Show only the location, objects, and light. Do not show any person, hand, arm, shadow, reflection, or silhouette.".into();
    }
    let people: Vec<String> = cast.iter().cloned().chain(extras.iter().map(|n| format!("{n} (mapped extras)"))).collect();
    let mut text = format!(
        "PEOPLE IN THIS SCENE: {}. No other person exists in this scene. Do not add an unnamed man, woman, girl, boy, stranger, crowd, extra hand or arm, shadow, reflection, or silhouette of anyone else. Refer to each person only by name, never by pronoun.",
        people.join(", ")
    );
    let banned = guard.excluded_names.join(", ");
    if !banned.is_empty() {
        text.push_str(&format!(" Not in this scene: {banned}."));
    }
    text
}
