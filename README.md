# VRGDG H3 Studio

Standalone Rust desktop app for MiniMax H3 music videos. It connects to a running ComfyUI. See [PLAN.md](PLAN.md).

## Requirements

- Rust stable (MSVC toolchain), Node 20+, WebView2 (included with Windows 11)
- ffmpeg and ffprobe on PATH, or set `ffmpeg_path` / `ffprobe_path` in the config
- ComfyUI running with the nodes listed by `h3 doctor`

## Commands

```powershell
cargo test --workspace                 # all Rust tests
cargo run -p h3-cli -- doctor          # check ComfyUI, H3 nodes, ffmpeg
cargo run -p h3-cli -- config          # show the config and its location
cd app; npm install; npm run tauri dev # desktop app in dev mode
```

Config: `%APPDATA%\VRGDG-H3\config.json`. Missing keys use defaults (`comfy_url` is `http://127.0.0.1:8188`).

## Render a scene from the command line

```powershell
cargo run -p h3-cli -- scenes --project "D:\Projects\My Song"
cargo run -p h3-cli -- render --project "D:\Projects\My Song" --scene 4 --dry-run --graph-out graph.json
cargo run -p h3-cli -- render --project "D:\Projects\My Song" --scene 4 --set seed=7
```

`--dry-run` builds the graph and the scene audio without queueing anything. `--set KEY=VALUE` overrides one render setting for that run. ComfyUI must be running with the VRGDG and MiniMax nodes.

## Render a whole project

```powershell
cargo run -p h3-cli -- render-all --project "D:\Projects\My Song"
cargo run -p h3-cli -- render-all --project "D:\Projects\My Song" --scope missing
cargo run -p h3-cli -- stitch --project "D:\Projects\My Song" --scene 3 --scene 4
```

`render` and `render-all` cut each clip to its exact length, collect it into `rendered_scene_videos`, record it on the scene and keep a replaced clip as a backup. `render --raw-only` stops after ComfyUI. The media tests need ffmpeg and ffprobe on PATH.

## Agent API

```powershell
cargo run -p h3-cli -- serve                                  # http://127.0.0.1:8190/api/v1
cargo run -p h3-cli -- serve --project-root "D:\Projects"     # projects are the folders in it
```

Same paths, envelopes and error codes as the pack's agent API. The app also has an MCP server of its own at `http://127.0.0.1:8190/mcp` (and `h3-studio --mcp-stdio` for agents that start a program), so an agent needs nothing else: see `docs/MCP.md`, or Menu > MCP Setup in the app. The pack's MCP server still works with `VRGDG_API_URL=http://127.0.0.1:8190/api/v1`. The open Builder shows what an agent does as it happens (an Agent activity panel and live project changes, `docs/MCP.md`). Only this computer may use the API: a request from a web page of another site, or addressed to a host name other than `localhost`, is refused with 403 (`crates/h3-api/src/guard.rs`). Jobs are saved in `jobs.db` next to the config file. A job a stopped app left unfinished shows as `interrupted`, and `POST /jobs/{id}/retry` with `{"resume": true}` renders only the scenes without a clip.

## Image presets

The app generates scene and reference card images in ComfyUI from presets. Three come with the app: `zimage`, `flux_klein` and `ernie`. To use another model, export its workflow from ComfyUI with Workflow > Export (API) and import it:

```powershell
curl -X POST http://127.0.0.1:8190/api/v1/image-presets/import -H "Content-Type: application/json" -d '{"name": "My model", "workflow": { ...the exported workflow... }}'
curl -X POST http://127.0.0.1:8190/api/v1/projects/MySong/scenes/3/image/generate -d '{"preset_id": "my_model", "prompt": "a rainy street"}'
```

The import guesses which node inputs are the prompt, size, seed and models. `PUT /image-presets/{id}` changes the bindings.

## Desktop app

```powershell
cd app; npm install; npm run tauri dev
```

The screen is the Video Builder of the ComfyUI pack, the pack's own front end copied into `app/web/extensions` by `tools/sync_builder_ui.ps1` (run it after the pack's front end changes). The window starts the API itself (`api_bind` in the config, default `127.0.0.1:8190`) and shows the Builder from that address: the server answers the pack's `/vrgdg` routes and the agent API `/api/v1`, and passes the few ComfyUI routes the Builder uses on to ComfyUI.

- The screens are embedded in the program. To work on them, run `cargo run -p h3-cli -- serve --web app\web` and open http://127.0.0.1:8190/ in a browser, the folder is then served from disk.
- Pop-outs (the player, LLM Prompting, the Builder Agent) open as windows of their own that can go to another screen.
- Menu > App Settings holds what ComfyUI keeps in the pack: the ComfyUI address and output folder, the project folder and the ffmpeg paths. They open on their own the first time the app starts.
- `docs/BUILDER_PARITY.md` lists every route the Builder calls and whether the app serves it. Changes to the Builder's own code are the entries of `tools/builder_ui_patches.json` and the files in `app/web/host`.

## Tests against the pack

The ported functions are checked against what the pack's own code returns for the same input. `python tools/make_all.py --pack <pack folder>` makes the cases (`fixtures/`) by running the pack's code with the LLM replaced by a recorder and ElevenLabs by a script of replies, then `cargo test` compares. The cases are made once and kept in the repository.

## LLM

The app writes MiniMax prompts and describes reference pictures with an LLM. In the app, Settings has an LLM section. The default is LM Studio: start its local server and load a model. The app uses that model and never loads or switches one. An own OpenAI compatible server, OpenAI, Anthropic and Google can be used instead.

```powershell
curl http://127.0.0.1:8190/api/v1/llm/active                                  # which model would be used
curl -X POST http://127.0.0.1:8190/api/v1/projects/MySong/minimax-prompts -d "{}"   # prompts for scenes that have none
```
