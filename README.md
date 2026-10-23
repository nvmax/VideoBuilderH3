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

Same paths, envelopes and error codes as the pack's agent API. The pack's MCP server connects with `VRGDG_API_URL=http://127.0.0.1:8190/api/v1`. Jobs are saved in `jobs.db` next to the config file. A job a stopped app left unfinished shows as `interrupted`, and `POST /jobs/{id}/retry` with `{"resume": true}` renders only the scenes without a clip.

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

The window starts the API itself (`api_bind` in the config, default `127.0.0.1:8190`). For the UI alone in a browser, run `cargo run -p h3-cli -- serve` and `npm run dev` in `app`, then open http://localhost:1420. Add `?api=http://host:port/api/v1` to use another address.

## LLM

The app writes MiniMax prompts and describes reference pictures with an LLM. In the app, Settings has an LLM section. The default is LM Studio: start its local server and load a model. The app uses that model and never loads or switches one. An own OpenAI compatible server, OpenAI, Anthropic and Google can be used instead.

```powershell
curl http://127.0.0.1:8190/api/v1/llm/active                                  # which model would be used
curl -X POST http://127.0.0.1:8190/api/v1/projects/MySong/minimax-prompts -d "{}"   # prompts for scenes that have none
```
