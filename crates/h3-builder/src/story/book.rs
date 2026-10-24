//! Story Mode book steps with the selected LLM runner: draft the book, rewrite a chapter, split a pasted story, remove people and find story
//! characters that have no cast card. Port of `storyboard/story_book.py`.
//!
//! Each step takes the usual Storyboard request (runner fields, `story_layer`, `scenes`, `reference_builder`, `lyrics`) and returns
//! `{"story_layer": {...changed Story Mode fields...}, ...}`. Saving is the caller's job.

use h3_core::pyval::{text_or_empty, truthy, Obj};
use h3_core::story::helpers::first_truthy;
use h3_core::story::mode::{
    chapters_from_split, chapters_keep_text, is_chapter_heading, is_story_mode_layer, normalize_story_book, parse_story_book_text, split_paragraphs, story_arc_to_story_text, story_book_to_text,
    sync_story_settings, text, STORY_FLOW_VERSION,
};
use h3_llm::locations::extract_json_object;
use h3_llm::prompts::constant;
use h3_llm::story_prompts::{
    story_book_instruction, story_cast_scan_instruction, story_chapter_rewrite_instruction, story_mode_lyric_guidance, story_remove_people_instruction, story_split_instruction, BookInput, ChapterInput,
};
use serde_json::{json, Value};

use super::context::{cast_lines, lead_name, layer, lyrics_of, scene_count, scenes as request_scenes, song_map, song_seconds, story_cast, story_template, user_places_text, video_style};
use crate::prompt_writers::{number, whole, Llm, TextCall};

pub const STORY_MODE_LYRIC_STRENGTH: i64 = 5;

type Reply = Result<Value, String>;

fn schema(name: &str) -> Option<Value> {
    Some(constant("story_mode", name).clone())
}

/// `_run`: one question to the selected runner. The reply keeps its paragraphs.
pub async fn run(llm: &dyn Llm, payload: &Obj, instruction: String, label: &str, max_new_tokens: i64, temperature: f64, json_schema: Option<Value>) -> Result<(String, Obj), String> {
    let mut call = TextCall::new(instruction, number(payload, "temperature", temperature)?, number(payload, "top_p", 0.92)?, whole(payload, "max_new_tokens", max_new_tokens)?, label).paragraphs();
    call.json_schema = json_schema;
    llm.text(call).await
}

/// `_runner_fields`: the runner and model a step used.
pub fn runner_fields(info: &Obj) -> Obj {
    let mut fields = Obj::new();
    fields.insert("runner".into(), info.get("runner").cloned().unwrap_or_else(|| json!("builtin")));
    fields.insert("used_model".into(), info.get("used_model").cloned().unwrap_or_else(|| json!("")));
    fields.insert("unloaded".into(), info.get("unloaded").cloned().unwrap_or_else(|| json!(true)));
    fields
}

/// `_first_story_mode_fields`: fields a project gets the first time it uses Story Mode.
pub fn first_story_mode_fields(layer: &Value) -> Obj {
    let mut fields = Obj::new();
    if !is_story_mode_layer(layer) {
        fields.insert("story_flow_version".into(), json!(STORY_FLOW_VERSION));
        fields.insert("lyric_story_strength".into(), json!(STORY_MODE_LYRIC_STRENGTH));
    }
    fields
}

/// `_book_fields`: story layer fields for a new or changed book.
pub fn book_fields(layer: &Value, book: &Value, setting_names: &Obj) -> Obj {
    let settings = Value::Array(sync_story_settings(book, setting_names, layer.get("story_settings")));
    let mut fields = Obj::new();
    fields.insert("story_book".into(), book.clone());
    fields.insert("story_book_text".into(), json!(story_book_to_text(book, Some(&settings))));
    fields.insert("story_settings".into(), settings);
    fields
}

fn merge(parts: Vec<Obj>) -> Obj {
    let mut all = Obj::new();
    for part in parts {
        for (key, value) in part {
            all.insert(key, value);
        }
    }
    all
}

fn pair(key: &str, value: Value) -> Obj {
    let mut map = Obj::new();
    map.insert(key.into(), value);
    map
}

