//! The instruction texts of Story Mode (the story book, acts, scene plan, setting looks, spots and scene revision). Port of the
//! functions in `llm/prompts/story_mode.py`. The editable templates, the rules and the JSON schemas are the pack's constants in
//! `data/prompt_constants.json`.

use h3_core::pyval::round_half_even;

use crate::prompts::text as constant;

fn c(name: &str) -> &'static str {
    constant("story_mode", name)
}

/// `with_realism`: an instruction with the believable-places and natural-actions rules added before its data and reply format.
pub fn with_realism(text: &str) -> String {
    format!("{}\n- {}\n- {}\n", text.trim_end(), c("BELIEVABLE_PLACES_RULE"), c("NATURAL_ACTIONS_RULE"))
}

/// `render_story_template`: a template with its `{{name}}` tokens filled in. Unknown tokens stay as written.
pub fn render_story_template(template: &str, tokens: &[(&str, String)]) -> String {
    let mut text = template.to_string();
    for (name, value) in tokens {
        text = text.replace(&format!("{{{{{name}}}}}"), value);
    }
    // A saved instruction loses its last line break. The step's own text always starts on a new line.
    if text.is_empty() || text.ends_with('\n') { text } else { format!("{text}\n") }
}

pub fn render_with_realism(template: &str, tokens: &[(&str, String)]) -> String {
    with_realism(&render_story_template(template, tokens))
}

/// A template rendered, then the believable-places rule added (steps that design places and have no people).
pub fn render_with_places(template: &str, tokens: &[(&str, String)]) -> String {
    format!("{}\n- {}\n", render_story_template(template, tokens).trim_end(), c("BELIEVABLE_PLACES_RULE"))
}

/// `story_mode_lyric_guidance`: how strongly a scene's emotion follows its lyric line (never events or places).
pub fn story_mode_lyric_guidance(strength: Option<&serde_json::Value>) -> String {
    let value = strength.and_then(h3_core::pyval::to_float).filter(|f| f.is_finite()).map_or(5, |f| f.trunc() as i64).clamp(0, 10);
    let guidance = if value <= 0 {
        "Ignore the lyrics entirely. The story alone sets the mood."
    } else if value <= 3 {
        "Let the lyrics tint the mood lightly. The story sets the emotion."
    } else if value <= 6 {
        "Balance the story's emotion with the feeling of the lyrics."
    } else if value <= 8 {
        "Let each scene's emotion follow the feeling of its lyric line closely."
    } else {
        "Match each scene's emotion to its lyric line as closely as the story allows."
    };
    format!("Lyric Story Strength: {value}/10 (emotion only). {guidance} {}", c("LYRICS_ARE_MOOD_ONLY"))
}

/// `story_book_size`: `(acts, paragraphs)` a book should have for a timeline of `scene_count` scenes. `act_count` is the user's choice. 0
/// lets the scene count decide (3 to 5 acts).
pub fn story_book_size(scene_count: i64, act_count: i64) -> (i64, i64) {
    let count = if scene_count == 0 { 1 } else { scene_count }.max(1);
    let acts = if act_count != 0 {
        act_count
    } else {
        let rounded = round_half_even(count as f64 / 10.0);
        3.max(5.min(if rounded == 0 { 3 } else { rounded }))
    };
    let paragraphs = (acts * 2).max(round_half_even(count as f64 / 2.5));
    (acts, paragraphs)
}

/// `story_cast_rule`: the cast rule line for the book and chapter prompts.
pub fn story_cast_rule(cast_text: &str, allow_new_cast: bool) -> String {
    if allow_new_cast || cast_text.is_empty() {
        return "Use the cast names below. Main characters drive the story. You may add a few new minor people when the story needs them; give each a name.".to_string();
    }
    let people_lines = cast_text.split("\nRelationships:").next().unwrap_or("");
    let count = people_lines.lines().filter(|l| l.trim().starts_with("- ")).count();
    let people = if count == 1 { "1 character".to_string() } else { format!("{count} characters") };
    format!(
        "The story has exactly {people}: the cast below, and no one else. Do not add, name or describe any other person, not even a minor one (no friends, partners, strangers, staff or family who are not in the cast). Places that would hold people (a club, a street, a restaurant) stay empty or have only a blurred, unnamed crowd that never speaks, acts or matters to the story. Every event happens to or through the cast. {}",
        c("CAST_PRESENCE_RULE")
    )
}

