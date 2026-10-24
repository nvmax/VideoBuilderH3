//! What the MCP server tells an agent besides its tools: documents (resources) and ready-made instructions (prompts). The documents are the pack's own
//! playbook and endpoint list, written for this app by `tools/export_mcp_catalog.py`.

use serde_json::{json, Map, Value};

pub const PLAYBOOK: &str = include_str!("docs/music_video_playbook.md");
pub const CHAT_PROMPT: &str = include_str!("docs/chat_agent_prompt.md");
pub const ENDPOINTS: &str = include_str!("endpoints.md");
pub const OPENAPI: &str = include_str!("openapi.json");

pub const INSTRUCTIONS: &str = "VRGDG H3 Studio. These tools drive the same projects the app's Video Builder edits, so a project made here opens there as if it was built by hand.\n\
To make a music video while chatting with the user, get the prompt `chat_make_music_video`: it asks the user what they want, confirms, then builds everything. If you already have all the details, get `make_music_video` (or read resource `vrgdg://docs/music-video-playbook`) and follow it step by step.\n\
Resource `vrgdg://docs/endpoints` lists every API endpoint and what it does, and `vrgdg://docs/openapi` is the full contract. Every endpoint has a tool: the named tools first, `api_*` tools for the rest, and `api_request` for any endpoint by path. Call `llm_active` before any step that writes text: the LLM is whatever the app is set to use, and you must never load, unload or switch a model.\n\
Long operations (LLM steps, renders, the stitch) return a job id. Call `job_wait` until the job is succeeded or failed. Run one GPU job at a time.";

/// The documents an agent can read: uri, name, type, description, text.
const DOCUMENTS: [(&str, &str, &str, &str, &str); 4] = [
    ("vrgdg://docs/music-video-playbook", "Music Video Playbook", "text/markdown", "Step-by-step instructions for building a finished music video with the tools, with a form for the song, lyrics, character and style.", PLAYBOOK),
    ("vrgdg://docs/chat-agent-prompt", "Chat Agent Prompt", "text/markdown", "Instructions for an agent talking with a user: it asks for the song, lyrics, character and style, confirms, then builds the whole video.", CHAT_PROMPT),
    ("vrgdg://docs/endpoints", "API Endpoints", "text/markdown", "Every endpoint of the agent API this app serves, and what it does. The tools call these.", ENDPOINTS),
    ("vrgdg://docs/openapi", "OpenAPI Contract", "application/json", "Machine-readable contract: paths, parameters, request bodies and error codes.", OPENAPI),
];

pub fn resources() -> Vec<Value> {
    let mut list: Vec<Value> = DOCUMENTS.iter().map(|(uri, name, mime, description, _)| json!({"uri": uri, "name": name, "description": description, "mimeType": mime})).collect();
    list.push(json!({"uri": "vrgdg://projects", "name": "Projects", "description": "The projects of this app.", "mimeType": "application/json"}));
    list.push(json!({"uri": "vrgdg://modes", "name": "Modes", "description": "What this app can render.", "mimeType": "application/json"}));
    list
}

pub fn templates() -> Vec<Value> {
    vec![
        json!({"uriTemplate": "vrgdg://project/{project_id}", "name": "Project", "description": "The full state of a project.", "mimeType": "application/json"}),
        json!({"uriTemplate": "vrgdg://project/{project_id}/scene/{scene_id}", "name": "Scene", "description": "One scene of the timeline.", "mimeType": "application/json"}),
        json!({"uriTemplate": "vrgdg://project/{project_id}/lyrics", "name": "Lyrics", "description": "The lyrics of a project.", "mimeType": "text/plain"}),
        json!({"uriTemplate": "vrgdg://jobs/{job_id}/log", "name": "Job", "description": "The status of a job.", "mimeType": "text/plain"}),
    ]
}

/// A document by uri: its type and text.
pub fn document(uri: &str) -> Option<(&'static str, &'static str)> {
    DOCUMENTS.iter().find(|d| d.0 == uri).map(|d| (d.2, d.4))
}

