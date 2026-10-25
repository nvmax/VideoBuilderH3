# MCP server

H3 Studio has an MCP server built in. An AI agent connects to the running app and uses the same projects as the Builder: it can create a project,
write the story and the prompts, render the scenes and stitch the video. Nothing else needs to be installed, and the pack is not needed.

In the app, Menu > MCP Setup shows the address and the exact settings for Claude Code, Claude Desktop, Cursor, LM Studio, VS Code, Codex and
Gemini CLI, with a Copy button and a connection test.

## Connect

| How | Address or command | For |
|---|---|---|
| Streamable HTTP | `http://127.0.0.1:8190/mcp` | agents that connect to a web address (Claude Code, Cursor, LM Studio, VS Code, Codex, Gemini CLI) |
| Standard input and output | `h3-studio.exe --mcp-stdio` | agents that can only start a program (Claude Desktop) |

The port is `api_bind` in the config file. The stdio command is a bridge: it passes each message to the running app, and starts the app first when it is
not running. It opens no window and writes nothing but JSON messages to standard output.

Agents that do not speak MCP can use the REST API under `/api/v1`. `GET /api/v1/openapi.json` is its contract and `GET /api/v1/endpoints.md` a short list.

## What an agent sees

- **126 tools.** 67 named tools with the pack's names and arguments (`project_create`, `story_create`, `minimax_prompts`, `video_render`, `job_wait`, ...),
  58 `api_*` tools for the endpoints the named tools do not cover (ElevenLabs and Speaking, timeline notes, instructions, post processing, ...), and
  `api_request` for any endpoint by path. `api_request` reaches the agent API only, never the app's own setup routes.
- **Resources.** The music video playbook and the chat prompt (`vrgdg://docs/music-video-playbook`, `vrgdg://docs/chat-agent-prompt`), the endpoint list,
  the OpenAPI file, and live ones: projects, modes, a project, a scene, lyrics, a job.
- **Prompts.** `chat_make_music_video` (asks the user for the song, lyrics, character and look, confirms, then builds the whole video),
  `make_music_video`, `review_scene`, `fix_failed_render`, `polish_timeline`.

Long operations return a job id. `job_wait` waits for it. An error says its code and what to try next, and a failed call is marked `isError`.

The LLM is whatever the app is set to use. No tool loads, unloads or switches a model.

## Following the agent

The open Builder shows what an agent does while it does it, so you can watch the work as it happens:

- **The project changes live.** Every save an agent (or a job) makes is announced to the open Builder as `vrgdg.project_changed` with what changed: the scene fields
  (merged in without touching what you have not saved), the character or location of a scene, Timeline Notes, or the project as a whole (the Builder reloads it when
  nothing is unsaved, focused or rendering). Each save has its own event and the next revision, which the Builder's merge needs. The Builder's own saves are not announced
  to itself (`SaveHooks.quiet`).
