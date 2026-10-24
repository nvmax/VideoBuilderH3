# VRGDG H3 Studio (Rust): Implementation Plan

Status: approved 2026-10-22 (answers in Section 10). P0 and P1 done 2026-10-22. P2 done 2026-10-23. P3 done 2026-10-23. P4 done 2026-10-23 (M1 reached, see Section 14). P5 done 2026-10-23 (M2 reached, see Section 15). P6 done 2026-10-23 (see Section 16). P6b done 2026-10-23 (see Section 17). P7 done 2026-10-23 (see Section 18). P8 done 2026-10-23 (see Section 19, story steps left out by decision F60). Next: P9. Everything stays local: no remote, no push.
Reference pack commit: `14421d2` (branch `newbuilder`).
Location: `rustapp/` inside the pack folder, excluded from git through `.git/info/exclude` (no tracked file changed).

## 1. Goal

A standalone Windows desktop app, written in Rust, that does what the Video Builder does for **MiniMax H3 only**: song in, finished music video out. It connects to a running ComfyUI over HTTP and WebSocket and queues the same H3 graphs the pack queues today. No LTX, no Z-Image/SDXL video paths, no canvas nodes.

The pack is read-only reference material. Nothing in it is edited. The app never imports, links or reads pack files at runtime.

## 2. How the current project works (what we replicate)

```
Browser UI (85k lines of .mjs)  ->  pack routes (aiohttp in ComfyUI)  ->  graph compiler (runner/)  ->  ComfyUI /prompt
        |                                   |                                                          |
   session.json edits              services: project, audio, media,                         VRGDG + 3rd-party nodes
   undo/redo, timeline             ffmpeg trim/collect/stitch, LLM                           write video + latents
```

One scene render (from `agent_api/orchestrator/video_orchestrator.py:131`):

1. Resolve effective H3 settings (project settings, scene lock overrides) -> `minimax/settings_payload.py`.
2. Resolve scene inputs: reference images in order, continuity frames, last frame -> `minimax/scene_inputs.py`.
3. Optional: write the prompt from the previous scene's final frame through the LLM.
4. Pick the project audio (or the Audio Mask mix) and timing (warm-up, cool-down, token math) -> `minimax/latent_manager.py:303 calculate_minimax_h3_timing`.
5. Load a JSON template from `Workflows/UsedForUIDoNotTouch/minimax_*.json` and patch it (models, LoRAs, sampler, resolution, attention, TE-Speed, latent save/load, masked continuation) -> `runner/minimax_workflows.py`, `runner/minimax_patches.py`, `runner/minimax_inputs.py`.
6. `POST /prompt`, follow `/ws`, read `/history`, take the video from node `142`.
7. Trim to the exact timeline length (`minimax_exact`), collect as `rendered_scene_videos/video_NNNN-audio.mp4` with backup and thumbnail -> `runner/video_files.py`.
8. Record on the segment (`video_path`, `video_history`, thumbnail, `video_cache_bust`, continuity used) -> `agent_api/scene_video.py`. Mark the next scene's latent dirty.

Full video = steps 1-8 for every scene in order (one GPU job at a time), then ffmpeg stitch with the song.

## 3. Decisions (confirmed)

- **D1. Shell: Tauri 2.** Rust backend owns all logic, state, files, jobs and ComfyUI traffic. The window is a WebView2 page that only renders and sends commands. Reason: the timeline needs `<video>` playback, scrubbing and waveform canvas, which WebView2 does natively. Doing that in egui/iced means writing a video player.
- **D2. UI language: TypeScript + SolidJS, thin (Q1).** No business logic in the UI.
- **D10. Any image model (Q4).** Image generation uses ComfyUI API workflows registered as image presets. A preset is a workflow JSON plus a binding map (prompt, negative, seed, width, height, steps, reference images, output node). The pack's Flux Klein, Z-Image, Krea2, Ernie and NanoBanana templates ship as built-in presets. Users add their own model by importing an API-format workflow and binding its inputs in the app. Generated images feed reference cards (subjects, locations) and I2V start/last frames.
- **D11. Agent API (Q6).** `h3-api` serves the `/api/v1` contract (`agent_api/openapi.json`) on localhost so the existing `mcp_server` and agents connect by changing `VRGDG_API_URL`. Built alongside the pipeline (from P6), not at the end.
- **D12. Public release (Q8).** Code signing, an LGPL ffmpeg build bundled as a sidecar with its license notice, a first-run node checker, and an update channel. Release work waits until the app is feature complete. Until then everything stays local.
- **D3. Same machine as ComfyUI in v1.** Latents and many graph inputs are absolute paths inside the project folder, so ComfyUI must see the same paths (same finding as `Apireport.md` 25.9).
- **D4. Graphs from templates, ported patchers.** Copy the H3 templates into `rustapp/templates/` and embed them with `include_str!`. Port the Python patch functions as typed Rust functions over `serde_json::Value`. Parity is proven with golden graphs dumped from the Python code (P1).
- **D5. Session file compatible with the Video Builder.** Same `vrgdg_builder_session.json` keys and folder layout, unknown keys preserved on save. Any project opens in both apps. This also gives free test data: your existing projects. See Q3.
- **D6. External tools as processes.** `ffmpeg`/`ffprobe` as a bundled sidecar (no `ffmpeg-next` linking). Audio decode and peaks in Rust (`symphonia`).
- **D7. One GPU job at a time.** A tokio job queue with SQLite persistence, cancel via ComfyUI `/interrupt`, recovery after restart.
- **D8. Never touch the LLM model.** LM Studio is read through `GET /api/v0/models` only. No load, unload or switch, same as `agent_api/llm_runtime.py`.
- **D9. Atomic writes everywhere.** Temp file, `fsync`, rename (Rust twin of `core/atomic_write.py`).

## 4. Architecture

```
+--------------------------------------- rustapp (Tauri 2 exe) ---------------------------------------+
|  UI (WebView2, TS)  <-- Tauri commands / events -->  Rust backend                                     |
|                                                       h3-pipeline  (render scene, render all, stitch)  |
|                                                       h3-jobs      (queue, SQLite, progress events)    |
|                                                       h3-graph     (templates + patchers -> /prompt)   |
|                                                       h3-core      (pure H3 math, settings, prompts)   |
|                                                       h3-project   (session model, atomic IO, undo)    |
|                                                       h3-media     (ffmpeg/ffprobe, audio, beats)      |
|                                                       h3-llm       (LM Studio, OpenAI, Anthropic, ...)  |
|                                                       h3-comfy     (HTTP + WS client, preflight)       |
|                                                       h3-api       (optional axum /api/v1 for MCP)     |
+-------------------------------------------------------------------------------------------------------+
          | HTTP /prompt /history /queue /interrupt /object_info /system_stats /upload /view, WS /ws
          v
   ComfyUI (running) + required node packs   ---   LM Studio / cloud LLMs / ElevenLabs (HTTP)
```