/// `story_medium_rule`: how the story is told on screen.
pub fn story_medium_rule(video_style: &str, lead_name: &str) -> String {
    let lead = if lead_name.is_empty() { "the main character" } else { lead_name };
    match video_style {
        "spoken" => format!("This is a spoken story set to music. The characters talk: write the lines they say in quotes, and let the story move through what {lead} and the others say and do. No one sings."),
        "story_only" => "This story plays out to the song with no one singing or speaking on camera. Everything is told through action, faces, places and objects.".to_string(),
        _ => format!("This is the story of a music video. {lead} performs the song inside the story: write the moments where {lead} sings, alone, to the camera, or to someone in the scene, woven into what is happening (singing in the rain on the street, singing alone in the empty room). Put the biggest of these moments on the chorus."),
    }
}

/// The data block for the user's own places, or nothing when there are none.
pub fn user_places_block(places_text: &str) -> String {
    if places_text.trim().is_empty() { String::new() } else { format!("Places the user wants (one per line):\n{places_text}\n\n") }
}

/// The data block for the place cards already made, or nothing when there are none.
pub fn made_places_block(made_places_text: &str) -> String {
    if made_places_text.trim().is_empty() { String::new() } else { format!("Places already made (name: look):\n{made_places_text}\n\n") }
}

fn or_text<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() { default } else { value }
}

fn extra_rules(places_text: &str) -> String {
    let places = if places_text.trim().is_empty() { String::new() } else { format!("- {}\n", c("USER_PLACES_RULE")) };
    format!("{places}- {}\n", c("CHARACTER_NOTES_RULE"))
}

/// The inputs of `story_book_instruction`.
#[derive(Default)]
pub struct BookInput<'a> {
    pub premise: &'a str,
    pub cast_text: &'a str,
    pub tone_notes: &'a str,
    pub scene_count: i64,
    pub song_seconds: f64,
    pub lyric_guidance: &'a str,
    pub lyrics: &'a str,
    pub previous_book: &'a str,
    pub seed: &'a str,
    pub allow_new_cast: bool,
    pub video_style: &'a str,
    pub lead_name: &'a str,
    pub template: Option<&'a str>,
    pub act_count: i64,
    pub places_text: &'a str,
}

/// `story_book_instruction`: draft the whole story book from the user's premise, in `act_count` acts.
pub fn story_book_instruction(i: &BookInput) -> String {
    let (acts, paragraphs) = story_book_size(i.scene_count, i.act_count);
    let template = i.template.filter(|t| !t.is_empty()).unwrap_or_else(|| c("STORY_BOOK_TEMPLATE"));
    let rendered = render_with_realism(
        template,
        &[
            ("medium_rule", story_medium_rule(i.video_style, i.lead_name)),
            ("acts", acts.to_string()),
            ("chapters", acts.to_string()),
            ("paragraphs", paragraphs.to_string()),
            ("scene_count", i.scene_count.to_string()),
            ("song_seconds", format!("{:.0}", i.song_seconds)),
            ("cast_rule", story_cast_rule(i.cast_text, i.allow_new_cast)),
            ("lyric_guidance", i.lyric_guidance.to_string()),
        ],
    );
    format!(
        "{rendered}{}- Output only the book in exactly this format, with no notes before or after it:\n\n{}\n\nPremise from the user:\n{}\n\nCast:\n{}\n\n{}Tone, genre and world notes:\n{}\n\nSong map in order (time, section, lyric). The timing and energy set the story's pace; the lyric words are for mood and emotion only:\n{}\n\nPrevious draft to move away from on this reroll (do not reuse anyone from it who is not in the cast):\n{}\n\nVariation seed: {}",
        extra_rules(i.places_text),
        c("_BOOK_FORMAT"),
        or_text(i.premise, "[none, invent one from the cast and tone]"),
        or_text(i.cast_text, "[none, invent a small cast]"),
        user_places_block(i.places_text),
        or_text(i.tone_notes, "[none]"),
        or_text(i.lyrics, "[none]"),
        or_text(i.previous_book, "[none]"),
        or_text(i.seed, "[none]"),
    )
}

