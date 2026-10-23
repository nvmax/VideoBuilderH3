# VRGDG H3 Studio (Rust): Implementation Plan

Status: approved 2026-10-22 (answers in Section 10). P0 and P1 done 2026-10-22. P2 done 2026-10-23. P3 done 2026-10-23. Next: P4. Everything stays local: no remote, no push.
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
- **R3. Beat detection parity.** `librosa` onset and tempo have no exact Rust twin. Mitigation: port the onset envelope with `rustfft` and accept a tolerance (for example 20 ms); or import beats from the pack's saved session.
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
