"""Dev tool: golden file-system fixtures for the project layer (``h3-project``), made by running the pack's own code.

    python tools/make_project_fixtures.py --pack <pack folder>

Every case builds a small project folder in a temp directory, runs the pack's function on it and records the folder
tree before and after (relative paths, text contents). The Rust port replays the same steps on its own temp folder and
must end with the same tree. Time is frozen, so file names and stamps are deterministic. The pack is not modified.

Writes ``fixtures/project/*.json``.
"""

import argparse
import contextlib
import copy
import importlib
import json
import os
import shutil
import sys
import tempfile
import time
import types
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

BASE_TIME = 1_700_000_000
MS = 123


def stamp_compact(t):
    return f"20260101_{t // 3600 % 24:02d}{t // 60 % 60:02d}{t % 60:02d}"


def stamp_iso(t):
    return f"2026-01-01T{t // 3600 % 24:02d}:{t // 60 % 60:02d}:{t % 60:02d}"


@contextlib.contextmanager
def frozen(t):
    """``time.time`` and ``time.strftime`` return values derived from ``t``."""
    real_time, real_strftime = time.time, time.strftime
    time.time = lambda: BASE_TIME + t + MS / 1000.0

    def fake_strftime(fmt, *args):
        return stamp_iso(t) if "T" in fmt else stamp_compact(t)

    time.strftime = fake_strftime
    try:
        yield
    finally:
        time.time, time.strftime = real_time, real_strftime


def write(out_dir: Path, name: str, function: str, cases: list, sort: bool = True) -> None:
    path = out_dir / f"{name}.json"
    path.write_text(json.dumps({"function": function, "cases": cases}, indent=1, sort_keys=sort, ensure_ascii=False), encoding="utf-8")
    print(f"{name}: {len(cases)} cases")


def seed(root: Path, files: dict) -> None:
    """``files`` maps a relative path to text. A path ending in ``/`` is an empty folder."""
    for rel, content in files.items():
        target = root / rel
        if rel.endswith("/"):
            target.mkdir(parents=True, exist_ok=True)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content.encode("utf-8"))


def snapshot(root: Path) -> dict:
    """Relative path (folders end in ``/`` and map to null) to text. Dirty flag timestamps are zeroed."""
    out = {}
    for base, dirs, files in os.walk(root):
        rel_base = os.path.relpath(base, root).replace("\\", "/")
        rel_base = "" if rel_base == "." else rel_base + "/"
        for d in dirs:
            out[rel_base + d + "/"] = None
        for f in files:
            text = Path(base, f).read_bytes().decode("utf-8")
            if f.endswith(".dirty") or f == ".timeline_journal.json":
                try:
                    data = json.loads(text)
                    for key in ("timestamp", "created_at", "updated_at"):
                        if key in data:
                            data[key] = 0
                    text = json.dumps(data, sort_keys=True)
                except ValueError:
                    pass
            out[rel_base + f] = text
    return dict(sorted(out.items()))


def rel_pairs(root: Path, pairs) -> list:
    def rel(p):
        return os.path.relpath(p, root).replace("\\", "/")

    return [[rel(a), rel(b)] for a, b in pairs]