Crates: `tokio`, `reqwest`, `tokio-tungstenite`, `serde`/`serde_json`, `rusqlite`, `tempfile`, `symphonia`, `rustfft`, `image`, `tracing`, `thiserror`, `axum` (only for h3-api), `keyring` (API keys in Windows Credential Manager).

## 5. Folder layout

```
rustapp/
  PLAN.md                 this file
  COPIED_FROM.md          every file copied or ported: pack path, commit, date, changes
  Cargo.toml              workspace
  crates/
    h3-core/              pure, no IO: resolution, tile plan, timing, token math, masked context plan,
                          settings normalize + scene overrides, scene inputs, shot prompt build/validate
    h3-project/           session model (serde + preserved extra keys), atomic IO, renumbering,
                          timeline journal, undo history, revision check
    h3-comfy/             client, WS progress, history parsing, /object_info preflight, fake client
    h3-graph/             template loader, node lookup by class, patchers, compat inputs
    h3-media/             ffmpeg/ffprobe wrapper, trim exact, collect, thumbnail, final frame,
                          color match, stitch, audio decode, peaks, beats
    h3-llm/               providers, JSON repair (twin of llm/output_checks.py), instructions
    h3-jobs/              queue, SQLite store, events, cancel, restart recovery
    h3-pipeline/          scene render, render all, build full video, story/prompt steps
    h3-api/               local /api/v1 for agents (from P6)
    h3-cli/               `h3` command line: doctor, config, later render, render-all, stitch
  app/
    src-tauri/            Tauri entry, commands, event bridge
    ui/                   TypeScript UI
  templates/              copied H3 workflow JSONs
  fixtures/               golden graphs, payloads, prompts, sample sessions (generated in P1)
  tools/                  dev-only scripts that generate fixtures from the pack (never shipped)
```

## 6. What we take from the current project

| Area | Pack source | Rust target | How |
| :--- | :--- | :--- | :--- |
| H3 templates | `Workflows/UsedForUIDoNotTouch/minimax_*.json` (7 files) | `templates/` | Copy |
| Graph compile | `runner/minimax_workflows.py`, `minimax_patches.py`, `minimax_inputs.py`, `minimax_keyframes.py`, `minimax_refmod.py`, `api_graph.py` (H3 parts) | `h3-graph` | Port, golden tests |
| Settings | `minimax/settings_payload.py`, `h3_settings_defaults.json` (115 keys), `minimax_h3.mjs` migrations | `h3-core::settings` | Port, payload fixtures |
| H3 math | `minimax/resolution.py`, `tile_plan.py`, `latent_manager.py` (timing, token math), masked context plan | `h3-core` | Port, table tests |
| Scene inputs | `minimax/scene_inputs.py`, `refmod_scene.py` | `h3-core::inputs` | Port, `refmod_scene_cases.json` |
| Prompts | `minimax/shot_prompt.py`, `prompt_assembly.py`, `lyric_free_performance.py`, `minimax_prompt.mjs` validation, `llm/prompts/minimax.py`, `story_mode.py`, `storyboard.py` | `h3-core::prompt`, `h3-llm` | Port, `minimax_craft_cases.json` |
| Latent bookkeeping | `minimax/latent_manager.py` (dirty flags, predecessor, rename on reorder) | `h3-project::latents` | Port |
| Project IO | `builder/project.py` (new, load, save, renumber), `project_copy.py`, `timeline.py`, `timeline_markers.py`, `agent_api/session_keys.py` | `h3-project` | Port |
| Audio | `builder/audio.py` (peaks, beats, convert, trim), `audio_clips.py`, `audio_stems.py` (mask mix) | `h3-media` | Port, beat tolerance tests |
| Video files | `runner/video_files.py` (trim exact, collect, backup, thumbnail, color match, stitch) | `h3-media` | Port |
| Lyrics to scenes | `builder/lyric_scenes.py`, `agent_api/orchestrator/lyrics_orchestrator.py` | `h3-pipeline` | Port |
| Orchestration | `agent_api/orchestrator/video_orchestrator.py`, `pipeline_orchestrator.py`, `minimax_prompt_orchestrator.py`, `reference_orchestrator.py`, `storyboard_orchestrator.py` | `h3-pipeline` | Port |
| Jobs | `agent_api/jobs/` | `h3-jobs` | Re-implement same states |
| ComfyUI client | `agent_api/orchestrator/comfy_client.py` (incl. fake client) | `h3-comfy` | Port |
| LLM | `llm/builder_runner.py` (HTTP runners only), `llm/api.py`, `agent_api/llm_runtime.py`, `llm/output_checks.py`, `text_cleaning.py` | `h3-llm` | Port HTTP paths, drop GGUF/local loaders |
| API contract | `agent_api/openapi.json`, `endpoints.json` | `h3-api` | Implement the subset, contract tests |
| UI behavior | `web/music_video_builder/` timeline, inspector, `minimax_panel*`, `reference_*`, `batch_render.mjs`, `video_render.mjs` | `app/ui` | Rebuild, use as spec only |

Not taken: everything LTX, Z-Image/SDXL/Krea/Ernie video paths, `optional_nodes/`, canvas nodes, browser automation, Face Fix, storyboard window as a separate app, GGUF local LLMs, wizard.

## 7. What ComfyUI must have

The graphs call these nodes. The app checks them at startup with `/object_info` (names and input names) and lists what is missing.

| Provider | Nodes |
| :--- | :--- |
| ComfyUI core (0.34+, you run 0.42) | `MiniMaxH3ReferenceToVideo`, `MiniMaxH3ImageToVideo`, `ResolutionSelector`, `ComfyMathExpression`, `EasyCache`, samplers, loaders, `LTXVAudioVAEDecode` |
| VRGDG pack | `VRGDG_MiniMaxH3AudioDrive`, `VRGDG_MiniMaxH3ReferenceMediaFromPaths`, Save/Load Latent, Apply Masked Continuation, latent upscaler nodes, `VRGDG_MiniMaxH3SpatialTilePlan`, `H3FastVAEDecode`, `VRGDG_MiniMaxH3KeyframeTiming`, `VRGDG_TextBox` |
| comfyui-kjnodes | `DiffusionModelLoaderKJ`, `ImageResizeKJv2`, `MiniMaxH3MemoryEfficientSageAttentionPatch` |
| VideoHelperSuite | `VHS_LoadAudio`, `VHS_VideoCombine` |
| comfyui-minimax-h3-audio-T8 | `MiniMaxH3AVLatentSeparateT8` |
| TE-Speed-MiniMaxH3-OSS | `TESpeedMiniMaxH3` |
| Comfyui-MMH3-UltimateUpscale | `MMH3UltimateUpscale`, `MMH3SpatialSplitParams`, `MMH3TemporalSplitParams` (2 Pass Advanced) |
| ComfyUI core and comfyui-kjnodes | `BlockSparseAttention` (core), `MiniMaxChunkFeedForward` (kjnodes), optional model patches |
| ComfyUI-MiniMaxH3Mod | RefMod text encode (RefMod pipeline only) |

