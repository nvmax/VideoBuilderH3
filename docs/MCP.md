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

## ElevenLabs

The key is saved once in Menu > App Settings > ElevenLabs API key, in the same place as Builder Settings. A Speaking project (`video_type: speaking`) uses its
own key when it has one and this key otherwise. An agent uses the key through the app (voices, voice design, dialogue, speech) and never receives it:
the status route says only whether a key is available and where it comes from.

## Who may call it

Only programs on this computer. `POST /mcp` refuses a request with an `Origin` that is not this computer and a `Host` that is a name other than
`localhost` (a web page cannot drive the server, and a rebinding name cannot reach it). There is no token: any program on the computer can connect. An agent
can create, change and delete projects and start renders.

The REST API under `/api/v1` still answers any origin (see R8 in `PLAN.md`).

## Keeping it right

`tools/export_mcp_catalog.py` writes `crates/h3-api/src/mcp/` (`catalog.json`, `endpoints.md`, `openapi.json`, `docs/`) from the pack's MCP server and the endpoints
this app serves. Run it with the pack's Python (`..\..\..\python_embeded\python.exe -I tools\export_mcp_catalog.py`) after a route is added or the pack's tools,
playbook or chat prompt change. `crates/h3-api/tests/mcp_api.rs` calls every tool and fails when one reaches a route that does not exist, and
`tests/route_coverage.rs` fails when an endpoint of the pack is neither served nor listed as left out.

Left out on purpose (17 of the pack's 154 endpoints): `POST /llm/unload` (the app never unloads a model), the LTX routes (face fix and first last frame),
and the previews, film grain and color adjust routes.
