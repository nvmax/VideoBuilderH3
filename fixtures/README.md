# Fixtures

Generated from the pack (read-only) by the scripts in `../tools`. Do not edit by hand. Each Rust port in `h3-core`, `h3-graph` and `h3-pipeline` is tested against these files.

| Folder | Contents | Made by |
| :--- | :--- | :--- |
| `graphs/` | One file per case: settings overrides, scene inputs, the render payload, the full ComfyUI API graph, timing and the audio trim call. `error_*` cases hold the expected error. `legacy_*` cases cover the retired 3 pass builder. | `make_graph_fixtures.py` |
| `core/` | Tables of `{input, output}` or `{input, error}` for timing, token math, masked context, resolution, tile plan, settings normalization, payload building. | `make_core_fixtures.py` |
| `scene/` | Reference choices, ordered references, image paths and scene input resolution against `session.json` and `segments.json`. | `make_scene_fixtures.py` |
| `prompt/` | Cut plans, craft text, cue handling, reference definitions, assembly, validation, lyric tagging, continuation text. | `make_prompt_fixtures.py` |
| `project/` | File system cases for the project layer: JSON text, session saves and revisions, renumbering, latent dirty flags, per take latents, the rename journal. Each holds the folder tree before and after. | `make_project_fixtures.py` |
| `media/` | Clip operations (trim, collect, final frame, color match, stitch) on synthetic ffmpeg clips, with what ffprobe reports and a hash of the decoded audio. Needs ffmpeg and ffprobe on PATH to regenerate. | `make_media_fixtures.py` |
| `pipeline/` | Stitch layouts, recording a clip on its scene, continuity a render used. | `make_pipeline_fixtures.py` |
| `cases/` | The pack's own case files, copied unchanged. | `make_all.py` |
| `object_info_h3.json` | `/object_info` entries for the H3 node classes. | `capture_object_info.py` |
| `session_shape.json` | Key paths and value types of 20 real sessions. No values. | `session_shape.py` |
| `MANIFEST.json` | Pack commit and file counts. | `make_all.py` |

Fake paths: everything lives under `Z:\h3fixture`. Paths there count as existing files in the graph builders, so graphs contain those exact strings.

Regenerate (the pack is not modified):

```powershell
..\..\..\..\python_embeded\python.exe tools\make_all.py --pack .. --comfy-url http://127.0.0.1:8188 --sessions ..\..\..\output
```

Run it after the pack changes, then `cargo test --workspace` to see which ports drifted.