/// The inputs of `story_chapter_rewrite_instruction`.
#[derive(Default)]
pub struct ChapterInput<'a> {
    pub book_text: &'a str,
    pub chapter_number: i64,
    pub chapter_text: &'a str,
    pub direction: &'a str,
    pub cast_text: &'a str,
    pub lyric_guidance: &'a str,
    pub allow_new_cast: bool,
    pub video_style: &'a str,
    pub lead_name: &'a str,
    pub template: Option<&'a str>,
    pub places_text: &'a str,
}

/// `story_chapter_rewrite_instruction`: rewrite or expand one act, keeping the rest of the book as context.
pub fn story_chapter_rewrite_instruction(i: &ChapterInput) -> String {
    let template = i.template.filter(|t| !t.is_empty()).unwrap_or_else(|| c("STORY_CHAPTER_TEMPLATE"));
    let rendered = render_with_realism(
        template,
        &[
            ("chapter_number", i.chapter_number.to_string()),
            ("cast_rule", story_cast_rule(i.cast_text, i.allow_new_cast)),
            ("medium_rule", story_medium_rule(i.video_style, i.lead_name)),
            ("lyric_guidance", i.lyric_guidance.to_string()),
        ],
    );
    let number = i.chapter_number;
    format!(
        "{rendered}{}- Output only the act in exactly this format:\n\nAct {number}: <act title>\n\n<paragraph>\n\n<paragraph>\n\nUser direction:\n{}\n\nCast:\n{}\n\n{}Act to rewrite:\n{}\n\nWhole book for context:\n{}",
        extra_rules(i.places_text),
        or_text(i.direction, "Improve and expand this act."),
        or_text(i.cast_text, "[none]"),
        user_places_block(i.places_text),
        i.chapter_text,
        i.book_text,
    )
}

/// `story_split_instruction`: group a pasted story's paragraphs into chapters, each with one setting.
pub fn story_split_instruction(numbered_paragraphs: &str, fixed_starts: &[i64], scene_count: i64) -> String {
    let fixed = if fixed_starts.is_empty() {
        "Choose where each chapter starts. A chapter starts where the story moves to a new general setting or a clear new part begins. Aim for 3-7 chapters.\n".to_string()
    } else {
        format!(
            "The story already has chapter headings. Keep exactly these chapter starts, in order: {}. Only give each one a title (keep a written title) and a setting.\n",
            fixed_starts.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
        )
    };
    format!(
        "You are an editor dividing a finished story into chapters for a music video. Do not rewrite any text.\n\nThe video has {scene_count} scenes.\nEvery chapter happens in ONE general setting (1-4 words, e.g. Home, The Night Train). A new setting needs a new chapter.\n{fixed}Return JSON only: {{\"chapters\": [{{\"start_paragraph\": 1, \"title\": \"...\", \"setting\": \"...\"}}]}}. start_paragraph is the number of the first paragraph of the chapter. The first chapter starts at 1.\n\nParagraphs:\n{numbered_paragraphs}"
    )
}

/// `story_remove_people_instruction`: rewrite a story (or one chapter) so the named people are gone and only the cast remains.
pub fn story_remove_people_instruction(story_text: &str, cast_text: &str, names: &[String]) -> String {
    format!(
        "You are a novelist editing a story that a music video will tell.\n\nRewrite the text so these people are gone: {}. Afterwards only the cast below appears, acts, speaks or is named.\n\nRules:\n- Keep every chapter in the same order, with its 'Chapter N: title' line and its 'Setting:' line unchanged. Keep the Title and Premise lines when the text has them; rewrite the premise only to drop removed people.\n- Keep the cast's actions and about the same length.\n- Where a removed person did something, the cast does it alone, or it shows through things: a message on a phone, an empty seat, a door closing, a voice off screen with no name.\n- No new people. A crowd may only be a blurred, unnamed background that never acts.\n- Output only the rewritten text in the same format, starting with its first line, with no notes.\n\nCast:\n{}\n\nText:\n{story_text}",
        names.join(", "),
        or_text(cast_text, "[none]"),
    )
}