fn array(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn strength(layer: &Value) -> Value {
    if is_story_mode_layer(layer) { layer.get("lyric_story_strength").cloned().unwrap_or_else(|| json!(STORY_MODE_LYRIC_STRENGTH)) } else { json!(STORY_MODE_LYRIC_STRENGTH) }
}

fn any_paragraphs(book: &Value) -> bool {
    array(book.get("chapters")).iter().any(|c| c.get("paragraphs").is_some_and(truthy))
}

/// `draft_story_book`: write the whole book from the premise with the LLM.
pub async fn draft_story_book(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = layer(payload);
    let scenes = request_scenes(payload);
    let scene_count = scene_count(payload, &scenes);
    if scene_count <= 0 {
        return Err("Create the timeline scenes before writing the story.".into());
    }
    let premise = {
        let given = first_truthy(payload, &["story_idea", "premise"]).or_else(|| layer.get("overall_story_idea").filter(|v| truthy(v)));
        text(given, 4000)
    };
    let tone = {
        let given = first_truthy(payload, &["tone_notes"]).or_else(|| layer.get("story_tone_notes").filter(|v| truthy(v))).or_else(|| payload.get("style_theme").filter(|v| truthy(v)));
        text(given, 2000)
    };
    let places = user_places_text(payload);
    let guidance = story_mode_lyric_guidance(Some(&strength(&layer)));
    let map = song_map(&scenes);
    let lyrics = if map.is_empty() { lyrics_of(payload, &scenes) } else { map };
    let (video, lead, cast) = (video_style(payload), lead_name(payload), cast_lines(payload));
    let template = story_template(payload, "story_mode_book");
    let previous = text(payload.get("previous_book"), 8000);
    let seed = text(payload.get("seed"), 80);
    let act_count = layer.get("story_act_count").and_then(|v| h3_core::story::mode::int_of_float(Some(v))).unwrap_or(0);
    let instruction = story_book_instruction(&BookInput {
        premise: &premise,
        cast_text: &cast,
        tone_notes: &tone,
        scene_count,
        song_seconds: song_seconds(payload, &scenes),
        lyric_guidance: &guidance,
        lyrics: &lyrics,
        previous_book: &previous,
        seed: &seed,
        allow_new_cast: false,
        video_style: &video,
        lead_name: &lead,
        template: Some(&template),
        act_count,
        places_text: &places,
    });
    let (reply, info) = run(llm, payload, instruction, "Story Mode Book", 6000, 0.7, None).await?;
    let parsed = parse_story_book_text(&reply);
    if array(parsed.book.get("chapters")).is_empty() || !any_paragraphs(&parsed.book) {
        return Err("The LLM did not return a story with chapters. Try again or write the story yourself.".into());
    }
    let (book, names, removed) = keep_book_to_cast(llm, payload, parsed.book, parsed.setting_names).await?;
    let fields = merge(vec![
        first_story_mode_fields(&layer),
        book_fields(&layer, &book, &names),
        pair("overall_story_idea", json!(if premise.is_empty() { text(book.get("premise"), 4000) } else { premise.clone() })),
        pair("story_acts", json!([])),
        pair("story_scene_plan", json!([])),
    ]);
    let mut reply = Obj::new();
    reply.insert("story_layer".into(), Value::Object(fields));
    reply.insert("story_book".into(), book);
    reply.insert("removed_people".into(), json!(removed));
    reply.extend(runner_fields(&info));
    Ok(Value::Object(reply))
}

fn setting_names_of(layer: &Value) -> Obj {
    let mut known = Obj::new();
    for item in array(layer.get("story_settings")).iter().filter_map(Value::as_object) {
        known.insert(text_or_empty(item.get("id")), item.get("name").cloned().unwrap_or(Value::Null));
    }
    known
}

/// `rewrite_story_chapter`: rewrite one chapter with the LLM. Acts and plan entries of that chapter are cleared.
pub async fn rewrite_story_chapter(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = layer(payload);
    let mut book = normalize_story_book(layer.get("story_book"));
    let chapter_id = text(payload.get("chapter_id"), 80);
    let chapters = array(book.get("chapters"));
    let Some(index) = chapters.iter().position(|c| text_or_empty(c.get("id")) == chapter_id) else {
        return Err(format!("Chapter '{chapter_id}' is not in the story book."));
    };
    let settings = layer.get("story_settings").cloned().unwrap_or_else(|| json!([]));
    let book_text = story_book_to_text(&book, Some(&settings));
    let chapter_only = json!({"title": "", "premise": "", "chapters": [chapters[index].clone()]});
    let chapter_text = story_book_to_text(&chapter_only, Some(&settings));
    let guidance = story_mode_lyric_guidance(Some(&strength(&layer)));
    let (cast, places, video, lead) = (cast_lines(payload), user_places_text(payload), video_style(payload), lead_name(payload));
    let template = story_template(payload, "story_mode_chapter");
    let direction = text(payload.get("direction"), 2000);
    let instruction = story_chapter_rewrite_instruction(&ChapterInput {
        book_text: &book_text,
        chapter_number: index as i64 + 1,
        chapter_text: &chapter_text,
        direction: &direction,
        cast_text: &cast,
        lyric_guidance: &guidance,
        allow_new_cast: false,
        video_style: &video,
        lead_name: &lead,
        template: Some(&template),
        places_text: &places,
    });
    let (reply, info) = run(llm, payload, instruction, "Story Mode Chapter", 2400, 0.7, None).await?;
    let parsed = parse_story_book_text(&reply);
    let first_has_text = array(parsed.book.get("chapters")).first().is_some_and(|c| c.get("paragraphs").is_some_and(truthy));
    if !first_has_text {
        return Err("The LLM did not return the chapter. Try again.".into());
    }
    let (rewritten, new_names, removed) = keep_book_to_cast(llm, payload, parsed.book, parsed.setting_names).await?;
    let mut new_chapter = array(rewritten.get("chapters")).into_iter().next().and_then(|c| c.as_object().cloned()).unwrap_or_default();
    let old_chapter = chapters[index].as_object().cloned().unwrap_or_default();
    new_chapter.insert("id".into(), old_chapter.get("id").cloned().unwrap_or(Value::Null));
    if !new_chapter.get("setting_id").is_some_and(truthy) {
        new_chapter.insert("setting_id".into(), old_chapter.get("setting_id").cloned().unwrap_or(Value::Null));
    }
    // The act keeps its purpose and presence grid.
    new_chapter.insert("purpose".into(), old_chapter.get("purpose").cloned().unwrap_or_else(|| json!("")));
    new_chapter.insert("presence".into(), old_chapter.get("presence").cloned().unwrap_or_else(|| json!({})));
    if let Some(list) = book.get_mut("chapters").and_then(Value::as_array_mut) {
        list[index] = Value::Object(new_chapter.clone());
    }
    let mut names = setting_names_of(&layer);
    names.extend(new_names);
    let acts: Vec<Value> = array(layer.get("story_acts")).into_iter().filter(|a| text_or_empty(a.get("chapter_id")) != chapter_id).collect();
    let act_ids: Vec<String> = acts.iter().map(|a| text_or_empty(a.get("id"))).collect();
    let plan: Vec<Value> = array(layer.get("story_scene_plan")).into_iter().filter(|e| act_ids.contains(&text_or_empty(e.get("act_id")))).collect();
    let fields = merge(vec![first_story_mode_fields(&layer), book_fields(&layer, &book, &names), pair("story_acts", Value::Array(acts)), pair("story_scene_plan", Value::Array(plan))]);
    let mut out = Obj::new();
    out.insert("story_layer".into(), Value::Object(fields));
    out.insert("chapter".into(), Value::Object(new_chapter));
    out.insert("removed_people".into(), json!(removed));
    out.extend(runner_fields(&info));
    Ok(Value::Object(out))
}

/// `_fixed_chapter_starts`: 1-based paragraph numbers where the pasted text already has `Chapter N` headings.
fn fixed_chapter_starts(source: &str) -> Vec<i64> {
    let mut starts = Vec::new();
    for (count, block) in split_paragraphs(source).iter().enumerate() {
        let first_line = block.split('\n').next().unwrap_or("");
        if is_chapter_heading(first_line) {
            starts.push(count as i64 + 1);
        }
    }
    starts
}

/// `split_pasted_story`: turn a story the user wrote into chapters with settings. The words are kept exactly as written. A story that
/// already has `Chapter N` headings keeps them and the LLM only names the settings. Otherwise the LLM picks the chapter starts by paragraph
/// number.
pub async fn split_pasted_story(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = layer(payload);
    let story = text(first_truthy(payload, &["text", "story_text"]), 200_000);
    if story.is_empty() {
        return Err("Paste the story text first.".into());
    }
    let parsed = parse_story_book_text(&story);
    let parsed_chapters = array(parsed.book.get("chapters"));
    let has_headings = parsed_chapters.len() > 1 || !fixed_chapter_starts(&story).is_empty();
    let (paragraphs, fixed_starts): (Vec<String>, Vec<i64>) = if has_headings {
        let paragraphs: Vec<String> = parsed_chapters.iter().flat_map(|c| array(c.get("paragraphs"))).map(|p| text_or_empty(Some(&p))).collect();
        let mut starts = Vec::new();
        let mut position = 1i64;
        for chapter in &parsed_chapters {
            starts.push(position);
            position += array(chapter.get("paragraphs")).len() as i64;
        }
        (paragraphs, starts)
    } else {
        (split_paragraphs(&story), Vec::new())
    };
    if paragraphs.is_empty() {
        return Err("The pasted story has no paragraphs.".into());
    }
    let numbered = paragraphs.iter().enumerate().map(|(i, p)| format!("[{}] {p}", i + 1)).collect::<Vec<_>>().join("\n\n");
    let numbered: String = numbered.chars().take(60_000).collect();
    let scenes = request_scenes(payload);
    let instruction = story_split_instruction(&numbered, &fixed_starts, scene_count(payload, &scenes));
    let (reply, info) = run(llm, payload, instruction, "Story Mode Split", 1600, 0.2, schema("STORY_SPLIT_SCHEMA")).await?;
    let mut split: Vec<Value> = match extract_json_object(&reply) {
        Ok(Value::Object(map)) => array(map.get("chapters")),
        _ => Vec::new(),
    };
    if !fixed_starts.is_empty() {
        // Keep the user's own chapter breaks and titles. Take only the settings (and missing titles) from the LLM.
        let suggestions = {
            let mut padded = split.clone();
            padded.extend(std::iter::repeat_n(json!({}), fixed_starts.len()));
            padded
        };
        split = Vec::new();
        for (index, start) in fixed_starts.iter().enumerate() {
            let suggestion = suggestions[index].as_object().cloned().unwrap_or_default();
            let own = parsed_chapters[index].as_object().cloned().unwrap_or_default();
            let own_title = text_or_empty(own.get("title"));
            let own_title = if own_title == format!("Chapter {}", index + 1) { String::new() } else { own_title };
            let own_setting_id = text_or_empty(own.get("setting_id"));
            let own_setting = parsed.setting_names.iter().find(|(sid, _)| **sid == own_setting_id).map(|(_, name)| text_or_empty(Some(name))).unwrap_or_default();
            let pick = |own: String, key: &str| if !own.is_empty() { json!(own) } else { suggestion.get(key).filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("")) };
            split.push(json!({"start_paragraph": start, "title": pick(own_title, "title"), "setting": pick(own_setting, "setting")}));
        }
    }
    let (chapters, names) = chapters_from_split(&paragraphs, Some(&Value::Array(split)));
    if !chapters_keep_text(&paragraphs, &chapters) {
        return Err("Splitting changed the story text. Nothing was saved.".into());
    }
    let parsed_title = text_or_empty(parsed.book.get("title"));
    let title = if parsed_title.is_empty() { text(payload.get("title"), 200) } else { parsed_title };
    let book = normalize_story_book(Some(&json!({"title": title, "premise": parsed.book.get("premise").cloned().unwrap_or(Value::Null), "chapters": chapters})));
    let mut fields = merge(vec![first_story_mode_fields(&layer), book_fields(&layer, &book, &names), pair("story_acts", json!([])), pair("story_scene_plan", json!([]))]);
    let premise = text_or_empty(book.get("premise"));
    if !premise.is_empty() && !layer.get("overall_story_idea").is_some_and(truthy) {
        fields.insert("overall_story_idea".into(), json!(premise));
    }
    let mut out = Obj::new();
    out.insert("story_layer".into(), Value::Object(fields));
    out.insert("story_book".into(), book);
    out.extend(runner_fields(&info));
    Ok(Value::Object(out))
}