pub fn prompts() -> Vec<Value> {
    let argument = |name: &str, description: &str, required: bool| json!({"name": name, "description": description, "required": required});
    vec![
        json!({"name": "make_music_video", "description": "Guided workflow from audio track to final stitched video with checkpoints.", "arguments": [
            argument("project_name", "Name for the project", true), argument("audio_file", "Path to the music audio file", true), argument("lyrics", "Full lyrics text", false),
            argument("character_name", "Name of the main character", false), argument("character_image", "Path to the character image", false),
            argument("location_style_theme", "Where and what look, for example 'Los Angeles nightlife, neon, night time'", false), argument("story_idea", "One or two sentences to build the story from", false)]}),
        json!({"name": "chat_make_music_video", "description": "Chat with the user: ask for the song, lyrics, character and style, confirm, then build the whole video.", "arguments": []}),
        json!({"name": "review_scene", "description": "View the scene's picture, compare it with its prompt and propose edits.", "arguments": [argument("project_id", "Project ID", true), argument("scene_id", "Scene ID to review", true)]}),
        json!({"name": "fix_failed_render", "description": "Inspect a failed job, find the cause and propose or run the fix.", "arguments": [argument("job_id", "ID of the failed job", true), argument("project_id", "Project ID", true)]}),
        json!({"name": "polish_timeline", "description": "Look at scene durations, find gaps and short scenes, and suggest merges or snaps.", "arguments": [argument("project_id", "Project ID to inspect", true)]}),
    ]
}

fn arg(args: &Map<String, Value>, key: &str, default: &str) -> String {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(default).to_string()
}

fn message(description: &str, text: String) -> Value {
    json!({"description": description, "messages": [{"role": "user", "content": {"type": "text", "text": text}}]})
}

/// A prompt by name with its arguments filled in, or `None` for a name this server does not have.
pub fn prompt(name: &str, args: &Map<String, Value>) -> Option<Value> {
    match name.trim().to_lowercase().as_str() {
        "make_music_video" => {
            let form = [
                "Fill-in values for this project:".to_string(),
                format!("- project_name: {}", arg(args, "project_name", "MyMusicVideo")),
                format!("- audio_file: {}", arg(args, "audio_file", "")),
                format!("- character name: {}", arg(args, "character_name", "(not given)")),
                format!("- character image file: {}", arg(args, "character_image", "(not given)")),
                format!("- location style theme: {}", arg(args, "location_style_theme", "(not given)")),
                format!("- story idea: {}", arg(args, "story_idea", "(not given; write one from the lyrics and theme)")),
                "- lyrics:".to_string(),
                arg(args, "lyrics", "(not given)"),
            ]
            .join("\n");
            Some(message("Make Music Video Pipeline", format!("{form}\n\n---\n\n{PLAYBOOK}")))
        }
        "chat_make_music_video" => Some(message("Chat Make Music Video", CHAT_PROMPT.to_string())),
        "review_scene" => Some(message(
            "Review Scene",
            format!(
                "Please review scene '{}' in project '{}'.\n1. Fetch the scene with `scene_get`.\n2. Look at its picture with `asset_view` (give project_id and scene_id).\n3. Compare what you see with the scene's prompt (`minimax_h3_prompt`, or `t2i_prompt` for the still).\n4. If it needs work, write a better prompt and save it with `scene_update`, or improve it with the LLM (`api_post_scenes_prompts_enhance`).",
                arg(args, "scene_id", ""),
                arg(args, "project_id", "")
            ),
        )),
        "fix_failed_render" => Some(message(
            "Fix Failed Render",
            format!(
                "Investigate failed job '{}' in project '{}'.\n1. Read the job with `job_get` and its log with `api_get_jobs_log`.\n2. If the error is PREDECESSOR_MISSING or LATENT_STALE, render the predecessor or call `latents_status_rebuild` with action 'rebuild'.\n3. If the render left a usable take, `video_recover` (or `video_select_take`) brings it back.\n4. Otherwise change the settings with `project_update_settings` and call `job_retry`.",
                arg(args, "job_id", ""),
                arg(args, "project_id", "")
            ),
        )),
        "polish_timeline" => Some(message(
            "Polish Timeline",
            format!(
                "Inspect and polish the timeline of project '{}'.\n1. Call `scene_list` and read the start and end of every scene.\n2. Find scenes shorter than 1.0 s and gaps between scenes.\n3. Fix them with `scene_split_merge_move_resize` (op merge, move or resize), `api_post_timeline_close_gaps` or `api_post_timeline_snap`.",
                arg(args, "project_id", "")
            ),
        )),
        _ => None,
    }
}
