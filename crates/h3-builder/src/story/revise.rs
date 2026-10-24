//! Story Mode: change the story from one scene (Update Story From This Scene). Port of `storyboard/story_revise.py`.
//!
//! The user sets a scene's characters and writes what should happen in it. One LLM step rewrites that scene's plan and adapts its act and
//! chapter to agree, as little as possible, and names the other scenes of the act that no longer fit. Takes the usual Story Mode request plus
//! `scene_id` and returns the changed story layer fields. Saving is the caller's job.

use std::collections::HashMap;

use h3_core::pyval::{text_or_empty, to_int, truthy};
use h3_core::story::mode::{acts_to_value, normalize_story_acts, normalize_story_book, normalize_story_scene_plan, normalize_story_settings, plan_to_value, story_book_to_text, text};
use h3_llm::locations::extract_json_object;
use h3_llm::prompts::constant;
use h3_llm::story_prompts::{story_mode_lyric_guidance, story_revise_scene_instruction, ReviseInput};
use serde_json::{json, Value};

use super::book::{run, runner_fields};
use super::context::{cast_lines, layer as request_layer, scenes as request_scenes, story_cast, story_template};
use crate::prompt_writers::Llm;
use h3_core::pyval::Obj;

type Reply = Result<Value, String>;

fn s(value: Option<&Value>) -> String {
    text_or_empty(value)
}

