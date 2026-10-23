# Copied and ported from the pack

Every file copied from `comfyui-vrgamedevgirl`, or code ported from it, gets one row. Re-check these rows when the pack changes.

| Date | Pack path | Pack commit | Rust destination | Kind | Changes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| 2026-10-22 | `core/atomic_write.py` | 14421d2 | `crates/h3-project/src/atomic.rs` | Port | Same behavior: temp sibling, fsync, rename. |
| 2026-10-22 | `Workflows/UsedForUIDoNotTouch/minimax_*.json`, `runner/minimax_*.py` (node class names only) | 14421d2 | `crates/h3-comfy/src/preflight.rs` | Node list | Grouped by render feature. |
| 2026-10-22 | `Workflows/UsedForUIDoNotTouch/minimax_*.json` (via ComfyUI `/object_info`) | 14421d2 | `fixtures/object_info_h3.json` | Snapshot | Only the node classes the H3 graphs use, from the live ComfyUI 0.42.0. |
| 2026-10-22 | `runner/minimax_workflows.py`, `minimax_patches.py`, `minimax_inputs.py`, `minimax_keyframes.py` (outputs) | 14421d2 | `fixtures/graphs/*.json` | Generated | Built by `tools/make_graph_fixtures.py` with fake folders; ffmpeg and ffprobe replaced by recorders. |
| 2026-10-22 | `minimax/latent_manager.py`, `resolution.py`, `tile_plan.py`, `settings_payload.py` (outputs) | 14421d2 | `fixtures/core/*.json` | Generated | By `tools/make_core_fixtures.py`. |
| 2026-10-22 | `minimax/scene_inputs.py` (outputs) | 14421d2 | `fixtures/scene/*.json` | Generated | By `tools/make_scene_fixtures.py` with a synthetic Reference Builder session. |
| 2026-10-22 | `minimax/shot_prompt.py` (outputs) | 14421d2 | `fixtures/prompt/*.json` | Generated | By `tools/make_prompt_fixtures.py`. |
| 2026-10-22 | `tests/minimax_craft_cases.json`, `refmod_scene_cases.json`, `story_acts_cases.json`, `story_locations_cases.json`, `story_mode_cases.json`, `temporal_effect_cases.json`, `video_style_cases.json` | 14421d2 | `fixtures/cases/` | Copy | Unchanged. |
| 2026-10-22 | Real `vrgdg_builder_session.json` files (20 projects) | 14421d2 | `fixtures/session_shape.json` | Shape only | Key paths and value types. No values, so no lyrics, prompts, names, paths or keys. |
| 2026-10-23 | `minimax/latent_manager.py` (`_tokens_to_frames`, `_frames_to_tokens`, `plan_masked_context`, `calculate_minimax_h3_timing`) | 14421d2 | `crates/h3-core/src/tokens.rs`, `timing.rs` | Port | Decimal math with Python's 28 digit rounding after each step, so frame counts match (a 4.0416..67 s context times 24 ceils to 98). |
