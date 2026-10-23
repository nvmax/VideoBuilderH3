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
