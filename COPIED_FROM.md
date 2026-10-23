# Copied and ported from the pack

Every file copied from `comfyui-vrgamedevgirl`, or code ported from it, gets one row. Re-check these rows when the pack changes.

| Date | Pack path | Pack commit | Rust destination | Kind | Changes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| 2026-10-22 | `core/atomic_write.py` | 14421d2 | `crates/h3-project/src/atomic.rs` | Port | Same behavior: temp sibling, fsync, rename. |
| 2026-10-22 | `Workflows/UsedForUIDoNotTouch/minimax_*.json`, `runner/minimax_*.py` (node class names only) | 14421d2 | `crates/h3-comfy/src/preflight.rs` | Node list | Grouped by render feature. |