/// `story_cast_scan_instruction`: list people in the story who have no cast card yet, with a visual description each.
pub fn story_cast_scan_instruction(book_text: &str, known_names: &str) -> String {
    format!(
        "You are a casting director reading a story for a music video.\n\nList every person in the story who is NOT in the known cast list. Only people who appear or act in the story, not people only mentioned in passing. Do not list groups or crowds.\n\nFor each person give:\n- name: as written in the story (invent a short fitting name only if the story names them by role).\n- role: main, supporting or background.\n- description: one sentence of how they look (age range, build, hair, clothing) that fits the story. No actions, no personality.\n\nReturn JSON only: {{\"characters\": [{{\"name\": \"...\", \"role\": \"...\", \"description\": \"...\"}}]}}. Return an empty list when everyone is already cast.\n\nKnown cast:\n{}\n\nStory:\n{book_text}",
        or_text(known_names, "[none]"),
    )
}

/// The inputs of `story_acts_instruction`.
#[derive(Default)]
pub struct ActsInput<'a> {
    pub chapter_number: i64,
    pub chapter_title: &'a str,
    pub setting_name: &'a str,
    pub chapter_text: &'a str,
    pub scenes_text: &'a str,
    pub cast_text: &'a str,
    pub notes_text: &'a str,
    pub target_acts: i64,
    pub template: Option<&'a str>,
    pub places_text: &'a str,
    pub made_places_text: &'a str,
}

/// `story_acts_instruction`: split one chapter into acts, each a contiguous run of the chapter's scenes.
pub fn story_acts_instruction(i: &ActsInput) -> String {
    let (own_places, places_text) = if !i.made_places_text.trim().is_empty() {
        (format!("- {}\n", c("MADE_PLACES_RULE_FOR_ACTS")), "")
    } else if !i.places_text.trim().is_empty() {
        (format!("- {}\n", c("USER_PLACES_RULE_FOR_PLANS")), i.places_text)
    } else {
        (String::new(), i.places_text)
    };
    let template = i.template.filter(|t| !t.is_empty()).unwrap_or_else(|| c("STORY_ACTS_TEMPLATE"));
    let rendered = render_with_realism(template, &[("target_acts", i.target_acts.to_string()), ("lyrics_rule", c("LYRICS_ARE_MOOD_ONLY").to_string())]);
    format!(
        "{rendered}{own_places}Return JSON only: {{\"acts\": [{{\"first_scene\": 1, \"title\": \"...\", \"summary\": \"...\", \"cast\": [\"...\"]}}]}}\n\nChapter {}: {}\nSetting: {}\n\n{}\n\nCast:\n{}\n\n{}{}Timed user notes for these scenes:\n{}\n\nScenes of this chapter (time, length, song section, lyric mood):\n{}",
        i.chapter_number,
        i.chapter_title,
        or_text(i.setting_name, "[not named]"),
        i.chapter_text,
        or_text(i.cast_text, "[none]"),
        made_places_block(i.made_places_text),
        user_places_block(places_text),
        or_text(i.notes_text, "[none]"),
        i.scenes_text,
    )
}

fn plan_performance_rule(video_style: &str) -> &'static str {
    match video_style {
        "spoken" => "Use speaking for scenes with vocals: a character says a line. Never singing.",
        "story_only" => "Use story for every scene with vocals, and instrumental where there are none. Never singing or speaking.",
        _ => "Use singing for most scenes with vocals, so the performer sings through the video; use story where the action needs the screen to itself.",
    }
}

/// The inputs of `story_scene_plan_instruction`.
#[derive(Default)]
pub struct PlanInput<'a> {
    pub act_title: &'a str,
    pub act_summary: &'a str,
    pub setting_name: &'a str,
    pub chapter_text: &'a str,
    pub scenes_text: &'a str,
    pub cast_text: &'a str,
    pub spot_min_scenes: i64,
    pub previous_spot: &'a str,
    pub lyric_guidance: &'a str,
    pub known_spots: &'a str,
    pub video_style: &'a str,
    pub act_cast: &'a str,
    pub main_name: &'a str,
    pub template: Option<&'a str>,
    pub places_text: &'a str,
    pub made_places_text: &'a str,
}

