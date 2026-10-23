"""Dev tool: golden project folders for the scene operations (``h3-project::scene_ops``), made by the pack's own code.

    python tools/make_scene_ops_fixtures.py --pack <pack folder>

Each case builds a project folder, runs one of the pack's mutation functions (create, delete, split, merge, move,
resize, bulk, close gaps) and records the folder before and after plus the result. The temp folder is written as
``<root>`` so the Rust port can replay on its own. Writes ``fixtures/scene_ops/ops.json``.
"""

import argparse
import copy
import importlib
import itertools
import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import make_project_fixtures as mpf  # noqa: E402
import pack_env  # noqa: E402


def normalize(text: str, root: str) -> str:
    escaped = json.dumps(root)[1:-1]
    return text.replace(escaped, "<root>").replace(root, "<root>").replace(root.replace("\\", "/"), "<root>")


def tree_of(root: Path) -> dict:
    snap = mpf.snapshot(root)
    base = str(root)
    return {k: (normalize(v, base) if isinstance(v, str) else v) for k, v in snap.items()}


def session_text(session: dict) -> str:
    return json.dumps(session, indent=2, ensure_ascii=False) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "scene_ops")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    mut = importlib.import_module("vrgdg_pack.agent_api.mutations")
    errors = importlib.import_module("vrgdg_pack.agent_api.errors")
    # The pack's loader also re-points paths and links loose media (`_rehydrate_builder_session`). That is a separate
    # piece of work, so these cases test the scene operations alone.
    project = importlib.import_module("vrgdg_pack.builder.project")
    project._rehydrate_builder_session = lambda folder, session: session
    counter = itertools.count(1)
    mut._generate_scene_id = lambda: f"seg_new{next(counter)}"
    mut.notify_project_changed = lambda *a, **k: None

    def base_session(root: str) -> dict:
        def vid(n):
            return os.path.join(root, "rendered_scene_videos", f"video_{n:04d}-audio.mp4")

        return {
            "project_name": "Ops",
            "revision": 4,
            "builder_save_revision": 4,
            "video_engine": "minimax_h3",
            "audio_path": "",
            "segments": [
                {"id": "s1", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "one two three four", "notes": "n1"},
                {"id": "s2", "start": 4.0, "end": 8.0, "label": "2. Rooftop", "lyric_text": "alpha\nbeta", "video_path": vid(2), "notes": "n2"},
                {"id": "s3", "start": 8.0, "end": 12.0, "label": "Scene 3", "lyric_text": "[instrumental]", "timeline_note": "t3"},
                {"id": "s4", "start": 12.0, "end": 16.0, "label": "Scene 4", "lyric_text": "last words here", "video_path": vid(4)},
            ],
            "overlay_segments": [],
        }

    def keep(path: str) -> bool:
        return "10001" not in path

    files = {k: v for k, v in mpf.rich_tree().items() if keep(k)}
    # Keep the tree small: scene folders for four scenes, no latents beyond a few.
    cases = []

    def run(name, call, t=7, mutate=None):
        counter_reset = itertools.count(1)
        mut._generate_scene_id = lambda: f"seg_new{next(counter_reset)}"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            root.mkdir()
            session = base_session(str(root))
            if mutate:
                mutate(session)
            seed_files = dict(files)
            seed_files["vrgdg_builder_session.json"] = session_text(session)
            mpf.seed(root, seed_files)
            mut.resolve_project_folder = lambda pid, r=str(root): r
            before = tree_of(root)
            entry = {"name": name, "before": before}
            try:
                with mpf.frozen(t):
                    result = call()
                entry["result"] = json.loads(normalize(json.dumps(result, default=str), str(root)))
            except Exception as exc:  # noqa: BLE001
                entry["error"] = {"type": type(exc).__name__, "message": normalize(str(exc), str(root)), "code": getattr(exc, "code", None)}
            entry["after"] = tree_of(root)
            cases.append(entry)

    P = "p"
    run("create_append", lambda: mut.create_scene(P, "append", None, 5.0, "", "tp", "ip", "nt"))
    run("create_before_s2", lambda: mut.create_scene(P, "before", "s2", 3.0, "Intro"))
    run("create_after_s1_by_number", lambda: mut.create_scene(P, "after", "1", 2.5))
    run("create_missing_ref", lambda: mut.create_scene(P, "after", "zz", 2.5))
    run("create_conflict", lambda: mut.create_scene(P, "append", None, 4.0, if_match_revision=2))
    run("delete_s2_ripple", lambda: mut.delete_scene(P, "s2", True))
    run("delete_s3_no_ripple", lambda: mut.delete_scene(P, "s3", False))
    run("delete_missing", lambda: mut.delete_scene(P, "nope"))
    run("split_s1_clear", lambda: mut.split_scene(P, "s1", 2.5, True))
    run("split_s3_keep_media", lambda: mut.split_scene(P, "s3", 10.0, False))
    run("split_locked", lambda: mut.split_scene(P, "s2", 6.0))
    run("split_out_of_range", lambda: mut.split_scene(P, "s1", 4.0))
    run("merge_s1_next", lambda: mut.merge_scenes(P, "s1", "next"))
    run("merge_s3_previous_locked", lambda: mut.merge_scenes(P, "s3", "previous"))
    run("merge_s3_next_locked", lambda: mut.merge_scenes(P, "s3", "next"))
    run("merge_bad_direction", lambda: mut.merge_scenes(P, "s1", "up"))
    run("merge_no_neighbor", lambda: mut.merge_scenes(P, "s1", "previous"))
    run("move_s1", lambda: mut.move_scene(P, "s1", 1.0, False))
    run("move_s3_ripple", lambda: mut.move_scene(P, "s3", 9.5, True))
    run("resize_s1_duration", lambda: mut.resize_scene(P, "s1", duration=3.0))
    run("resize_s3_end_ripple", lambda: mut.resize_scene(P, "s3", end_time=15.0, ripple=True))
    run("bulk_replace_durations", lambda: mut.timeline_bulk(P, "3\n4 hello there\n5", "durations", "replace"))
    run("bulk_replace_carry", lambda: mut.timeline_bulk(P, "0 - 6\n6 - 16", "ranges", "replace"))
    run("bulk_append_markers", lambda: mut.timeline_bulk(P, "0 a\n3 b\n6", "markers", "append", 20.0))
    run("bulk_append_default", lambda: mut.timeline_bulk(P, "2\n2", "durations", "append"))
    run("bulk_bad", lambda: mut.timeline_bulk(P, "oops", "durations", "replace"))
    run("bulk_bad_action", lambda: mut.timeline_bulk(P, "2", "durations", "merge"))

    def gapped(session):
        session["segments"][1]["start"] = 5.0
        session["segments"][1]["end"] = 9.0
        session["segments"][2]["start"] = 10.0
        session["segments"][2]["end"] = 13.0
        session["segments"][3]["start"] = 14.0
        session["segments"][3]["end"] = 18.0

    run("close_gaps", lambda: mut.timeline_close_gaps(P), mutate=gapped)

    (args.out / "ops.json").write_text(json.dumps({"cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"ops: {len(cases)} cases, errors: {sum(1 for c in cases if 'error' in c)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