/// `convert_story_arc`: move an older project to Story Mode. Its song-section story arc becomes the story book.
pub async fn convert_story_arc(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = layer(payload);
    let story = story_arc_to_story_text(&text_or_empty(layer.get("user_story_arc")));
    if story.is_empty() {
        return Err("This project has no story arc to convert. Write a new story instead.".into());
    }
    let mut next = payload.clone();
    next.insert("text".into(), json!(story));
    split_pasted_story(llm, &next).await
}

/// `_known_people`: names the story may use: the story cast and the extras.
fn known_people(payload: &Obj) -> Vec<String> {
    let mut known: Vec<String> = story_cast(payload).iter().map(|s| text_or_empty(s.get("name"))).collect();
    let extras = payload.get("reference_builder").filter(|v| truthy(v)).and_then(|r| r.get("extras")).filter(|v| truthy(v)).cloned();
    if let Some(Value::Array(items)) = extras {
        known.extend(items.iter().filter_map(Value::as_object).map(|e| text(e.get("name"), 160)));
    }
    known.into_iter().filter(|n| !n.is_empty()).collect()
}

/// Why a cast scan failed: the reply was not a list, or the runner failed.
enum ScanError {
    Format(String),
    Run(String),
}

/// `_scan_people`: the LLM's list of people in the text who are not in `known`, and the run.
async fn scan_people(llm: &dyn Llm, payload: &Obj, source: &str, known: &[String], label: &str) -> Result<(Vec<Value>, Obj), ScanError> {
    let names = known.iter().map(|n| format!("- {n}")).collect::<Vec<_>>().join("\n");
    let instruction = story_cast_scan_instruction(&source.chars().take(60_000).collect::<String>(), &names);
    let (reply, info) = run(llm, payload, instruction, label, 1600, 0.2, schema("STORY_CAST_SCAN_SCHEMA")).await.map_err(ScanError::Run)?;
    match extract_json_object(&reply) {
        Ok(Value::Object(map)) => Ok((array(map.get("characters")), info)),
        Ok(_) => Err(ScanError::Format("The LLM did not return a character list: 'list' object has no attribute 'get'".into())),
        Err(error) => Err(ScanError::Format(format!("The LLM did not return a character list: {error}"))),
    }
}