def rich_tree() -> dict:
    """A four scene project with a file in every per scene folder."""
    files = {}
    for n in (1, 2, 3, 4):
        files[f"rendered_scene_videos/video_{n:04d}-audio.mp4"] = f"video {n}"
        files[f"rendered_scene_videos/video_{n:04d}.mp4"] = f"raw video {n}"
        files[f"scene_video_thumbnails/video_{n:04d}.jpg"] = f"thumb {n}"
        files[f"zimage_approved/image_{n:04d}.png"] = f"image {n}"
        files[f"scene_audio/scene_audio_{n:04d}.wav"] = f"scene audio {n}"
        files[f"scene_audio_trimmed/audio_{n:04d}.wav"] = f"trimmed {n}"
        files[f"scene_srt/scene_{n:04d}.srt"] = f"srt {n}"
        files[f"minimax_h3_scene_audio/scene_{n:04d}.wav"] = f"h3 audio {n}"
        files[f"reference_to_video_clips/video_{n:04d}.mp4"] = f"r2v {n}"
        files[f"scene_image_previews/scene_{n:04d}/image_{n:04d}_a.png"] = f"preview {n}a"
        files[f"scene_image_previews/scene_{n:04d}/notes.txt"] = f"notes {n}"
    files["rendered_scene_videos/video_10001-audio.mp4"] = "insert track clip"
    files["rendered_scene_videos_backup/scene_0002/video_0002-audio_old.mp4"] = "old take 2"
    files["rendered_scene_videos_backup/scene_0002/keep.mp4"] = "unnumbered"
    files["project_context/ConceptPrompts.txt"] = "concepts"
    files["project_context/scene_0003_context.txt"] = "context 3"
    for n in (1, 2, 3):
        files[f"latents/scene_{n:03d}.latent"] = f"latent {n}"
        files[f"latents/scene_{n:03d}.latent.json"] = json.dumps({"token_count": str(10 + n), "scene_number": str(n)})
    files["latents/scene_002.pass1.latent"] = "pass1 2"
    files["latents/scene_002.pass1.latent.json"] = json.dumps({"token_count": "12"})
    files["latents/scene_002.dirty"] = json.dumps({"scene_number": 2, "timestamp": 1.0, "reason": "seed"})
    files["latents/scene_002.takes/index.json"] = json.dumps({"takes": [{"id": "t2", "video_fp": "aa"}], "pending": ""})
    files["latents/scene_002.takes/t2.latent"] = "take latent 2"
    files["latents/scene_003.takes/index.json"] = json.dumps({"takes": [{"id": "t3", "video_fp": "bb"}], "pending": ""})
    files["latents/scene_003.takes/t3.latent"] = "take latent 3"
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "project")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)

    class _Routes:
        def __getattr__(self, name):
            return lambda *a, **k: (lambda f: f)

    class _PromptServer:
        instance = types.SimpleNamespace(routes=_Routes(), send_sync=lambda *a, **k: None, app=None)

    server = types.ModuleType("server")
    server.PromptServer = _PromptServer
    sys.modules["server"] = server

    project = importlib.import_module("vrgdg_pack.builder.project")
    lm = importlib.import_module("vrgdg_pack.minimax.latent_manager")
    takes = importlib.import_module("vrgdg_pack.minimax.latent_takes")
    timeline = importlib.import_module("vrgdg_pack.builder.timeline")
    manager = lm.SceneLatentManager
    out = args.out

    # -- JSON text the pack writes: json.dumps(indent=2, ensure_ascii=False) + newline.
    texts = [
        "{}", "[]", '{"a": 1, "b": [1, 2, {"c": null}], "d": {}, "e": []}', '{"x": 1.0, "y": 100000.0, "z": 0.5}',
        '{"small": 1e-05, "big": 1e+16, "mid": 123456789012345.0, "neg": -0.0, "tiny": 5e-324}', '{"t": 0.1, "u": 1787010332.0634232}',
        '{"s": "caf\\u00e9 \\u4e2d\\u6587 \\ud83d\\ude00 \\"quoted\\" back\\\\slash slash/ tab\\t nl\\n ctl\\u0001 del\\u007f"}',
        '{"order": {"z": 1, "a": 2, "m": 3}, "n": [3, 2, 1]}', '{"int": 9007199254740993, "neg": -12, "zero": 0}',
        '{"nested": [[], [[]], [{}], [{"k": []}]]}', '{"bool": [true, false], "none": null}',
        '{"float_int_like": 3.0, "exp": 2.5e+20, "frac": 0.000123}',
        '{"tie": 0.18913650512695312, "tie2": 0.18913650512695313, "seventeen": 5.8473178796702996e-05, "peak": [0.0002681194164324552, 4.352075848146342e-05]}',
        '{"a": 0.30000000000000004, "b": 1e22, "c": 1.7976931348623157e+308, "d": 123456789.12345679, "e": 9.999999999999999e-05}',
    ]
    cases = []
    for text in texts:
        value = json.loads(text)
        cases.append({"input": text, "output": json.dumps(value, indent=2, ensure_ascii=False) + "\n"})
    write(out, "json_dumps", "json.dumps(indent=2, ensure_ascii=False) + newline", cases)

    # -- Session save: revision check, stale snapshot, derived fields, backups, SRT.
    base_segments = [
        {"id": "s1", "start": 0.0, "end": 4.5, "label": "First scene", "t2i_prompt": "a"},
        {"id": "s2", "start": 4.5, "end": 9.25, "label": "Second scene", "i2v_prompt": "b"},
    ]
    scenarios = {
        "fresh": {"existing": None, "session": {"segments": base_segments, "note": "fresh"}},
        "next_revision": {
            "existing": {"revision": 5, "builder_save_revision": 5, "segments": base_segments, "kept": "yes"},
            "session": {"builder_save_revision": 5, "segments": base_segments, "extra_key": {"a": 1}},
        },
        "stale_snapshot": {
            "existing": {"revision": 9, "builder_save_revision": 9, "segments": base_segments, "marker": "server"},
            "session": {"builder_save_revision": 4, "segments": [], "marker": "client"},
        },
        "equal_revision": {
            "existing": {"revision": 3, "builder_save_revision": 3, "segments": base_segments},
            "session": {"builder_save_revision": 3, "segments": base_segments},
        },
        "no_incoming_revision": {
            "existing": {"revision": 7, "builder_save_revision": 8, "segments": base_segments},
            "session": {"segments": base_segments, "marker": "no revision sent"},
        },
        "existing_without_revision": {
            "existing": {"builder_save_revision": 3, "segments": base_segments},
            "session": {"builder_save_revision": 3, "segments": base_segments},
        },
        "null_reference_builder_keeps_previous": {
            "existing": {"revision": 2, "builder_save_revision": 2, "flux_reference_builder": {"subjects": [{"id": "a"}]}},
            "session": {"builder_save_revision": 2, "flux_reference_builder": None, "segments": base_segments},
        },
        "null_reference_builder_no_previous": {
            "existing": {"revision": 2, "builder_save_revision": 2, "segments": base_segments},
            "session": {"builder_save_revision": 2, "flux_reference_builder": None, "segments": base_segments},
        },
        "overlay_slots": {
            "existing": None,
            "session": {"segments": base_segments, "overlay_segments": [
                {"id": "o1"}, {"id": "o2", "overlay_slot_number": 10005}, {"id": "o3", "overlay_slot_number": 10005},
                {"id": "o4", "slot_number": "10002"}, {"id": "o5", "scene_slot_number": 3},
            ]},
        },
        "bad_segments_type": {"existing": None, "session": {"segments": "nope"}},
        "overlay_not_a_list": {"existing": None, "session": {"segments": base_segments, "overlay_segments": "nope"}},
        "derived_files": {
            "existing": None,
            "session": {
                "video_engine": "minimax_h3",
                "segments": [
                    {"id": "s1", "start": 0.0, "end": 4.5, "label": "First", "t2i_prompt": " image one ", "timeline_note": "note one"},
                    {"id": "s2", "start": 4.5, "end": 9.0, "i2v_prompt": "motion two", "timeline_note": None},
                    {"id": "s3", "start": 9.0, "end": 12.0, "t2i_prompt": "image three", "i2v_prompt": "motion three", "timeline_note": "café"},
                ],
                "overlay_segments": [{"id": "o1", "start": 1.0, "t2i_prompt": "overlay image"}],
                "lyric_mapper": {"source_text": "  Line one\nLine two  "},
                "flux_reference_builder": {
                    "subjects": [
                        {"name": "Mara", "description": "red jacket"}, {"name": "A/B:C?", "description": ""},
                        {"name": "x" * 130, "description": "long name"}, {"description": "unnamed"}, "junk",
                    ],
                    "locations": [{"name": "Neon street", "description": "wet street"}],
                    "scene_map": {"s1": "loc1"},
                },
            },
        },
        "unicode_and_floats": {
            "existing": None,
            "session": {"segments": [{"id": "u", "start": 0, "end": 1.0, "label": "café 中文", "x": 1e-05}], "ratio": 0.1},
        },
        "srt_ordering": {
            "existing": None,
            "session": {"segments": [
                {"id": "b", "start": 5, "end": 6, "label": "late"}, {"id": "a", "start": 0, "end": 0.01, "label": ""},
                {"id": "c", "start": 2.0004, "t2i_prompt": "prompt text"},
            ]},
        },
    }
    cases = []
    for name, scenario in scenarios.items():
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            root.mkdir()
            session_file = root / "vrgdg_builder_session.json"
            if scenario["existing"] is not None:
                session_file.write_text(json.dumps(scenario["existing"], indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
            before = snapshot(root)
            payload = {"project_folder": str(root), "session": copy.deepcopy(scenario["session"])}
            with frozen(1):
                result = project._save_builder_session(payload)
            after = snapshot(root)
            saved = json.loads(session_file.read_text(encoding="utf-8"))
            saved_text = session_file.read_text(encoding="utf-8")
            record = {
                "input": {"name": name, "existing": scenario["existing"], "session": scenario["session"]},
                "stale": bool(result.get("stale")),
                "revision": result.get("revision"),
                "saved_session": {k: ("<root>" if k == "project_folder" and v == str(root) else v) for k, v in saved.items() if k != "updated"},
                "saved_key_order": [k for k in saved.keys()],
                "has_updated": "updated" in saved,
                "srt": after.get("builder_segments.srt"),
                "backup_count": len([k for k in after if k.startswith("session_backups/") and k.endswith(".json")]),
                "backup_text": next((v for k, v in after.items() if k.startswith("session_backups/")), None),
                "stale_current_revision": result.get("current_revision"),
            }
            record["derived"] = {k: v for k, v in after.items() if v is not None and k not in ("vrgdg_builder_session.json",) and not k.startswith("session_backups/")}
            record["folders_created"] = sorted(k for k in after if k.endswith("/") and k.count("/") == 1)
            record["_before_files"] = sorted(k for k in before)
            record["saved_text_equals_dumps"] = saved_text == json.dumps(saved, indent=2, ensure_ascii=False) + "\n"
            cases.append(record)
    write(out, "save_session", "builder.project._save_builder_session", cases, sort=False)

    # -- Backup pruning keeps the newest 25 by modification time.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        folder = root / "session_backups"
        folder.mkdir()
        for i in range(30):
            path = folder / f"vrgdg_builder_session_20260101_{i:06d}.json"
            path.write_text("{}", encoding="utf-8")
            os.utime(path, (1000 + i, 1000 + i))
        (folder / "other.txt").write_text("x", encoding="utf-8")
        project._prune_session_backups(str(root))
        kept = sorted(p.name for p in folder.iterdir())
    write(out, "prune_backups", "_prune_session_backups", [{"input": {"made": 30, "max_keep": 25}, "kept": kept}])

    # -- New project scaffold.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "Song"
        project._new_builder_project({"project_folder": str(root)})
        scaffold = snapshot(root)
    write(out, "new_project", "_new_builder_project", [{"input": {"name": "Song"}, "tree": scaffold}])

    # -- Scene asset renumbering.
    cases = []
    for op, scene in [("insert", 1), ("insert", 2), ("insert", 3), ("insert", 4), ("insert", 7),
                      ("remove", 1), ("remove", 2), ("remove", 3), ("remove", 4), ("remove", 9)]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            seed(root, rich_tree())
            before = snapshot(root)
            with frozen(7):
                if op == "insert":
                    renamed = project._renumber_scene_assets_after_insert(str(root), scene)
                else:
                    renamed = project._renumber_scene_assets_after_removal(str(root), scene)
            cases.append({"input": {"op": op, "scene": scene, "tree": before}, "renamed": rel_pairs(root, renamed), "tree_after": snapshot(root)})
    write(out, "renumber", "_renumber_scene_assets_after_insert / _removal", cases)

    # -- Removed scene archive pruning.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for i in range(24):
            folder = root / "removed_scene_assets" / f"scene_{i:04d}_20260101_000000"
            folder.mkdir(parents=True)
            (folder / "f.txt").write_text("x", encoding="utf-8")
            os.utime(folder, (2000 + i, 2000 + i))
        project._prune_removed_scene_assets(str(root))
        kept = sorted(p.name for p in (root / "removed_scene_assets").iterdir())
    write(out, "prune_removed", "_prune_removed_scene_assets", [{"input": {"made": 24, "max_keep": 20}, "kept": kept}])

    # -- Latent bookkeeping.
    def session_with(used_values):
        segments = []
        for used in used_values:
            segment = {"id": "x"}
            if used is not None:
                segment["minimax_h3_continuity_mode_used"] = used
            segments.append(segment)
        return {"segments": segments}

    cases = []
    latent_files = {"latents/scene_001.latent": "l1", "latents/scene_002.latent": "l2"}
    for label, session, extra in [
        ("no_record", session_with([None, None]), {}),
        ("masked_used", session_with([None, "latent_continuation_masked"]), {}),
        ("off_used", session_with([None, "off"]), {}),
        ("upper_off", session_with([None, " OFF "]), {}),
        ("short_session", session_with([None]), {}),
        ("no_session", None, {}),
        ("no_successor_latent", session_with([None, None]), {"drop": "latents/scene_002.latent"}),
    ]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            files = dict(latent_files)
            if session is not None:
                files["vrgdg_builder_session.json"] = json.dumps(session)
            files.pop(extra.get("drop", ""), None)
            seed(root, files)
            with frozen(3):
                returned = manager.mark_successor_dirty(str(root), 1, reason="Scene 001 re-rendered")
                continues = manager.continues_from_predecessor(str(root), 2)
            cases.append({"input": {"op": "mark_successor_dirty", "label": label, "files": files, "scene": 1},
                          "returned": returned, "continues": continues, "tree_after": snapshot(root), "dirty": manager.list_dirty(str(root))})
    ops = [
        ("mark_dirty", {"scene": 2, "reason": ""}), ("mark_dirty", {"scene": 3, "reason": "because"}),
        ("clear_dirty", {"scene": 2}), ("delete_latent", {"scene": 2, "reindex": False}),
        ("delete_latent", {"scene": 2, "reindex": True}), ("delete_all", {}), ("make_room", {"scene": 2}), ("reindex", {"scene": 1}),
    ]
    for op, params in ops:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            seed(root, rich_tree())
            before = snapshot(root)
            with frozen(4):
                if op == "mark_dirty":
                    manager.mark_dirty(str(root), params["scene"], params["reason"])
                    returned = None
                elif op == "clear_dirty":
                    manager.clear_dirty(str(root), params["scene"])
                    returned = None
                elif op == "delete_latent":
                    returned = manager.delete_latent(str(root), params["scene"], reindex=params["reindex"])
                elif op == "delete_all":
                    returned = manager.delete_all_latents(str(root))
                elif op == "make_room":
                    manager.make_room_for_scene(str(root), params["scene"])
                    returned = None
                else:
                    manager.reindex_latents(str(root), params["scene"])
                    returned = None
            cases.append({"input": {"op": op, "params": params, "tree": before}, "returned": returned,
                          "tree_after": snapshot(root), "dirty": manager.list_dirty(str(root))})
    write(out, "latent_ops", "SceneLatentManager dirty, delete, reindex, make_room", cases)

    cases = []
    sidecars = {
        "full": {"frame_count": "97", "token_count": "26", "fps": "24.0", "timestamp": "1700000000.5", "tail_padding_frames": "9"},
        "no_tail": {"frame_count": "97", "token_count": "26", "timestamp": "1700000000.5"},
        "empty_tail": {"token_count": "5", "tail_padding_frames": "", "timestamp": "10"},
        "float_tail": {"token_count": "5", "tail_padding_frames": "9.0", "timestamp": "10"},
        "bad_json": "not json",
        "none": None,
    }
    for name, sidecar in sidecars.items():
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            files = {"latents/scene_001.latent": "abcdef"}
            if isinstance(sidecar, dict):
                files["latents/scene_001.latent.json"] = json.dumps(sidecar)
            elif isinstance(sidecar, str):
                files["latents/scene_001.latent.json"] = sidecar
            seed(root, files)
            info = manager.get_latent_info(str(root), 1)
            info["path"] = os.path.relpath(info["path"], root).replace("\\", "/")
            if not isinstance(sidecar, dict):
                info.pop("timestamp", None)  # file modification time, not stable
            cases.append({"input": {"name": name, "files": files}, "info": info})
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "proj"
        root.mkdir()
        info = manager.get_latent_info(str(root), 4)
        info["path"] = os.path.relpath(info["path"], root).replace("\\", "/")
        cases.append({"input": {"name": "missing", "files": {}, "scene": 4}, "info": info})
    write(out, "latent_info", "SceneLatentManager.get_latent_info", cases)

    # -- Per take latents: replayed op by op.
    def run_takes(ops):
        results = []
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            root.mkdir()
            for step in ops:
                with frozen(step.get("t", 0)):
                    kind = step["op"]
                    if kind == "write":
                        seed(root, {step["path"]: step["content"]})
                        results.append(None)
                    elif kind == "archive":
                        latents = root / "latents"
                        results.append(takes.archive_latent(str(latents), step["scene"], str(latents / f"scene_{step['scene']:03d}.latent")))
                    elif kind == "attach":
                        results.append(takes.attach_video(str(root), step["scene"], str(root / step["video"])))
                    elif kind == "activate":
                        results.append(takes.activate_for_video(str(root), step["scene"], str(root / step["video"]) if step.get("video") else ""))
                    elif kind == "list":
                        results.append(takes.list_takes(str(root), step["scene"]))
                    elif kind == "rename_video":
                        old = takes.video_fingerprint(str(root / step["old"]))
                        new = takes.video_fingerprint(str(root / step["new"]))
                        takes.rename_video(str(root), step["scene"], old, new)
                        results.append(None)
                    elif kind == "fingerprint":
                        results.append(takes.video_fingerprint(str(root / step["video"])))
                    elif kind == "remove":
                        os.remove(root / step["path"])
                        results.append(None)
            return results, snapshot(root)

    big = "x" * (300 * 1024) + "tail-bytes"
    scenarios = {
        "two_renders_then_activate_first": [
            {"op": "write", "path": "latents/scene_001.latent", "content": "latent A"},
            {"op": "write", "path": "latents/scene_001.latent.json", "content": "{\"token_count\": \"9\"}"},
            {"op": "write", "path": "rendered_scene_videos/video_0001-audio.mp4", "content": "video A"},
            {"op": "archive", "scene": 1, "t": 1},
            {"op": "attach", "scene": 1, "video": "rendered_scene_videos/video_0001-audio.mp4", "t": 2},
            {"op": "list", "scene": 1},
            {"op": "write", "path": "rendered_scene_videos_backup/scene_0001/video_0001-audio_old.mp4", "content": "video A"},
            {"op": "write", "path": "latents/scene_001.latent", "content": "latent B"},
            {"op": "write", "path": "rendered_scene_videos/video_0001-audio.mp4", "content": "video B"},
            {"op": "archive", "scene": 1, "t": 3},
            {"op": "attach", "scene": 1, "video": "rendered_scene_videos/video_0001-audio.mp4", "t": 4},
            {"op": "list", "scene": 1},
            {"op": "activate", "scene": 1, "video": "rendered_scene_videos_backup/scene_0001/video_0001-audio_old.mp4", "t": 5},
            {"op": "activate", "scene": 1, "video": "rendered_scene_videos_backup/scene_0001/video_0001-audio_old.mp4", "t": 6},
            {"op": "activate", "scene": 1, "video": "rendered_scene_videos/video_0001-audio.mp4", "t": 7},
        ],
        "no_takes_and_no_archive": [
            {"op": "write", "path": "rendered_scene_videos/video_0002-audio.mp4", "content": "v"},
            {"op": "activate", "scene": 2, "video": "rendered_scene_videos/video_0002-audio.mp4"},
            {"op": "write", "path": "latents/scene_002.latent", "content": "L"},
            {"op": "archive", "scene": 2, "t": 1},
            {"op": "attach", "scene": 2, "video": "rendered_scene_videos/video_0002-audio.mp4", "t": 2},
            {"op": "write", "path": "rendered_scene_videos/video_0002-audio_other.mp4", "content": "different"},
            {"op": "activate", "scene": 2, "video": "rendered_scene_videos/video_0002-audio_other.mp4", "t": 3},
            {"op": "activate", "scene": 2, "video": "", "t": 3},
        ],
        "pending_replaced_and_missing_video": [
            {"op": "write", "path": "latents/scene_003.latent", "content": "first"},
            {"op": "archive", "scene": 3, "t": 1},
            {"op": "write", "path": "latents/scene_003.latent", "content": "second"},
            {"op": "archive", "scene": 3, "t": 2},
            {"op": "attach", "scene": 3, "video": "rendered_scene_videos/missing.mp4", "t": 3},
            {"op": "write", "path": "rendered_scene_videos/video_0003-audio.mp4", "content": "v3"},
            {"op": "attach", "scene": 3, "video": "rendered_scene_videos/video_0003-audio.mp4", "t": 4},
            {"op": "attach", "scene": 3, "video": "rendered_scene_videos/video_0003-audio.mp4", "t": 5},
            {"op": "list", "scene": 3},
        ],
        "rewritten_clip_keeps_take": [
            {"op": "write", "path": "latents/scene_001.latent", "content": "L"},
            {"op": "write", "path": "rendered_scene_videos/video_0001-audio.mp4", "content": "before color match"},
            {"op": "archive", "scene": 1, "t": 1},
            {"op": "attach", "scene": 1, "video": "rendered_scene_videos/video_0001-audio.mp4", "t": 2},
            {"op": "write", "path": "rendered_scene_videos/video_0001-audio_matched.mp4", "content": "after color match"},
            {"op": "rename_video", "scene": 1, "old": "rendered_scene_videos/video_0001-audio.mp4", "new": "rendered_scene_videos/video_0001-audio_matched.mp4"},
            {"op": "list", "scene": 1},
            {"op": "activate", "scene": 1, "video": "rendered_scene_videos/video_0001-audio_matched.mp4", "t": 3},
        ],
        "fingerprints": [
            {"op": "write", "path": "small.bin", "content": "tiny"},
            {"op": "write", "path": "big.bin", "content": big},
            {"op": "fingerprint", "video": "small.bin"},
            {"op": "fingerprint", "video": "big.bin"},
            {"op": "fingerprint", "video": "absent.bin"},
        ],
    }
    cases = []
    for name, steps in scenarios.items():
        results, tree = run_takes(steps)
        cases.append({"input": {"name": name, "steps": steps}, "results": results, "tree_after": tree})
    write(out, "latent_takes", "latent_takes archive, attach, activate, rename, fingerprint", cases)

    # -- Take folder helpers on the rich tree.
    cases = []
    for op, params in [("shift", {"first": 2, "delta": 1}), ("shift", {"first": 2, "delta": -1}), ("delete_scene", {"scene": 2}),
                       ("delete_all", {}), ("copy", {})]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            seed(root, rich_tree())
            latents = str(root / "latents")
            dst = Path(tmp) / "dst"
            (dst / "latents").mkdir(parents=True)
            if op == "shift":
                takes.shift_takes(latents, params["first"], params["delta"])
            elif op == "delete_scene":
                takes.delete_scene_takes(latents, params["scene"])
            elif op == "delete_all":
                takes.delete_all_takes(latents)
            else:
                manager.copy_latents_folder(str(root), str(dst))
            cases.append({"input": {"op": op, "params": params, "tree": rich_tree()}, "tree_after": snapshot(root), "dst_after": snapshot(dst)})
    write(out, "take_folders", "latent_takes shift, delete, copy", cases)

    # -- Timeline journal.
    def run_journal(steps):
        results = []
        journal = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proj"
            root.mkdir()
            for step in steps:
                with frozen(step.get("t", 0)):
                    kind = step["op"]
                    if kind == "write":
                        seed(root, {step["path"]: step["content"]})
                    elif kind == "start":
                        journal = timeline.TimelineJournal(str(root), step["name"])
                        journal.start()
                    elif kind == "rename":
                        source, target = str(root / step["from"]), str(root / step["to"])
                        os.rename(source, target)
                        journal.record_renames([(source, target)])
                    elif kind == "commit":
                        journal.commit()
                    elif kind == "rollback":
                        journal.rollback()
                    elif kind == "recover":
                        results.append(timeline.recover_pending_journal(str(root)))
                        continue
                    elif kind == "overwrite_session":
                        seed(root, {"vrgdg_builder_session.json": step["content"]})
                results.append(None)
            return results, snapshot(root)

    def journal_tree_text(tree):
        # journal and backup names carry the frozen clock, so they are stable across runs
        return tree

    base = [
        {"op": "write", "path": "vrgdg_builder_session.json", "content": "{\"revision\": 1}"},
        {"op": "write", "path": "zimage_approved/image_0001.png", "content": "one"},
        {"op": "write", "path": "zimage_approved/image_0002.png", "content": "two"},
    ]
    scenarios = {
        "commit": base + [
            {"op": "start", "name": "create_scene", "t": 1},
            {"op": "rename", "from": "zimage_approved/image_0002.png", "to": "zimage_approved/image_0003.png", "t": 2},
            {"op": "commit", "t": 3},
        ],
        "rollback_restores_names_and_session": base + [
            {"op": "start", "name": "delete_scene", "t": 1},
            {"op": "rename", "from": "zimage_approved/image_0002.png", "to": "zimage_approved/image_0003.png", "t": 2},
            {"op": "rename", "from": "zimage_approved/image_0001.png", "to": "zimage_approved/image_0002.png", "t": 3},
            {"op": "overwrite_session", "content": "{\"revision\": 99}", "t": 4},
            {"op": "rollback", "t": 5},
        ],
        "recover_after_crash": base + [
            {"op": "start", "name": "split_scene", "t": 1},
            {"op": "rename", "from": "zimage_approved/image_0002.png", "to": "zimage_approved/image_0003.png", "t": 2},
            {"op": "overwrite_session", "content": "{\"revision\": 50}", "t": 3},
            {"op": "recover", "t": 4},
            {"op": "recover", "t": 5},
        ],
        "recover_nothing_pending": base + [{"op": "recover", "t": 1}],
        "rollback_skips_blocked_rename": base + [
            {"op": "start", "name": "merge_scenes", "t": 1},
            {"op": "rename", "from": "zimage_approved/image_0002.png", "to": "zimage_approved/image_0003.png", "t": 2},
            {"op": "write", "path": "zimage_approved/image_0002.png", "content": "a new file took the old name"},
            {"op": "rollback", "t": 3},
        ],
    }
    cases = []
    for name, steps in scenarios.items():
        results, tree = run_journal(steps)
        cases.append({"input": {"name": name, "steps": steps}, "results": results, "tree_after": tree})
    write(out, "journal", "TimelineJournal and recover_pending_journal", cases)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
