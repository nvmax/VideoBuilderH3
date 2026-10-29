# VRGDG H3 Studio

Desktop app that turns a song into a MiniMax H3 music video with ComfyUI. It is a Rust port of the Video Builder of the [comfyui-vrgamedevgirl](https://github.com/vrgamegirl19/comfyui-vrgamedevgirl) node pack by Jean Thompson (VRGameDevGirl) and runs the pack's own Builder screens. It connects to a ComfyUI that you run on the same computer. See [PLAN.md](PLAN.md) for how it is built.

## Download

1. Open the [latest release](https://github.com/nvmax/VideoBuilderH3/releases/latest) and download `H3-Studio_<version>_x64-setup.exe`.
2. Run it. It installs for your user only and needs no administrator rights. Windows SmartScreen may say "Windows protected your PC" because the installer is not signed with a paid code signing certificate. Choose **More info**, then **Run anyway**.
3. Start ComfyUI with the node packs the app needs, then start H3 Studio. The first start opens the setup screen. It checks ComfyUI, the node packs, your folders and ffmpeg, and says what to fix.

It needs Windows 10 or 11 (64 bit) and ComfyUI on the same computer. ffmpeg is included.

### Linux

Download `H3-Studio_<version>_amd64.AppImage` (any distribution: make it executable and run it) or `H3-Studio_<version>_amd64.deb` (Debian, Ubuntu: `sudo apt install ./H3-Studio_<version>_amd64.deb`) from the same page. Both need ffmpeg and a file dialog program (`sudo apt install ffmpeg zenity`), and ComfyUI on the same computer. `docs/BUILDING_LINUX.md` lists the requirements and what differs from Windows.

The app updates itself. A few seconds after it starts (and on demand from Menu, Check for Updates) it looks for a newer release, and it installs one only when you click Install and restart. Updates are signed, and the app refuses one that is not.

## Build it yourself

Needs Rust stable (MSVC toolchain), Node 20 or newer and PowerShell.

```powershell
git clone https://github.com/nvmax/VideoBuilderH3.git
cd VideoBuilderH3
build_installer.bat          # downloads ffmpeg, builds the installer and prints where it is
```

The installer is `target\release\bundle\nsis\VRGDG H3 Studio_<version>_x64-setup.exe`. `docs/RELEASE.md` has the same steps by hand and how a release is published.

On Linux, `bash build_installer.sh` builds the `.deb` and the AppImage (needs Rust stable, Node 20 or newer and the WebKitGTK development packages, see `docs/BUILDING_LINUX.md`).

## License

The app is released under the VRGameDevGirl Community and Commercial License, Version 1.0 (`LICENSE`), the license of the pack it is ported from. It allows free personal, hobby and community use and making videos with the app, including commercial videos. Selling the software, or a service built on it, needs a commercial license from the licensor. Read `LICENSE` for the terms. The bundled ffmpeg (a GPL build, run as a separate program) and the other third party software have their own licenses, listed in `app/src-tauri/licenses/THIRD_PARTY_NOTICES.md`. Files copied or ported from the pack, and the changes made to them, are listed in `COPIED_FROM.md` and `tools/builder_ui_patches.json`.

## Development

- Rust stable (MSVC toolchain on Windows), Node 20+, WebView2 (included with Windows 11) or, on Linux, the WebKitGTK packages of `docs/BUILDING_LINUX.md`
- ffmpeg and ffprobe on PATH, or set `ffmpeg_path` / `ffprobe_path` in the config
- ComfyUI running with the nodes listed by `h3 doctor`

## Commands

```powershell
cargo test --workspace                 # all Rust tests
cargo run -p h3-cli -- doctor          # check ComfyUI, H3 nodes, ffmpeg
cargo run -p h3-cli -- config          # show the config and its location
cd app; npm install; npm run tauri dev # desktop app in dev mode
```

Config: `%APPDATA%\VRGDG-H3\config.json` (Linux: `~/.config/VRGDG-H3/config.json`). Missing keys use defaults (`comfy_url` is `http://127.0.0.1:8188`).

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

## Transitions

Transitions are blocks on the timeline, like overlays. Open **Post Process > Transitions** (next to Overlays), choose **Add a transition...** and it goes on a lane under the overlay lane at the playhead (over the selected range of the timeline, if there is one). Drag the block to move it, drag its edges to stretch it, double-click it to change the transition, and right click for more (center on the nearest cut, duplicate, delete). The Delete key removes the selected block, and undo and redo cover every change. The block snaps to the scene edges, and to the beats with Snap beats on. Blocks cannot overlap.

A block on a cut between two scenes blends the one scene into the next. A block inside a scene is an effect on that scene's footage (a flash, a zoom pulse, a glitch). The player draws the block live while the playhead is inside it, on the real frames of the scenes, and Render All, Export and the stitch draw it into the video on the GPU (DirectX 12 or Vulkan, the software adapter when there is none). There are 143 transitions: 18 of the app's own, in the style of the popular short-form editors (zoom pulls, whip pans, spin blur, pulse pop, stretch, camera shake, RGB glitch, digital tear, VHS tracking, TV off, color flash, light leak, blur dissolve, chromatic fade), and the 125 of the [gl-transitions](https://gl-transitions.com) collection (swirl, cube, ripple, glitch displace, page curl, wipes, cross zoom and more). The picker has search, a tag filter, a preview on the real frames of the scenes around a cut, the length and the transition's own settings, which can be changed later in the Transitions tab.

A transition replaces the same number of frames of the video, so the video keeps the timeline's length and the song stays in step. Where a scene has no frames left, its first or last frame is held still. A block that reaches over more than one cut is cut down to the cut nearest its middle.

The app's own transitions are `crates/h3-transitions/transitions/*.glsl`, one GLSL file each (the header and the `uniform` lines are described at the top of `src/custom.rs`). The preview runs the file in WebGL and the final render translates the same file to WGSL, so adding a file adds a transition to both. The transition screens are the pack's own front end (`web/music_video_builder/transition_*.mjs`, copied by `tools/sync_builder_ui.ps1`). This app turns them on by answering `GET /vrgdg/music_builder/transitions/capabilities` and serves the catalog with the GLSL at `GET /vrgdg/music_builder/transitions`. Without that answer (the plain pack) the lane, the tab and the preview stay out of sight and a project's `scene_transitions` are kept as saved. The window rule is tested against the pack's `tests/transition_window_cases.json` (`fixtures/cases/`, copied by `tools/make_all.py`).

For agents: `GET /transitions`, `GET/PUT /projects/{pid}/transitions`, `POST /projects/{pid}/transitions/set`, and `transitions` in the stitch request (all listed at `/api/v1/endpoints.md`). They are saved in the project as `scene_transitions`, a list of `{id, transition, start, seconds, params}`. A project saved with the earlier one-per-cut entries (`after_scene`, `align`) opens with each one as the block it stood for.

## Effects and filters

Effects and filters are blocks on the timeline too, each kind on a lane of its own under the transition lane. Open **Post Process > Effects** or **Post Process > Filters**, choose **Add an effect...** or **Add a filter...** (or **Whole video...** to put one over the whole timeline), and pick from a grid of the looks drawn on a real frame of the scene at the playhead, grouped by category with search. The block goes at the playhead (over the selected range of the timeline, if there is one). Moving, stretching, snapping, the menu, the Delete key and undo work as for transitions. Each block has a strength (0 to 100%, how much of the change is mixed in) and the look's own settings, changed in the tab. Blocks may overlap and stack in rows. On a frame every filter that covers it is applied first, then every effect, each in the order of the timeline, after the transitions and before the overlays. A look changes the picture and never the number of frames, so the song stays in step. The player draws the looks live while the playhead is inside a block, and Render All, Export and the stitch draw them into the video on the GPU.

There are 78 effects (Glitch, Camera, Blur, Distort, Light, Color, Retro, Stylize, Frame: RGB split, shake, zoom pulse, glow, bloom, flash, CRT, VHS, halftone, kaleidoscope, edge detection and more) and 126 filters (Cinematic, Film, Retro, Black & White, Warm, Cool, Night & Moody, Vivid, Pastel & Soft, Portrait, Food, Landscape, Music Video, and `Adjust`, the neutral grade). A filter is one parametric grade (exposure, contrast, saturation, temperature, tint, highlights, shadows, fade, gamma, a split tone, a mono mix, vignette, grain, sharpen), so there are no LUT files to ship. All of them are the app's own work, named after what they look like and not after any product. There is no `.cube` import in the Filters lane: a LUT clip of the Overlays tab does that.

The looks are `crates/h3-transitions/looks/effects/*.glsl` and `looks/filters/*.glsl`, one GLSL file each with `vec4 effect(vec2 uv)` that reads the picture with `getColor(uv)` and may use `progress` (0 to 1 over the block), `ratio` and `time` (seconds since the block started). The header, the `// @param` lines and the `uniform` lines are as for transitions, plus `// Category:`. `looks/prelude.glsl` holds the helpers every look can call. The preview runs the file in WebGL and the final render translates the same file to WGSL, so adding a file adds a look to both. The screens are the pack's own front end (`web/music_video_builder/look_*.mjs`, copied by `tools/sync_builder_ui.ps1`). This app turns them on by answering `GET /vrgdg/music_builder/effects/capabilities` and `.../filters/capabilities` and serves the catalogs with the GLSL at `GET /vrgdg/music_builder/effects` and `/filters`. Without that answer (the plain pack) the lanes, the tabs and the preview stay out of sight and a project's `video_effects` and `video_filters` are kept as saved. The frame rule is tested against the pack's `tests/look_frame_cases.json` (`fixtures/cases/`).

For agents: `GET /effects`, `GET /filters`, `GET/PUT /projects/{pid}/effects` and `/filters`, `POST /projects/{pid}/effects/set` and `/filters/set`, and `effects` and `filters` in the stitch request (all listed at `/api/v1/endpoints.md`). They are saved in the project as `video_effects` and `video_filters`, lists of `{id, look, start, seconds, intensity, params}`.

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