The VRGDG nodes are Python and must run inside ComfyUI. Rust cannot replace them. See Q2.

## 8. Phases

Each phase ends with a check you can run. Order matters: pure logic and parity first, UI after the CLI can already render.

| Phase | Work | Done when |
| :--- | :--- | :--- |
| **P0 Scaffold** | Install Rust (MSVC), Tauri CLI, Node; workspace and crates; config file (ComfyUI URL, project root, ffmpeg path); `tracing` logs with `[VRGDG-RS]` prefix; `COPIED_FROM.md`. | `cargo test` passes on empty crates; `h3 doctor` pings ComfyUI `/system_stats` and lists missing nodes from Section 7. |
| **P1 Fixtures** (done) | Dev-only Python in `tools/` runs against the pack read-only. `tools/make_all.py` regenerates: 84 graph cases (78 graphs, 6 expected errors), 16 core tables (timing, token math, masked context, resolution, tile plan, settings, payload), 10 scene input tables, 22 prompt tables, the pack's 7 case files, a node snapshot from the live ComfyUI and a types-only shape of 20 real sessions. Output is identical on every run. | `cargo test` loads every fixture (`h3-core/tests/fixtures_load.rs`) and every node class in every graph is in the preflight list. |
| **P2 Pure core** | `h3-core`: resolution, tile plan, token math, timing, masked context plan, settings normalize/migrate/scene override, scene inputs, shot prompt build/parse/validate, 7,000-character budget, lyric `<d>` tagging. | All fixture and case tests pass. |
| **P3 Project** | `h3-project`: session model with preserved unknown keys, atomic save, revision check, new project scaffold, renumber after insert/remove with journal rollback, latent dirty bookkeeping, undo history. | Load and save 5 of your real projects with zero diff except `revision`; renumber tests match the pack's. |
| **P4 First render (CLI)** | `h3-comfy` + `h3-graph`: template load, all patchers, queue, WS progress, history, output pick (node `142`). Single pass and 2 Pass, Reference to Video, input audio. | Rust graphs equal the golden graphs; `h3 render --project X --scene 3` produces the same clip the Builder makes with the same seed. **Milestone M1.** |
| **P5 Media** | `h3-media`: ffprobe, exact trim, collect with backup and thumbnail, final frame, color match, stitch with song, audio decode, peaks, beats, scene audio cut. | `h3 render-all` + `h3 stitch` produce a full video from an existing project. **Milestone M2.** |
| **P6 Jobs and API** | `h3-jobs` + `h3-pipeline`: queue, SQLite, cancel/interrupt, restart recovery, render all with latent continuation order, predecessor checks, randomize seeds, timeouts. `h3-api` starts here: system, projects, scenes, settings, jobs, render and stitch routes from the `/api/v1` contract. | Kill the app mid-render, restart, job shows `interrupted`, resume finishes the rest. The existing `mcp_server` lists projects and renders a scene against the app. |
| **P6b Image presets** | Preset store, workflow import (API format), input binding editor data model, built-in presets copied from the pack templates, image jobs, image history per scene and per reference card. | Generate a character image with Flux Klein and with one imported custom workflow, and use both as H3 references. |
| **P7 UI MVP** | Project list/new/open, audio import with waveform, timeline (scene blocks, resize, split, merge, insert, delete, snap to beats), scene inspector, H3 settings panel, reference cards (import images, map to scenes), prompt editor with validation, render queue with live progress, preview player, final video panel. | You build a short video end to end in the app without the Builder. **Milestone M3.** |
| **P8 LLM and story** | `h3-llm`: LM Studio (loaded model only), own OpenAI-compatible server, OpenAI, Anthropic, Gemini; JSON repair; character describe, location extract, assign scenes, story settings/arc/brief/beats, MiniMax prompt generation, prompt from previous final frame; lyrics: paste lines, ElevenLabs Scribe word timing, `timeline/from-lines`. | The `MUSIC_VIDEO_AGENT_PROMPT.md` flow runs inside the app from song + lyrics + character image. |
| **P9 Full H3 coverage** | 2 Pass Advanced (tile plan), Image to Video Normal/FLF, RefMod pipeline, built-in audio, Audio Mask with Demucs stems (ComfyUI graph), per-scene locked settings, takes and re-trim, LUT via ffmpeg `lut3d`. | Golden graphs for every mode pass; each mode renders once on your machine. |
| **P10 Full API and release** | Rest of the `/api/v1` contract with contract tests against `openapi.json`; Tauri installer (NSIS), code signing, LGPL ffmpeg sidecar, first-run setup (ComfyUI URL, project root, node check), update channel, optional slim companion node pack. | A fresh Windows user account installs and renders against a ComfyUI install. |

Start with P0, P1, P2. They have no UI and no GPU work, and they catch most porting mistakes early.

## 9. Risks

- **R1. Node input drift.** A node input rename in ComfyUI or a pack breaks graphs silently. Mitigation: startup preflight compares graph inputs with `/object_info`; `COPIED_FROM.md` records the pack commit; re-run P1 fixtures on pack updates.
- **R2. UI size.** The Builder UI is 129 modules and 85k lines. Rebuilding all of it is the largest cost. Mitigation: MVP UI in P7 covers only the H3 path; everything else waits for a request.
- **R3. Beat detection parity.** Closed: the librosa path is ported with `rustfft` (`beats.rs`). On five fixture songs it gives librosa's onset strength, tempo and beat times exactly, and on five real songs the same beat counts and tempos with 99.4% to 100% of beats within one frame (23 ms), the rest differing by the resampler. `tools/compare_beats.py` repeats the check on any song.
- **R4. Two apps on one project.** Both write the same session file. Mitigation: revision check before every save (same as `builder_save_revision`/`revision`), refuse stale writes.
- **R5. Prompt and LLM parity.** Prompt code is large (`shot_prompt.py` 1,255 lines, `minimax_prompt.mjs` 2,966 lines) and LLM output varies. Mitigation: case-file tests for deterministic parts; LLM steps are tested for valid structure, not identical text.
- **R6. Exact trim.** Frame-accurate trimming depends on ffmpeg arguments. Mitigation: port arguments exactly and compare frame counts with `ffprobe` against Builder outputs.
- **R7. Two codebases.** A fix in the pack must be ported by hand. Mitigation: `COPIED_FROM.md` and fixture regeneration show what changed.

## 10. Answers (2026-10-22)