- **The Agent activity panel** (bottom right, opens with the first activity, and stands above the Builder's own dialogs) lists, as it happens:
  - **That an agent connected** (`Agent connected`, with the client's name and version and the protocol). The button shows `Agent connected: <name>` for 15 seconds, also with
    the panel hidden. A client that says hello with every call is announced once a minute.
  - **Each MCP tool call** while it runs and when it ends, and each REST call a program makes (a call with no `Origin`). Click a row for the details: the API calls the tool
    made (`PATCH /projects/Demo/scenes/seg_1`), what that endpoint does, what it was told (`patch: {label, minimax_h3_prompt}`, `lyrics: 640 characters`), what it answered
    (`revision: 7`), what it changed (`Scene 3: label, MiniMax prompt`, from the saves made while the call ran), how long it took and the client. Keys, tokens and file
    contents never appear.
  - **Reads** such as `scene_list` dimmed, a repeated read as one row (`x3`). The Reads checkbox hides them. Calls that only wait or check (`job_get`, `job_wait`,
    `system_health`) are not listed.
  - **Each job** with its message, a clock that moves every second and, for a job that measures its work, a bar. A render and an image show ComfyUI's own step
    (`Rendering with ComfyUI - Pass 1 sampler step 12 of 30 - about 2:10 left - 4:21 elapsed`), read from ComfyUI's WebSocket (`h3_comfy::progress`). A batch or a full
    video build moves through all its scenes, each scene filling its share. A job that is one call to an LLM has no percent: the number it sets only marks a step, and showing it
    as progress was what looked stuck at 20% and 30%.

  A window that opens later asks for what it missed (`GET /api/v1/agent/activity?after=N`, which also lists the scenes jobs work on now).
- **A scene an agent renders looks like one you render.** The timeline draws it with the rendering glow (blue, purple when it is the selected scene) and its card in the scene list
  carries a badge with the render's progress, until the scene is done or the job fails. A picture an agent makes shows the badge only. The server sends `vrgdg.scene_work`
  for this and the Builder keeps the scenes in a set beside `video_status` (`window.__h3Builder.agentRendering`), so the scene's own data is not changed, merging the
  agent's saves finds no local edit, and the window is not left with unsaved ones. A render started by an agent runs on the server, as before, whether or not a Builder is open.
  It is not a click on the Render button, and the Builder's own Render status log and Stop button do not know about it.
- **Follow** (a checkbox on the panel, on by default, remembered) takes the Builder where the agent goes: it opens the project the agent works in (not while there are
  unsaved edits, a text field in use or a render running) and selects the scene the agent edits or renders, scrolls to it and flashes it. Click a scene in a row to go to it.
  With Follow off the data still updates, and the selection stays where you left it.

The events travel over `/vrgdg/events` (`h3_project::changes` for the saves, `activity.rs` for the feed). The panel is `app/web/host/agent_follow.js`. The Builder gives it a
small handle (`window.__h3Builder`: select a scene, load a project, whether it is busy, which scenes render) through patches in `tools/builder_ui_patches.json`.

## ElevenLabs

The key is saved once in Menu > App Settings > ElevenLabs API key, in the same place as Builder Settings. A Speaking project (`video_type: speaking`) uses its
own key when it has one and this key otherwise. An agent uses the key through the app (voices, voice design, dialogue, speech) and never receives it:
the status route says only whether a key is available and where it comes from.

## Who may call it

Only programs and pages on this computer. `guard.rs` runs before every route (`/mcp`, the REST API, the Builder's `/vrgdg` routes and the ComfyUI pass-through):
a request with an `Origin` that is not this computer (including `null`, which a sandboxed page sends) or a `Host` that is a name other than `localhost`, an
`*.localhost` name or an IP address is refused with 403, so a web page open in a browser cannot drive the server and a rebinding name cannot reach it. CORS answers
only pages of this computer. A request with no `Origin` (an agent, curl, the stdio bridge) passes. There is no token: any program on the computer can connect,
and an agent can create, change and delete projects and start renders.

If `api_bind` is set to an address other than this computer's, reach the app by its IP address: a machine name is refused as a `Host`.

## Keeping it right

`tools/export_mcp_catalog.py` writes `crates/h3-api/src/mcp/` (`catalog.json`, `endpoints.md`, `openapi.json`, `docs/`) from the pack's MCP server and the endpoints
this app serves. Run it with the pack's Python (`..\..\..\python_embeded\python.exe -I tools\export_mcp_catalog.py`) after a route is added or the pack's tools,
playbook or chat prompt change. `crates/h3-api/tests/mcp_api.rs` calls every tool and fails when one reaches a route that does not exist, and
`tests/route_coverage.rs` fails when an endpoint of the pack is neither served nor listed as left out.

Left out on purpose (17 of the pack's 154 endpoints): `POST /llm/unload` (the app never unloads a model), the LTX routes (face fix and first last frame),
and the previews, film grain and color adjust routes.