/// `_without_people`: the book rewritten by the LLM so `people` are gone. The last value is false when the reply was unusable.
async fn without_people(llm: &dyn Llm, payload: &Obj, book: &Value, setting_names: &Obj, people: &[String]) -> Result<(Value, Obj, bool), String> {
    let layer = layer(payload);
    let settings = Value::Array(sync_story_settings(book, setting_names, layer.get("story_settings")));
    let instruction = story_remove_people_instruction(&story_book_to_text(book, Some(&settings)), &cast_lines(payload), people);
    let size: usize = array(book.get("chapters")).iter().flat_map(|c| array(c.get("paragraphs"))).map(|p| text_or_empty(Some(&p)).chars().count()).sum();
    let (reply, _) = run(llm, payload, instruction, "Story Mode Remove People", (size as i64 / 2).clamp(0, 8000).max(1600), 0.5, None).await?;
    let parsed = parse_story_book_text(&reply);
    let mut fixed = parsed.book;
    let (old_chapters, new_chapters) = (array(book.get("chapters")), array(fixed.get("chapters")));
    if new_chapters.len() != old_chapters.len() || !new_chapters.iter().all(|c| c.get("paragraphs").is_some_and(truthy)) {
        return Ok((book.clone(), setting_names.clone(), false));
    }
    let mut rebuilt = Vec::new();
    for (new, old) in new_chapters.iter().zip(&old_chapters) {
        let mut chapter = new.as_object().cloned().unwrap_or_default();
        chapter.insert("id".into(), old.get("id").cloned().unwrap_or(Value::Null));
        if !chapter.get("setting_id").is_some_and(truthy) {
            chapter.insert("setting_id".into(), old.get("setting_id").cloned().unwrap_or(Value::Null));
        }
        rebuilt.push(Value::Object(chapter));
    }
    if let Some(map) = fixed.as_object_mut() {
        map.insert("chapters".into(), Value::Array(rebuilt));
        for key in ["title", "premise"] {
            if !map.get(key).is_some_and(truthy) {
                map.insert(key.into(), book.get(key).cloned().unwrap_or_else(|| json!("")));
            }
        }
    }
    let mut names = setting_names.clone();
    names.extend(parsed.setting_names);
    Ok((fixed, names, true))
}