/// `story_scene_plan_instruction`: give every scene of one act its slice of the story, a spot, a mood, a performance and its cast.
pub fn story_scene_plan_instruction(i: &PlanInput) -> String {
    let (own_places, places_text) = if !i.made_places_text.trim().is_empty() {
        (format!("- {}\n", c("MADE_PLACES_RULE_FOR_SPOTS")), "")
    } else if !i.places_text.trim().is_empty() {
        (format!("- {}\n", c("USER_PLACES_RULE_FOR_SPOTS")), i.places_text)
    } else {
        (String::new(), i.places_text)
    };
    let template = i.template.filter(|t| !t.is_empty()).unwrap_or_else(|| c("STORY_SCENE_PLAN_TEMPLATE"));
    let rendered = render_with_realism(
        template,
        &[
            ("main_character_line", if i.main_name.is_empty() { String::new() } else { format!("{} is the main character and is in most scenes. ", i.main_name) }),
            ("act_cast", or_text(i.act_cast, "the cast below").to_string()),
            ("setting_name", or_text(i.setting_name, "the setting").to_string()),
            ("spot_min_scenes", i.spot_min_scenes.to_string()),
            ("known_spots", or_text(i.known_spots, "[none yet]").to_string()),
            ("performance_rule", plan_performance_rule(i.video_style).to_string()),
            ("lyric_guidance", i.lyric_guidance.to_string()),
        ],
    );
    format!(
        "{rendered}{own_places}Return JSON only: {{\"scenes\": [{{\"scene\": 1, \"story_slice\": \"...\", \"spot\": \"...\", \"mood\": \"...\", \"performance\": \"singing\", \"continues\": false, \"continuation\": \"...\", \"cast\": [\"...\"], \"performer\": \"...\", \"interaction\": \"...\"}}]}}\n\nAct: {}\n{}\n\nSpot of the scene before this act: {}\n\nCast:\n{}\n\n{}{}Chapter text for context:\n{}\n\nScenes of this act:\n{}",
        i.act_title,
        i.act_summary,
        or_text(i.previous_spot, "[none]"),
        or_text(i.cast_text, "[none]"),
        made_places_block(i.made_places_text),
        user_places_block(places_text),
        i.chapter_text,
        i.scenes_text,
    )
}

/// `story_act_summary_instruction`: rewrite one act's description so it matches the scenes planned for it.
pub fn story_act_summary_instruction(act_title: &str, setting_name: &str, scenes_text: &str, user_text: &str, cast_text: &str, old_summary: &str) -> String {
    let head = with_realism(
        "You are a story editor. The scenes of one act were just planned, and the act's description must now match them.\n\nRules:\n- summary: 2-4 sentences of plain prose that tell what happens across these scenes, in order: who does what, where, and how it ends. Use only people, places and events found in the scenes below or in the user text.\n- The user text (where it happens, what must happen) wins over the old description.\n- No camera or song words, no scene numbers, nobody outside the cast.",
    );
    format!(
        "{head}Return JSON only: {{\"summary\": \"...\"}}\n\nAct: {act_title}\nSetting: {}\n\nUser text:\n{}\n\nCast:\n{}\n\nOld description (replace it):\n{}\n\nPlanned scenes in order:\n{scenes_text}",
        or_text(setting_name, "[not named]"),
        or_text(user_text, "[none]"),
        or_text(cast_text, "[none]"),
        or_text(old_summary, "[none]"),
    )
}

/// `story_setting_looks_instruction`: one shared look per general setting, so every spot inside it matches. No lyrics are sent.
pub fn story_setting_looks_instruction(settings_text: &str, book_summary: &str, style_theme: &str, cast_text: &str, template: Option<&str>, places_text: &str) -> String {
    let own_places = if places_text.trim().is_empty() { String::new() } else { format!("- {}\n", c("USER_PLACES_RULE_FOR_LOOKS")) };
    let template = template.filter(|t| !t.is_empty()).unwrap_or_else(|| c("STORY_SETTING_LOOKS_TEMPLATE"));
    format!(
        "{}{own_places}Return JSON only: {{\"settings\": [{{\"name\": \"...\", \"look\": \"...\"}}]}} with every setting listed below, names unchanged.\n\nStyle and theme notes (look and banned places):\n{}\n\nCast:\n{}\n\n{}Story:\n{book_summary}\n\nSettings:\n{settings_text}",
        render_with_places(template, &[]),
        or_text(style_theme, "[none]"),
        or_text(cast_text, "[none]"),
        user_places_block(places_text),
    )
}