- **Q1.** Tauri with a web UI (TypeScript). D1, D2.
- **Q2.** Use the installed VRGDG pack's H3 nodes for now. Companion pack is optional in P10.
- **Q3.** Keep the Video Builder session format. D5.
- **Q4.** Generate from any image model the user picks. D10, P6b.
- **Q5.** v1 scope agreed: Reference to Video, Single and 2 Pass, input audio, Latent Continuation Masked. Other modes in P9.
- **Q6.** Yes, expose `/api/v1` for agents. D11.
- **Q7.** Own local git repository in `rustapp/`, no remote.
- **Q8.** Public. D12.
- **Q9.** Install whatever is needed. Keep everything local.

## 11. Findings from P1 (they change how later phases are built)

- **F1. `three_pass` is 2 Pass Advanced.** The saved `render_pass: "three_pass"` runs the tile-plan graph (`MMH3UltimateUpscale`). The older fixed 3 pass builder (`_build_minimax_h3_3pass_api_prompt`, nodes `PT_H3ConcatAVLatent`, `MiniMaxH3AVDecodeT8`, LayerStyle) can only be reached by naming its mode and the Video Builder never does. The app drops it. Two `legacy_*` fixtures keep it on record.
- **F2. Settings normalize before graphs.** `normalize_minimax_h3_settings` migrates and clamps every value. I2V needs `i2v_pass_settings_version: 1` to keep 2 Pass, and its default LoRA is the FL2V turbo (`minimax_h3_fl2v_turbo_4step_v1.1_768p_comfyui_bf16`). Saves without the version fall back to Single (`i2v_legacy_save_migrates_to_single`).
- **F3. Seeds.** A pass seed below 0 means random in the pack, so golden graphs use fixed seeds on every pass.
- **F4. Output folder names hash the project path.** `VRGDG_MiniMaxH3/<name>_<sha1(normcase(path))[:8]>/scene_NNNN`. The Rust builder must compute the same hash on Windows paths.
- **F5. Real sessions hold API keys.** `llm_api_key_project`, `elevenlabs_api_key_project`, `lm_studio_api_key`, `own_server_api_key_project` and `nb_image_settings.api_key` sit inside `vrgdg_builder_session.json`. The session model must keep them untouched on save. The app stores its own keys in the Windows credential store and never writes new ones into a session.
- **F6. Session size.** 127 top-level keys and about 1,000 distinct segment key paths across 20 sessions (`fixtures/session_shape.json`). P3 uses a typed core with a flattened extra-keys map so unknown keys survive a round trip.
- **F7. Real project round-trip test stays local.** P3 reads real sessions from a path given by the `H3_REAL_PROJECTS` environment variable, never from committed files, because the repo will become public.
- **F8. Masked continuation inputs.** The graph needs the predecessor latent header (`token_count`, `tail_padding_frames` from `scene_NNN.latent.json`). The limits are real: a 192 frame context needs a predecessor of about 241 frames or more, and the mode is refused in 2 Pass Advanced, I2V and Image + Reference.


## 12. P2 findings