/// `_keep_book_to_cast`: rewrite out anyone outside the story cast. Returns the book, the setting names and the people removed.
async fn keep_book_to_cast(llm: &dyn Llm, payload: &Obj, book: Value, setting_names: Obj) -> Result<(Value, Obj, Vec<String>), String> {
    if story_cast(payload).is_empty() {
        return Ok((book, setting_names, Vec::new()));
    }
    let layer = layer(payload);
    let settings = Value::Array(sync_story_settings(&book, &setting_names, layer.get("story_settings")));
    let known = known_people(payload);
    let found = match scan_people(llm, payload, &story_book_to_text(&book, Some(&settings)), &known, "Story Mode Cast Check").await {
        Ok((found, _)) => found,
        Err(ScanError::Format(error)) => {
            eprintln!("[VRGDG-RS Story Mode] Cast check skipped, the reply was not a list: {error}");
            return Ok((book, setting_names, Vec::new()));
        }
        Err(ScanError::Run(error)) => return Err(error),
    };
    let known_lower: Vec<String> = known.iter().map(|k| k.to_lowercase()).collect();
    let mut people: Vec<String> = Vec::new();
    for item in &found {
        let name = item.as_object().map(|m| text(m.get("name"), 120)).unwrap_or_default();
        if !name.is_empty() && !known_lower.contains(&name.to_lowercase()) && !people.contains(&name) {
            people.push(name);
        }
    }
    if people.is_empty() {
        return Ok((book, setting_names, Vec::new()));
    }
    let (fixed, names, ok) = without_people(llm, payload, &book, &setting_names, &people).await?;
    Ok(if ok { (fixed, names, people) } else { (book, setting_names, Vec::new()) })
}