/// `story_spots_instruction`: one location card per spot inside a setting, built on the setting's look. No lyrics are sent.
pub fn story_spots_instruction(setting_name: &str, setting_look: &str, spots_text: &str, style_theme: &str, template: Option<&str>) -> String {
    let template = template.filter(|t| !t.is_empty()).unwrap_or_else(|| c("STORY_SPOTS_TEMPLATE"));
    format!(
        "{}Return JSON only: {{\"spots\": [{{\"name\": \"...\", \"description\": \"...\"}}]}} with every spot listed below, names unchanged.\n\nSetting: {setting_name}\nSetting look: {}\n\nStyle and theme notes:\n{}\n\nSpots and what happens there:\n{spots_text}",
        render_with_places(template, &[]),
        or_text(setting_look, "[none yet]"),
        or_text(style_theme, "[none]"),
    )
}

/// `with_places_rules`: an instruction with the believable-places rule added before its data.
pub fn with_places_rules(text: &str) -> String {
    format!("{}\n- {}\n\n", text.trim_end(), c("BELIEVABLE_PLACES_RULE"))
}

/// `story_place_list_instruction`: the specific places inside each setting that the story uses, before any act or scene exists.
pub fn story_place_list_instruction(settings_text: &str, user_places_text: &str, style_theme: &str) -> String {
    with_places_rules(&format!(
        "You are a music video location scout reading a finished story.\n\nFor each general setting, list the specific places inside it where the story's events happen.\n\nRules:\n- 2-5 places per setting, each 1-3 words with no setting name (e.g. Kitchen, Back Porch, Platform).\n- Only places the story's events need. Every event of the setting must fit one of them.\n- Include every place under 'Places the user wants' that belongs in the setting, named as the user wrote it.\n- A setting with one place inside it lists that one place.\nReturn JSON only: {{\"settings\": [{{\"name\": \"...\", \"spots\": [\"...\"]}}]}} with every setting listed below, names unchanged.\n\nStyle and theme notes:\n{}\n\n{}Settings and what happens there:\n{settings_text}",
        or_text(style_theme, "[none]"),
        user_places_block(user_places_text),
    ))
}

/// The inputs of `story_revise_scene_instruction`.
#[derive(Default)]
pub struct ReviseInput<'a> {
    pub book_text: &'a str,
    pub chapter_number: i64,
    pub chapter_title: &'a str,
    pub setting_name: &'a str,
    pub chapter_text: &'a str,
    pub act_title: &'a str,
    pub act_summary: &'a str,
    pub act_scenes_text: &'a str,
    pub scene_number: i64,
    pub scene_cast: &'a str,
    pub user_note: &'a str,
    pub cast_text: &'a str,
    pub lyric_guidance: &'a str,
    pub template: Option<&'a str>,
}

/// `story_revise_scene_instruction`: change one scene the way the user wants, and adapt the act and chapter around it as little as possible.
pub fn story_revise_scene_instruction(i: &ReviseInput) -> String {
    let template = i.template.filter(|t| !t.is_empty()).unwrap_or_else(|| c("STORY_REVISE_SCENE_TEMPLATE"));
    let rendered = render_with_realism(
        template,
        &[("scene_number", i.scene_number.to_string()), ("cast_rule", story_cast_rule(i.cast_text, false)), ("lyric_guidance", i.lyric_guidance.to_string())],
    );
    let n = i.scene_number;
    format!(
        "{rendered}Return JSON only: {{\"scene\": {{\"story_slice\": \"...\", \"spot\": \"...\", \"mood\": \"...\", \"performer\": \"...\", \"interaction\": \"...\"}}, \"act_summary\": \"...\", \"chapter_paragraphs\": [\"...\"], \"other_scenes\": [{{\"scene\": 3, \"reason\": \"...\"}}]}}\n\nUser's change to Scene {n}:\n- Characters in the scene: {}\n- Direction: {}\n\nCast:\n{}\n\nChapter {}: {}\nSetting: {}\n\n{}\n\nAct: {}\n{}\n\nScenes of this act (scene, spot, story slice, characters):\n{}\n\nWhole story for context:\n{}",
        or_text(i.scene_cast, "[as planned]"),
        or_text(i.user_note, "[none, only the characters changed]"),
        or_text(i.cast_text, "[none]"),
        i.chapter_number,
        i.chapter_title,
        or_text(i.setting_name, "[not named]"),
        i.chapter_text,
        i.act_title,
        i.act_summary,
        i.act_scenes_text,
        i.book_text,
    )
}