- **F9.** Python's decimal context keeps 28 significant digits and rounds after every operation. The pack depends on it: a context of 4.0416..67 s times 24 comes out a hair above 97, so its ceiling is 98 frames. `timing.rs` repeats that rounding after each step and matches all 233 timing cases.
- **F10.** Fixture inputs that name a plan, item list or scene (`plans`, `items`, `places`, `cue_segments`) now come with `fixtures/prompt/named_inputs.json`, written by the generator, so the Rust tests build the same objects.
- **F11.** Prompt patterns use lookahead and lookbehind, so `h3-core` uses `fancy-regex`. Python slices strings by character and Rust by byte, so every cut in the port goes through characters.
- **F12.** Deferred from P2 to the phase that needs them: `minimax/prompt_assembly.py` (the agent API's cut plan from duration and frequency, and its context builder) goes to P6 and P8. `schema`, `attention summary` and seed randomizing in `settings_payload.py` go to P10.
- Test count after P2: 39 fixture tests in `h3-core`, covering 233 timing, 156 masked context, 360 scene input and 1,038 prompt cases. A deliberate typo in one pack sentence made the prompt test fail, so the string checks are real.

## 13. P3 findings

- **F13.** The Python Builder writes sessions as `json.dumps(indent=2, ensure_ascii=False)` plus a newline. `h3-project` writes the same bytes. Of 20 real sessions, 16 are byte identical after a Rust round trip, the same count Python's own dump gives. The other 4 are older saves with CRLF line endings, a byte order mark or no final newline. Loading accepts all of them and saving writes the current form.
- **F14.** Two things broke byte equality until the real projects exposed them. `serde_json` needs the `float_roundtrip` feature, or it parses some floats one step off. Python also breaks an exact tie between two shortest spellings toward the even digit, where Rust rounds up (`0.18913650512695312` against `...313`). `pyjson.rs` formats again with exact rounding at the shortest length. Both cases are fixtures now.
- **F15.** A save changes only `revision`, `updated` and `project_folder` in a real session, and keeps every key and its order. Tested on all 20 projects with `H3_REAL_PROJECTS`. The test never prints session text.
- **F16.** The save writes the same derived files as the pack: `builder_segments.srt`, `SceneNotes.json` (the pack falls back to it for an empty timeline note, so a stale copy would bring an old note back), `prompts/*.txt`, `full_lyrics.txt`, the reference descriptions folder and `MINIMAX_PROJECT_FILES.md`.
- **F17.** Left to later phases: copying the audio into `project_audio/` on first save (the caller passes the audio path), `project_context_files`, the global model defaults file, moving a project folder (`_rehydrate_builder_session`), and `_save_builder_project_as`.
- **F18.** The pack has no project level undo. The Rust app uses the pack's own `session_backups/` (newest 25) as the history. `restore_backup` backs up the current session first and raises the revision.
- **F19.** One pack bug is not ported: a non-object entry in `overlay_segments` makes the pack's save raise. The Rust save skips such an entry.

## 14. P4 findings

- **F20.** The graph builders in `h3-graph` reproduce all 82 golden graphs and errors exactly, number kinds included (`1` is not `1.0`). They reach the outside world only through the `GraphEnv` trait: files, the node list and model lists from ComfyUI, ffprobe and ffmpeg, the saved latents. The app implements it with `LiveEnv`, the tests with the fixtures' fake ComfyUI. Changing one value in the builder (`multiple` 32 to 33) made 28 of 82 cases fail, so the comparison is real.
- **F21.** ComfyUI reports `main.py` as a relative path (`ComfyUI\main.py` in the portable build), so its output folder cannot come from the command line alone. `ComfyDirs::detect` anchors it with the absolute `custom_nodes` folder from `/internal/folder_paths`, and `comfy_output_dir` in the config overrides it.
- **F22.** ComfyUI history carries `[name, data]` event pairs in `status.messages`, also on success. The pack ignores them. A failed node only appears there (`execution_error`), so the Rust client reads that one pair and reports the node, its type and the first 600 characters of the reason.
- **F23.** Live check on a scratch copy of one of your projects (only its session file; nothing in your projects was touched): scene 4, 2.46 s, 2 Pass, seed 69. `h3 render` built a 40 node graph, cut the scene audio, ComfyUI rendered it in 110 s (48 cached nodes on the second run), and the clip was found. The clip has 73 frames at 24 fps (17 n + 5), 1920x1088. The pack's save latent nodes wrote the final latent, the first pass latent and the take archive into the scratch project, with `tail_padding_frames` 14 = 73 - 59 as the plan says. The first attempt failed in the sampler because that project still names an old T5 text encoder, which does not fit the MiniMax model. A per run `--set clip_name=...` fixed it. The Builder would fail the same way.
- **F24.** M1 is reached for the render itself. What is not done: cutting the clip to the exact 59 frames (`minimax_exact`), collecting it as `video_NNNN-audio.mp4` with backup and thumbnail, recording it on the scene, and marking the next scene dirty. They are the first items of P5, so the "same clip the Builder makes" comparison happens there.
- **F25.** Left out on purpose: the RefMod pipeline (clear error), the final frame prompt writer (needs the LLM, P8), the Audio Mask override, speaking clips, WebSocket step progress (the client polls `/history` once a second like the pack; the job system adds live steps).

## 15. P5 findings

- **F26.** The media layer matches the pack on 19 cases built from small synthetic clips (test pattern video, sine audio): exact trim (frame counts and lengths), collect with backup and thumbnails, final frame, opening color match, and 14 stitch layouts (project song, timeline sync with uneven frames, preview window, inserts, canvas change, embedded audio in `continuous_song` and `per_scene` mode, audio that runs short, silent clip, no song, a second output name). For stitches and trims the decoded audio is byte identical to the pack's output, which checks the embedded audio track (frame exact boundaries, FFT song matching, crossfades) sample by sample.
- **F27.** Windows starts a program from the application's own folder first, and ComfyUI's portable `python_embeded` folder holds its own `ffmpeg.exe`. The pack's Python therefore runs that ffmpeg, while the Rust app uses the one on PATH (or the configured one). Both gave the same frames and lengths, but encoded audio differs between builds. The generator now names one ffmpeg explicitly and records its version, and the Rust test compares decoded audio only when the version is the same.
- **F28.** Pipeline decisions match the pack on 24 stitch layouts, 20 scene recordings and 72 continuity records: which scenes a stitch uses, audio mode (auto, embedded, project), the 24 fps timing with frame aligned preview offsets, the history and thumbnail lists of a scene, and the continuity a render really used.
- **F29.** Live M2 on a scratch project of three consecutive scenes from one of your projects (single pass, 287 s): `h3 render-all` rendered, trimmed to exactly 91, 59 and 71 frames, collected as `video_NNNN-audio.mp4`, recorded the clips and their history on the scenes (revision 1 to 4), wrote the take archive, and stitched `FINAL_VIDEO.mp4` with 221 frames, the timeline's total. Saves keep the project openable in the Python Builder.
- **F30.** A stitch of part of the song without `--scene` muxes the whole song, as the pack does. Use `--scene` for a preview, which cuts the song to the selection.
- **F31.** Left for P7 and P9, where the UI needs them: waveform peaks, beat estimation, scene audio mixes and silent audio (`builder/audio.py`), speaking clips, the Audio Mask override, and the LTX paths.

## 16. P6 findings

- **F32.** `h3-jobs` keeps jobs and their logs in SQLite (`jobs.db` next to the config file). One GPU job runs at a time, other jobs run beside it. Statuses, progress fields, ids (`job_YYYYMMDD_HHMMSS_xxxx`) and log numbering match the pack's job model. A job a stopped app left `queued` or `running` comes back as `interrupted` with code `JOB_INTERRUPTED`. Retry makes a new job with the same parameters, and `resume: true` adds `run_mode: resume_missing`, which renders only the scenes without a clip.
- **F33.** Cancel needs no hook into ComfyUI. The job's cancel flag is the render's flag, and the render's wait loop already interrupts ComfyUI and removes its queue item. A queued job is cancelled at once. After a hard kill of the app ComfyUI keeps rendering the orphaned prompt, since the app is gone. The job record does not hold the prompt id yet, so the restarted app cannot interrupt it. Interrupt it in ComfyUI or add the id to the render events later.
- **F34.** `h3-api` serves `/api/v1` on `127.0.0.1:8190` with the pack's envelopes, error codes and SSE format. Routes so far: health, meta, models, queue, events, jobs (list, get, log, cancel, retry), projects (list, get, summary, settings get and patch with `If-Match`), scenes (list, get), scene render, batch render, video graph, stitch, latents (list, dirty, one, delete). The pack's `mcp_server` works against it with `VRGDG_API_URL=http://127.0.0.1:8190/api/v1`. The rest of the contract is P10.
- **F35.** Live check, scratch copy of a 3 scene project: the real `mcp_server` listed projects and scenes and started a batch render over stdio. The app was killed while scene 2 rendered. After a restart the job was `interrupted`, scene 1 kept its clip, and a retry with `resume` rendered scenes 2 and 3 only, then stitched. Clips are 91, 59 and 71 frames, the exact timeline lengths. The test renders in ComfyUI's output folder were deleted.
- **F36.** Two bugs found by that run. A bare `ffmpeg` in the config counted as a found program and failed mid-render with "program not found", now `Tools::find` looks bare names up on PATH and checks paths, so health and `doctor` fail early. And the pack ends a batch job as `succeeded` even when every scene failed. The app fails it with `BATCH_FAILED` (retryable) and the per scene errors in `details`.
- **F37.** `randomize_seed` is implemented for single and batch renders: fresh values for the five seed settings, saved to the project (or to the scene when its settings are locked) before the render, so it can be repeated.
- **F38.** The latent predecessor checks report `PREDECESSOR_MISSING` and `LATENT_STALE` (HTTP 409) as in the pack. `render_all` still runs scenes in timeline order, which is the latent continuation order.

## 17. P6b findings

- **F39.** An image preset is a ComfyUI API workflow plus bindings. A binding names what the app fills in (`prompt`, `seed`, `width`, `reference_images`...) and lists the node inputs it writes. Kinds: text, int, float, bool, choice (model files), seed (a negative value means random), images (a list of files), image (one file, copied into ComfyUI's input folder), loras (up to 20). A binding can also run graph changes when it has a value and when it has none (switch an image to image branch on and off, take the reference nodes out of the graph). New models need no code: import the workflow and edit its bindings.
- **F40.** The three built-in presets (Z-Image Turbo 2 pass, Flux Klein with reference images, Ernie Image Turbo) use the pack's own templates and the pack's setting names, so `image_model_mode` values carry over. 14 cases built by the pack's `_patch_*_api_prompt` functions match the app's graphs exactly. The first run of that comparison found that the pack's LoRA nodes have 20 slots, not 5. Krea2 (two stage, derived aspect ratio) and NanoBanana (cloud API key) are not built in yet.
- **F41.** Import accepts the API format only and says how to export it when it gets the editor format. It guesses bindings: model loaders, the positive and negative prompt (found through the sampler links), image size and batch, seed, steps, cfg, LoadImage nodes, and the output nodes. A missing custom node class is reported by name before anything is queued.
- **F42.** Scene images follow the pack's lifecycle: a generated image is archived under `scene_image_previews/scene_NNNN/preview_<stamp>.png`, joins `image_history`, and clears the approved and custom image. Approve, revert and clear match the pack. A reference card picture is saved as `subject_location/subject/<card name>.png` with the 220 px JPEG preview the Builder keeps on the card.
- **F43.** Live check on the scratch project: Flux Klein generated a new character picture from the card's old picture as a reference (768x768), and an imported Z-Image workflow (standard nodes, built outside the app) generated the scene 1 location picture (1024x576). Both are on their cards, the H3 graph for scene 1 contains both files, and scene 1 rendered with them. The test files in ComfyUI's output folder were deleted.
- **F44.** Presets live in `image_presets/` next to the config file, one JSON file each. The API is `/image-presets` (list, get, import, update, delete, choices, graph dry run), scene image routes as in the pack, and two routes the pack does not have: `POST /projects/{pid}/references/{kind}/{rid}/image/generate` and `PUT .../image` for a card picture.

## 18. P7 findings

- **F45.** The scene operations are ports of the pack's mutations and match them on 28 cases that compare the whole project folder and the session text, key order included: insert, delete, split, merge, move, resize, bulk timeline (durations, ranges, markers, replace and append, lyrics carried over), close gaps. The timeline rules (normalize, ripple, gaps, snap, validate, bulk text) and the lyric rules (split, merge, carry over, scene length fixes) match on 450 more cases, and the scene card fields (49 patches, both video engines) match as well. Edits run in a journal, so a failure puts files and session back.
- **F46.** The app does not yet do what the pack's loader does on open (`_rehydrate_builder_session`): re-point paths after a project folder moved, and link loose media files to scenes by number. The scene operation fixtures turn it off in the pack so they test the operations alone. A project built by this app stays in one folder, so it does not matter there. It matters for a project moved on disk, and for a Builder project whose session lost its media paths. Port it before release.
- **F47.** Audio: the song is copied to `project_audio/` and read through ffmpeg for the waveform peaks (the pack uses torchaudio for those, so the numbers differ slightly). The beats and the tempo come from `beats.rs`, a port of the pack's librosa path (mono 22,050 Hz, mel onset strength, autocorrelation tempo, Ellis's beat tracker, onset backtrack). The loudness peaks estimate stays as the fallback, as in the pack. An earlier version used only that estimate, which marked few and irregular beats.
- **F48.** The UI is a SolidJS single page that talks only to `/api/v1`, the same one agents use, so the window and outside tools share one code path. The desktop shell starts the API inside the app. Panels: project list, song with waveform, timeline (select, resize by dragging, split at the playhead, merge, add, delete, close gaps, bulk text), scene form with the prompt editor and length check, settings (curated groups and every setting, model lists from ComfyUI), characters and locations (picture upload, generate with a preset, use in every scene, per scene mapping), render queue with live progress, resume and retry, the final video player. It was driven in Chrome over the debugging port: creating a project, adding a song, bulk text, label and prompt edits with the check, split at the playhead, resizing by dragging, merge, a character with a picture used in every scene, and a settings change all work. That run found and fixed four bugs: the scene view had no label, the waveform was stretched over a 10 s minimum, the settings groups overlapped, and a prompt check was cleared by the reload after saving. Fast consecutive edits sent a stale revision (409), so edits now run one after another.
- **F49.** Whole flow through the API, no Builder: create a project, attach an 8 s song, make 2 scenes from text, make a character picture with Z-Image, map it to both scenes, write prompts, single pass, render all and stitch. 192 frames, 8.000 s, as the timeline says. The test files in ComfyUI's output folder were deleted.
- **F50.** Not done in P7: the MiniMax prompt checks beyond empty and the 7,000 character limit (reference capacity and format rules come with the prompt writer in P8), timed lyric notes, silent audio, per scene audio settings, duplicate and export, and the Storyboard cards, which this app does not have, so card only fields are refused with a clear message.

## 19. P8 findings (partly done)

- **F51.** `h3-llm` talks to LM Studio through the model that is already loaded. It reads `/api/v0/models` (read only), names a loaded model in the request, sizes the request to that model's context and sends nothing when none is loaded, because a request for a model that is not loaded makes LM Studio load it. A test against a stand-in server checks that no chat request is ever sent when nothing is loaded, and that the loaded model wins over the saved name. Other runners: an own OpenAI compatible server, OpenAI, Anthropic and Google (text; pictures only for LM Studio, own server and OpenAI). Settings are saved for every project in the config file, keys are never shown by the API.
- **F52.** The MiniMax prompt writer is a port of the pack's `generate_scene_prompt` for reference to video scenes. 24 final prompts (plain and structured format, cut frequencies, video styles, custom style, FX accents, temporal effects, built-in audio, an instrumental scene) are identical to the pack's for the same session and the same LLM reply. The wording around the LLM call (instructions, acting direction, reply cleaning, repeat and template checks) matches the pack's text on 80 more cases. Where it differs on purpose: the request leaves out the Storyboard card JSON the pack adds, because this app has no Storyboard, so the LLM sees the same scene facts but not that block.
- **F53.** Not written yet, and refused or noted: timed cue maps (one lyric block is used and a warning says so), the "omit lyrics from video prompts" option (an error that says to turn it off), and scenes written from the previous scene's final frame (a scene with latent continuation is written as a standalone scene and a warning says so).
- **F54.** Describing a picture (character, location, prop, face, extra) sends the picture to the loaded vision model with the pack's instruction for that kind, cleans the answer (name instead of "the character", skin words removed, 100 words), refuses repeated or template text, and asks once more, calmer, before it gives up. The routes are `POST /projects/{pid}/minimax-prompts` and `POST /projects/{pid}/references/{kind}/{rid}/describe`, both jobs that do not wait behind a render, plus `/llm/settings`, `/llm/active`, `/llm/models` and `/llm/test`.
- **F55.** No LM Studio was running here, so nothing was run against a real model. The tests use a stand-in server. The first live run needs LM Studio started with a model loaded.
- **F56.** Left in P8: location extract, assigning characters and locations to scenes by pattern, the story steps (settings, arc, brief, scene beats), prompts from the previous final frame, lyric timing (ElevenLabs Scribe or the Stable-ts workflow) and `timeline/from-lines`.
- **F57.** Assign Scenes is done: `POST /projects/{pid}/references/assign-scenes` with a dry run, and a control in the References tab. Rotate, blocks and unchanged match the pack on 165 cases. Random is not the same sequence as Python's for a given seed, which cannot matter because the plan is random by purpose.
- **F58.** Lyric timing is done: `POST /projects/{pid}/lyrics/align` and `POST /projects/{pid}/timeline/from-lines` (jobs that wait behind GPU work, like the pack's). Scenes made from timed lines match the pack on 448 cases and the timestamp graph on 6. The route test uses a stand-in ComfyUI, so the real Stable-ts nodes (`VRGDG_TimestampedLyricsExtractor`) have not been run from this app yet. A scene that is split while enforcing lengths uses the default `[instrumental]` text for an empty half even when a custom instrumental text was asked for, because the shared split helper does not take that text. An empty `audio_path` gives a clear message here where the pack reports the working folder.
- **F59.** Location extract is done: `POST /projects/{pid}/references/locations/extract` (an LLM job). It ran live against a real model (an own OpenAI compatible server, Qwen 3.8 flash) on a scratch project and added 22 locations, and the earlier prompt writing and picture description also ran live on that model (F55 is closed for the own server runner). LM Studio itself has still not been run.
- **F60.** Decision: the story steps stay out. Arc, brief, scene beats and Story Mode write with the Storyboard Builder (about 3,000 lines of prompts and card logic) and only reach the MiniMax prompt through Storyboard scene cards, which this app leaves out. The story settings (scene defaults and the story idea) are saved in the Builder's own keys, so projects stay compatible, and `POST story/{step}` answers 501 `NOT_SUPPORTED` with a message that says why.
- **F61.** Cue maps and continued scenes are done. A scene with a timed cue map is cut at each cue and every shot gets its singer's words. A scene continued with masked latent continuation is written as the next moment of the previous take. With `continuity_prompt_from_last_frame` the real last frame of the previous clip is extracted with ffmpeg and shown to the LLM, right before the scene renders (a hook in the render context, so a batch writes each scene after the one before it finished). A model that cannot read pictures falls back to the previous scene's last shot as text and says so in the warnings. This replaces F53 except for omit-lyrics.
- **F62.** Omit lyrics from video prompts is done, so the MiniMax prompt writer covers everything the pack's reference to video writer does. With the project option on and the song supplied, shots describe singing in sync with the audio and never quote words or mouth movement, per singer and per timed cue. Final prompts match the pack on 64 cases in all. F53 is closed. Still not ported: the pack's Storyboard card JSON in the request, which this app has no use for. Real prompts differ from the pack's only where an LLM is free to differ.
- **F63.** P9 started with takes and re-trim: `GET .../video/takes`, `POST .../video/trim` and `match-start-color` (jobs that do not wait behind a render), `POST .../video/select` and `recover`, `DELETE .../video`. The scratch folder is found from ComfyUI's output folder, so listing takes needs ComfyUI running or `comfy_output_dir` in the config. Left out for now: `video/scan` (re-linking clips after a project move) and `minimax-stage-recover`.
- **F64.** LUT grading is done: `GET /post/luts`, `POST /post/luts/upload` (raw body with `?filename=` or JSON with base64 `data`), `POST .../post/lut` and `POST .../post/apply-all` (jobs). LUTs live in a `luts` folder next to the config file. Not in this app: LUTs on scene images, previews, film grain, color adjust and face fix. Grain and adjust run on torch in the pack and are not H3 features.
- **F65.** More of `/api/v1`: timeline notes (`GET/POST .../timeline/notes`, `PATCH/DELETE .../notes/{nid}`), `POST .../timeline/enforce-length` (with `dry_run`), `GET/PUT .../story` and `POST .../settings/preflight`.
- **F66.** `POST .../audio/silent` (project scope) and `POST .../duplicate` are in. Left out on purpose: Speaking mode and ElevenLabs (`speaking-audio`, scene `audio-settings`, `dialogue`, `elevenlabs` routes), because they belong to the Speaking video type, not to music videos.
- **F67.** The RefMod pipeline is in: `GET /refmods` lists the saved RefMods (read from ComfyUI's `refmods` folders, `models/refmods` and the MiniMaxH3Mod pack's `mods`), and a project whose MiniMax setting `pipeline` is `refmod` renders each scene from the RefMod cards mapped to it, with `<Video n>`, `<Picture n>` and `<Audio 1>` labels. It works with Single and 2 Pass, like the pack's. It has not been rendered on this machine, because it needs the separate MiniMaxH3Mod node pack and some saved RefMods. Making new RefMods (RefMod Studio) is not in this app.

- F68: Audio Mask renders. An enabled mask renders with its saved masked_mix.wav and the finished clip gets the real song audio back. Building stems stays in the Video Builder.

- F69: P9 route gaps left for P10: prompts/context, prompts/minimax/assemble (and the full prompt validator), minimax/cleanup, minimax/index, video/scan, minimax-stage-recover, per-scene locked settings. They are authoring helpers for outside agents. The app writes prompts itself with minimax-prompts.

- F70: Pipelines. The build job never replaces an existing prompt (the pack's fresh_rebuild overwrites it with the lyric line). It fills a missing one with the lyric line like the pack. The images stage uses the project's image preset. A full unattended build is not run end to end yet (needs the GPU go-ahead). Film grain, adjust, face fix, speaking mode, ElevenLabs, instruction presets and the LLM prompt routes (concepts, motion notes, image/video/enhance/edit/batch) are not in this app yet.

- F71: Image prompt writer. One neutral instruction for scene stills, character sheets and locations. A hint line sets the text style for Z-Image, FLUX or neither (from the preset id or mode). Routes: POST scenes/{sid}/prompts/image, POST scenes/{sid}/prompts/video (writes the H3 prompt), POST prompts/batch (image or video), POST references/{kind}/{rid}/prompt. Image generation jobs accept write_prompt: true. Checked live on the own-server LLM. Still out: concepts, motion notes, enhance, edit, video-chained (LTX) and the instruction presets.

- F72: Installer and first run. NSIS per-user installer (95.5 MiB) built with npm run release (tauri.release.conf.json, ffmpeg and ffprobe as sidecars from tools/fetch_ffmpeg.ps1). The app finds the bundled ffmpeg first. GET /setup/status, POST /setup/check and PUT /setup/config drive the Setup screen (ComfyUI, node packs by provider, output folder, project folder, ffmpeg, libx264, LLM). Saved settings apply after a restart. D12 changed: libx264 needs the GPL ffmpeg build, bundled as a separate program with a license and source offer. Not done: code signing, update channel (docs/RELEASE.md), a clean machine install test. The release exe was started once against this machine's ComfyUI and reported all checks ok.

- F73: Model lists. POST /llm/models lists the models of the settings typed on the screen without saving them (LM Studio: loaded only, own server and OpenAI: GET /models, Anthropic and Google: their model lists). A key left out is taken from the saved settings. GET /llm/models still answers for the saved runner. The LLM panel has a Find models button and fills the model field from a list, and looks up the saved runner when it opens.

- F74: Built-in Gemma. Runner comfyui_gemma queues a graph with the pack's VRGDG_SuperGemmaGGUFChat node (task_preset custom, the whole instruction as custom_instructions) and a PreviewAny, waits on /history and reads the text. Models and mmproj files are the .gguf files the node lists from ComfyUI's models/LLM folder (GET /object_info/VRGDG_SuperGemmaGGUFChat), so no model name is typed. Pictures are uploaded to ComfyUI (/upload/image) and passed as image1..N for the describe and frame prompts. The model leaves the GPU after each answer unless comfy_gemma_keep_loaded is set. No JSON schema constraint (the node cannot), same as the pack's built-in runner. The request waits in ComfyUI's queue behind a running render. Checked against this machine's ComfyUI: a short test answer and a picture description both came back. The LM Studio rule (never load or switch) is unchanged: it applies to LM Studio only.

- F75: Existing Video Builder projects. The Builder keeps its projects directly in ComfyUI's output folder. The app now reads projects from its own project folder and from ComfyUI's output folder (the configured comfy_output_dir, else the one the running ComfyUI reports at start). New projects are still made in the project folder. On the first run the setup suggests the output folder as the project folder. Checked read-only against this machine: all 20 projects listed with their scene counts. UI: the app's screens are the thin D2 UI, not a copy of the Builder's look.

- F76: UI in the Builder's layout, from the user's builder.png (Pictures folder). Header bar (Menu, project, Auto Build, Ref Builder, LLM Runner, Render Queue, All Settings, health, MiniMax H3, Stop, Setup), scene cards on the left (Scenes, Tools, Post Process), a large preview with the song scrub in the middle, Image, Video and Audio tabs on the right (pipeline, mode and pass buttons, Models, Video Settings, Scene, Prompt, and the cyan Create MiniMax H3 Scene Video button), and the timeline along the bottom (ruler, thumbnail clips, waveform, button column). Colors from the picture: zinc 950 background, panels 202024, cyan 06b6d4, amber 451a03 scene cards. Same API as before, no logic moved. Not checked in a browser yet.

## 21. P11: the Builder's own front end (decision F77, replaces the thin UI of F76)

The user wants the app to look and work like the Video Builder in the ComfyUI pack, button for button. A hand made UI can
not get there, so the app now runs the pack's own front end: `web/music_video_builder`, `web/storyboard_builder` and the
top level scripts it imports, copied unchanged by `tools/sync_builder_ui.ps1` into `app/web/extensions/comfyui-vrgamedevgirl`.

- **Host.** `app/web/scripts/api.js` and `app.js` stand in for ComfyUI's (the Builder uses `api.fetchApi`, a client id, one
  server event and `app.registerExtension`). `app/web/host/boot.js` starts `openBuilder` with a stand-in node that keeps its
  four widget values in browser storage. The old SolidJS screens, Vite and the Node build are gone. Tauri embeds `app/web`.
- **Server.** `h3-api` answers the pack's `/vrgdg/...` routes (`crates/h3-api/src/vrgdg`) with the pack's request and reply
  shapes. A route not built yet answers 501 naming itself and is logged (`[VRGDG-RS Builder] Not built yet`). Paths that are
  not `/vrgdg` go to ComfyUI (the Builder uses `/prompt`, `/history`, `/queue`, `/view`, `/interrupt`). The agent API is also
  served at `/vrgdg/api/v1`. `h3 serve --web app/web` serves the screens too, so a browser can open the Builder.
- **Inventory.** `tools/inventory_builder_routes.py` lists the 232 pack routes, their handlers and which front end modules
  call them (`fixtures/api/builder_routes.json`, `docs/BUILDER_ROUTES.md`). LTX, face fix, Speaking mode and the browser
  automation routes are out. Everything H3 uses stays.
- **Order.** M1 start up and open a project, M2 timeline editing, M3 render path, M4 LLM and prompts, M5 reference builder
  and RefMods, M6 audio mask and lyrics, M7 Story Mode, M8 post process, M9 pop out windows and the desktop shell.
- **Proof.** Each route is checked against the live pack (ComfyUI at 8188 answers the same request on a scratch copy) and in
  Chrome next to the real Builder.

### P11 status

- **Routes.** Every route the Builder front end calls is served, or left out on purpose (LTX, the wizards, browser automation of other sites): 182 served, 53 left out, 0 to build (`docs/BUILDER_PARITY.md`, made by `tools/parity_report.py`). Speaking mode is in (ElevenLabs voices and speech, Voice Design, dialogue and voice description helpers, scene audio settings, Auto-Generate Scenes), so are the Story Layer (brief, arc), the Builder Agent, motion notes and the Gemma4 drafts.
- **Proof.** Each ported function has golden cases made by running the pack's own code (`tools/make_*_fixtures.py`, listed in `tools/make_all.py`). The LLM is a recorder, ElevenLabs is a script of replies. The Rust code is compared with the instruction text it asked for, the sampling values, the reply and the error text. Known differences: error texts of Python's JSON parser (cases left out), the built-in runner is ComfyUI's Gemma (the pack loads a GGUF itself).
- **Window.** The desktop window opens the API server's own address, which serves the embedded Builder (`crates/h3-api/src/embedded.rs`), so the page, its media and the API share one origin, like the Builder inside ComfyUI. `window.open` makes real windows (the player pop-out, the LLM Prompting pop-out, the Builder Agent), a link to another site opens in the system browser. App Settings (ComfyUI address, folders, ffmpeg) is in the Menu and opens on first start.
- **Front end differences from the pack** are the entries of `tools/builder_ui_patches.json` and the files in `app/web/host`. The sync copies the Face Fix and Wizard Beta scripts too, because the Builder imports them and a missing import stops it.
- **Checked in the real window** (Tauri, WebView2, scratch copies of two projects, ComfyUI on this machine): the layout against `builder.png`, ctrl multi-select, the scene right-click menu, ctrl + wheel zoom, every header dialog, the player and LLM pop-outs as windows of their own, Space playing the scenes one after another with the next video preloading, Create Story Brief through the own server LLM (the project's brief changed), and one 2 pass Ref to Video render of a scene (the video came back at the exact scene length, h264 and aac, and the old video went to the backup folder).
- **Installer.** `npm run release` makes `VRGDG H3 Studio_0.1.0_x64-setup.exe` (98.9 MiB, per user). A silent install into a scratch folder started, passed every setup check with the bundled ffmpeg and showed the Builder and the projects. The uninstaller left no folder, Start Menu entry or registry entry. This was this machine, not a clean one.
- **Still to do.** A test of the installer on a clean machine, code signing, a squashed git history without the author's address before the repository is public, the update channel, real GPU render tests per mode, native 6 stem Demucs. Beats are tracked natively like the pack's librosa path (`beats.rs`, checked on five songs and five fixture songs).
