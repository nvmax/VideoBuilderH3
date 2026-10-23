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
