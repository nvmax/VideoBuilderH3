//! Scene context for a fresh prompt: the saved prompts of a scene are left out, so a new prompt is written from the scene's directions and not
//! from an earlier draft. Port of `fresh_prompt_scene_context` and `fresh_prompt_context_text` in `storyboard/session_sync.py`.

use serde_json::Value;

use crate::pyjson;

/// Keys of a scene card that hold a prompt the app or an LLM wrote.
const OUTPUT_FIELDS: &[&str] = &[
    "image_prompt", "t2i_prompt", "text_to_image_prompt", "flux_prompt", "nb_prompt", "ernie_prompt", "flow_gpt_prompt",
    "video_prompt", "i2v_prompt", "t2v_prompt", "minimax_h3_prompt", "minimax_h3_pass2_prompt", "pass2_prompt",
    "video_prompt_origin", "i2v_prompt_origin", "minimax_h3_prompt_origin",
];

/// `value` without its saved prompts, at every depth. The stored card is not changed.
pub fn fresh_prompt_scene_context(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(fresh_prompt_scene_context).collect()),
        Value::Object(map) => Value::Object(map.iter().filter(|(key, _)| !OUTPUT_FIELDS.contains(&key.as_str())).map(|(key, item)| (key.clone(), fresh_prompt_scene_context(item))).collect()),
        other => other.clone(),
    }
}

/// Python's `\s`: what `str.isspace` accepts.
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

const MARKER: &str = "Complete scene_card:";

/// `text` with the saved prompts taken out of every serialized scene card in it (`Complete scene_card:` followed by the card as JSON), as older clients
/// send them in a normal prompt request.
pub fn fresh_prompt_context_text(text: &str) -> String {
    let mut out = String::new();
    let mut offset = 0;
    let mut changed = false;
    let mut from = 0;
    while let Some(found) = text[from..].find(MARKER) {
        let start = from + found;
        let mut end = start + MARKER.len();
        end += text[end..].chars().take_while(|c| is_space(*c)).map(char::len_utf8).sum::<usize>();
        // The next marker is looked for after this one's whitespace, as `re.finditer` does.
        from = end;
        if start < offset {
            continue;
        }
        let mut cards = serde_json::Deserializer::from_str(&text[end..]).into_iter::<Value>();
        let Some(Ok(card)) = cards.next() else { continue };
        if !card.is_object() {
            continue;
        }
        let length = cards.byte_offset();
        out.push_str(&text[offset..end]);
        out.push_str(&pyjson::dumps(&fresh_prompt_scene_context(&card)));
        offset = end + length;
        changed = true;
    }
    if changed {
        out.push_str(&text[offset..]);
        out
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn saved_prompts_are_dropped_at_every_depth() {
        let card = json!({"label": "A", "video_prompt": "x", "nested": [{"minimax_h3_prompt": "y", "keep": 1}], "video_prompt_origin": "gemma"});
        assert_eq!(fresh_prompt_scene_context(&card), json!({"label": "A", "nested": [{"keep": 1}]}));
    }

    #[test]
    fn a_serialized_card_in_a_request_is_cleaned_and_the_rest_is_kept() {
        let card = pyjson::dumps(&json!({"label": "A", "image_prompt": "old", "story_beat": "b"}));
        let text = format!("Notes first.\n\nComplete scene_card:\n{card}\n\nAfter the card.");
        let want = format!("Notes first.\n\nComplete scene_card:\n{}\n\nAfter the card.", pyjson::dumps(&json!({"label": "A", "story_beat": "b"})));
        assert_eq!(fresh_prompt_context_text(&text), want);
        assert_eq!(fresh_prompt_context_text("no card here"), "no card here");
        assert_eq!(fresh_prompt_context_text("Complete scene_card: not json"), "Complete scene_card: not json");
    }
}