/// `remove_story_people`: rewrite the book without the named people (`names`), so only the story cast remains.
pub async fn remove_story_people(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = layer(payload);
    let book = normalize_story_book(layer.get("story_book"));
    if array(book.get("chapters")).is_empty() {
        return Err("Write the story first.".into());
    }
    let people: Vec<String> = array(payload.get("names")).iter().map(|n| text(Some(n), 120)).filter(|n| !n.is_empty()).collect();
    if people.is_empty() {
        return Err("Name the people to remove from the story.".into());
    }
    let known = setting_names_of(&layer);
    let (fixed, setting_names, ok) = without_people(llm, payload, &book, &known, &people).await?;
    if !ok {
        return Err("The LLM did not return the whole story. Nothing was changed; try again.".into());
    }
    let fields = merge(vec![first_story_mode_fields(&layer), book_fields(&layer, &fixed, &setting_names), pair("story_acts", json!([])), pair("story_scene_plan", json!([]))]);
    let mut out = Obj::new();
    out.insert("story_layer".into(), Value::Object(fields));
    out.insert("story_book".into(), fixed);
    out.insert("removed_people".into(), json!(people));
    Ok(Value::Object(out))
}

/// `scan_story_cast`: people in the story with no character card.
pub async fn scan_story_cast(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = layer(payload);
    let book = normalize_story_book(layer.get("story_book"));
    if array(book.get("chapters")).is_empty() {
        return Err("Write the story first.".into());
    }
    let known = known_people(payload);
    let (found, info) = match scan_people(llm, payload, &story_book_to_text(&book, layer.get("story_settings")), &known, "Story Mode Cast Scan").await {
        Ok(done) => done,
        Err(ScanError::Format(message) | ScanError::Run(message)) => return Err(message),
    };
    let known_lower: Vec<String> = known.iter().map(|k| k.to_lowercase()).collect();
    let mut characters: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for item in &found {
        let Some(item) = item.as_object() else { continue };
        let name = text(item.get("name"), 120);
        let key = name.to_lowercase();
        if name.is_empty() || known_lower.contains(&key) || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let role = text(item.get("role"), 40).to_lowercase();
        characters.push(json!({
            "name": name,
            "role": if matches!(role.as_str(), "main" | "supporting" | "background") { role } else { "supporting".to_string() },
            "description": text(item.get("description"), 600),
        }));
    }
    let mut out = Obj::new();
    out.insert("characters".into(), Value::Array(characters));
    out.extend(runner_fields(&info));
    Ok(Value::Object(out))
}