/// `revise_story_from_scene`: rewrite one scene's plan from the user's change and adapt its act and chapter to it.
pub async fn revise_story_from_scene(llm: &dyn Llm, payload: &Obj) -> Reply {
    let layer = request_layer(payload);
    let scene_id = text(payload.get("scene_id"), 160);
    let scene_ids: Vec<String> = request_scenes(payload).iter().enumerate().map(|(i, sc)| match sc.get("id").filter(|v| truthy(v)) {
        Some(v) => text_or_empty(Some(v)),
        None => format!("scene_{}", i + 1),
    }).collect();
    let number_by_id: HashMap<&String, i64> = scene_ids.iter().enumerate().map(|(i, id)| (id, i as i64 + 1)).collect();
    let mut plan = normalize_story_scene_plan(layer.get("story_scene_plan"));
    let entry_index = plan.iter().position(|item| item.scene_id == scene_id);
    let Some(entry_index) = entry_index.filter(|_| number_by_id.contains_key(&scene_id)) else {
        return Err("Plan this scene first; the story is updated from a planned scene.".into());
    };
    let mut acts = normalize_story_acts(layer.get("story_acts"));
    let act_index = acts.iter().position(|item| item.id == plan[entry_index].act_id);
    let mut book = normalize_story_book(layer.get("story_book"));
    let chapters: Vec<Value> = book.get("chapters").and_then(Value::as_array).cloned().unwrap_or_default();
    let chapter_index = act_index.and_then(|a| chapters.iter().position(|c| s(c.get("id")) == acts[a].chapter_id));
    let (Some(act_index), Some(chapter_index)) = (act_index, chapter_index) else {
        return Err("This scene's act or chapter is missing. Rebuild the acts first.".into());
    };
    let chapter = chapters[chapter_index].clone();
    let settings = normalize_story_settings(layer.get("story_settings"));
    let setting_name = settings.iter().find(|item| s(item.get("id")) == s(chapter.get("setting_id"))).map(|item| s(item.get("name"))).unwrap_or_default();
    let mut names_by_id: Vec<(String, String)> = Vec::new();
    for subject in story_cast(payload) {
        if subject.get("id").is_some_and(truthy) {
            let (id, name) = (s(subject.get("id")), s(subject.get("name")));
            match names_by_id.iter_mut().find(|(i, _)| *i == id) {
                Some(entry) => entry.1 = name,
                None => names_by_id.push((id, name)),
            }
        }
    }
    let ids_by_name: HashMap<String, String> = names_by_id.iter().map(|(id, name)| (name.to_lowercase(), id.clone())).collect();
    let name_or_id = |id: &str| names_by_id.iter().find(|(i, _)| i == id).map_or(id.to_string(), |(_, n)| n.clone());
    let by_scene: HashMap<String, usize> = plan.iter().enumerate().map(|(i, item)| (item.scene_id.clone(), i)).collect();
    let mut ordered_ids = acts[act_index].scene_ids.clone();
    ordered_ids.sort_by_key(|id| number_by_id.get(id).copied().unwrap_or(0));
    let act_lines: Vec<String> = ordered_ids
        .iter()
        .map(|sid| {
            let other = by_scene.get(sid).map(|i| &plan[*i]);
            let cast = other.map(|o| o.cast_ids.iter().map(|c| name_or_id(c)).collect::<Vec<_>>().join(", ")).filter(|c| !c.is_empty()).unwrap_or_else(|| "as planned".into());
            let spot = other.map(|o| o.spot.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| setting_name.clone());
            let slice = other.map(|o| o.story_slice.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| "[no slice yet]".into());
            let number = number_by_id.get(sid).map_or("?".to_string(), |n| n.to_string());
            format!("Scene {number} ({spot}): {slice} [characters: {cast}]")
        })
        .collect();
    let settings_value = Value::Array(settings.clone());
    let template = story_template(payload, "story_mode_revise_scene");
    let guidance = story_mode_lyric_guidance(Some(layer.get("lyric_story_strength").unwrap_or(&json!(5))));
    let cast_text = cast_lines(payload);
    let book_text = story_book_to_text(&book, Some(&settings_value));
    let chapter_text = chapter.get("paragraphs").and_then(Value::as_array).map(|p| p.iter().map(|x| s(Some(x))).collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
    let entry = plan[entry_index].clone();
    let scene_cast = entry.cast_ids.iter().map(|c| name_or_id(c)).collect::<Vec<_>>().join(", ");
    let act_scenes_text = act_lines.join("\n");
    let instruction = story_revise_scene_instruction(&ReviseInput {
        book_text: &book_text,
        chapter_number: chapter_index as i64 + 1,
        chapter_title: &s(chapter.get("title")),
        setting_name: &setting_name,
        chapter_text: &chapter_text,
        act_title: &acts[act_index].title,
        act_summary: &acts[act_index].summary,
        act_scenes_text: &act_scenes_text,
        scene_number: number_by_id[&scene_id],
        scene_cast: &scene_cast,
        user_note: &entry.user_note,
        cast_text: &cast_text,
        lyric_guidance: &guidance,
        template: Some(&template),
    });
    let (raw, info) = run(llm, payload, instruction, "Story Mode Revise Scene", 3200, 0.5, Some(constant("story_mode", "STORY_REVISE_SCENE_SCHEMA").clone())).await?;
    let reply = match extract_json_object(&raw)? {
        Value::Object(map) => map,
        _ => return Err("'list' object has no attribute 'get'".into()),
    };
    let scene = reply.get("scene").and_then(Value::as_object).cloned().unwrap_or_default();
    if text(scene.get("story_slice"), 1200).is_empty() {
        return Err("The LLM did not return the changed scene. Try again.".into());
    }
    // The user's characters and direction are kept exactly. The LLM fills in the rest of the scene.
    let performer = ids_by_name.get(&s(scene.get("performer").filter(|v| truthy(v))).trim().to_lowercase()).cloned().unwrap_or_default();
    let performers: Vec<String> = if entry.performer_source == "manual" {
        entry.performer_ids.iter().filter(|c| entry.cast_ids.contains(c)).cloned().collect()
    } else if entry.cast_ids.contains(&performer) {
        vec![performer]
    } else {
        Vec::new()
    };
    {
        let target = &mut plan[entry_index];
        target.story_slice = text(scene.get("story_slice"), 1200);
        let spot = text(scene.get("spot"), 160);
        if !spot.is_empty() {
            target.spot = spot;
        }
        let mood = text(scene.get("mood"), 200);
        if !mood.is_empty() {
            target.mood = mood;
        }
        target.performer_id = performers.first().cloned().unwrap_or_default();
        target.performer_ids = performers;
        target.interaction = if target.cast_ids.len() > 1 { text(scene.get("interaction"), 600) } else { String::new() };
    }
    let summary = text(reply.get("act_summary"), 2000);
    if !summary.is_empty() {
        acts[act_index].summary = summary;
    }
    let paragraphs: Vec<Value> = reply
        .get("chapter_paragraphs")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(|t| text(Some(&json!(t)), 4000)).filter(|t| !t.is_empty()).map(Value::String).collect())
        .unwrap_or_default();
    if !paragraphs.is_empty() {
        if let Some(list) = book.get_mut("chapters").and_then(Value::as_array_mut) {
            list[chapter_index]["paragraphs"] = Value::Array(paragraphs);
        }
    }
    let mut others: Vec<(String, String)> = Vec::new();
    for item in reply.get("other_scenes").and_then(Value::as_array).cloned().unwrap_or_default() {
        let Some(number) = item.as_object().and_then(|m| m.get("scene")).and_then(to_int) else { continue };
        let other_id = if number >= 1 && number as usize <= scene_ids.len() { scene_ids[number as usize - 1].clone() } else { String::new() };
        if !other_id.is_empty() && other_id != scene_id && acts[act_index].scene_ids.contains(&other_id) && !others.iter().any(|(id, _)| *id == other_id) {
            others.push((other_id, text(item.get("reason"), 300)));
        }
    }
    let mut out = Obj::new();
    out.insert(
        "story_layer".into(),
        json!({
            "story_scene_plan": plan_to_value(&plan),
            "story_acts": acts_to_value(&acts),
            "story_book": book,
            "story_book_text": story_book_to_text(&book, Some(&settings_value)),
        }),
    );
    out.insert("scene_id".into(), json!(scene_id));
    out.insert("act_id".into(), json!(acts[act_index].id));
    out.insert("chapter_id".into(), json!(s(chapter.get("id"))));
    out.insert("other_scenes".into(), Value::Array(others.into_iter().map(|(id, reason)| json!({"scene_id": id, "reason": reason})).collect()));
    out.extend(runner_fields(&info));
    Ok(Value::Object(out))
}
