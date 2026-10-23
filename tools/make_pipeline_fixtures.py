"""Dev tool: golden results for the pipeline decisions (``h3-pipeline``), made by the pack's own code.

    python tools/make_pipeline_fixtures.py --pack <pack folder>

Covers what the pack's orchestrator decides without running ComfyUI or ffmpeg: which scenes a stitch uses and how it is
laid out (``build_stitch_payload``), how a finished clip is recorded on its scene (``apply_scene_video``) and which
continuity a render used (``_continuity_used``). Writes ``fixtures/pipeline/*.json``.
"""

import argparse
import contextlib
import copy
import importlib
import json
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

BASE_TIME = 1_700_000_000


@contextlib.contextmanager
def frozen(t):
    real = time.time
    time.time = lambda: BASE_TIME + t + 0.123
    try:
        yield
    finally:
        time.time = real


def write(out: Path, name: str, function: str, cases: list) -> None:
    (out / f"{name}.json").write_text(json.dumps({"function": function, "cases": cases}, indent=1, sort_keys=False, ensure_ascii=False), encoding="utf-8")
    print(f"{name}: {len(cases)} cases")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "pipeline")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    orch = importlib.import_module("vrgdg_pack.agent_api.orchestrator.video_orchestrator")
    scene_video = importlib.import_module("vrgdg_pack.agent_api.scene_video")

    def segment(i, start, end, video=True, **extra):
        s = {"id": f"seg{i}", "start": start, "end": end}
        if video:
            s["video_path"] = f"clips/video_{i:04d}-audio.mp4"
        s.update(extra)
        return s

    def session(segments, **extra):
        value = {"video_engine": "minimax_h3", "audio_path": "song.wav", "segments": copy.deepcopy(segments), "minimax_h3_settings": {"audio_mode": "input_audio"}}
        value.update(extra)
        return value

    full = [segment(1, 0.0, 3.5), segment(2, 3.5, 7.02), segment(3, 7.02, 10.0), segment(4, 10.0, 14.0, video=False), segment(5, 14.0, 18.25)]
    all_video = [segment(1, 0.0, 3.5), segment(2, 3.5, 7.02), segment(3, 7.02, 10.0)]
    built_in = session(all_video, minimax_h3_settings={"audio_mode": "built_in_audio"})
    mixed = session(all_video, minimax_h3_settings={"audio_mode": "built_in_audio"})
    mixed["segments"][1]["use_scene_minimax_h3_settings"] = True
    mixed["segments"][1]["minimax_h3_settings"] = {"audio_mode": "input_audio"}
    cases = []
    stitch_inputs = [
        ("all_scenes_one_missing_strict", session(full), {}, True),
        ("all_scenes_one_missing_lenient", session(full), {}, False),
        ("contiguous_selection", session(all_video), {"scene_ids": [2, 3]}, True),
        ("selection_by_id_in_wrong_order", session(all_video), {"scene_ids": ["seg3", "seg1", "seg2"]}, True),
        ("selection_not_contiguous_project_song", session(all_video), {"scene_ids": [1, 3]}, True),
        ("selection_not_contiguous_embedded", session(all_video), {"scene_ids": [1, 3], "audio": "embedded"}, True),
        ("built_in_audio_is_embedded_by_auto", built_in, {}, True),
        ("built_in_audio_forced_to_project_song", built_in, {"audio": "project"}, True),
        ("mixed_audio_modes_use_the_song", mixed, {}, True),
        ("embedded_forced", session(all_video), {"audio": "embedded"}, True),
        ("audio_path_override", session(all_video, audio_path=""), {"audio_path": "other.wav"}, True),
        ("no_audio_anywhere", session(all_video, audio_path=""), {}, True),
        ("bad_audio_value", session(all_video), {"audio": "loud"}, True),
        ("bad_audio_value_none_text", session(all_video), {"audio": 5}, True),
        ("unknown_scene", session(all_video), {"scene_ids": [9, "nope"]}, True),
        ("bool_scene_id", session(all_video), {"scene_ids": [True]}, True),
        ("scene_ids_not_a_list", session(all_video), {"scene_ids": "1"}, True),
        ("overlays_not_a_list", session(all_video), {"overlays": "x"}, True),
        ("overlays_listed", session(all_video), {"overlays": [{"path": "a.mp4", "start": 1, "end": 2, "source_start": 0}]}, True),
        ("prefix_not_text", session(all_video), {"output_prefix": 7}, True),
        ("custom_prefix", session(all_video), {"output_prefix": "MyCut"}, True),
        ("no_video_at_all", session([segment(1, 0.0, 3.0, video=False)]), {}, False),
        ("single_scene_preview", session(all_video), {"scene_ids": [3]}, True),
        ("scene_with_rendered_video_path_key", session([{"id": "x", "start": 0, "end": 2, "rendered_video_path": "clips/video_0001-audio.mp4"}]), {}, True),
    ]
    for name, sess, params, strict in stitch_inputs:
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / "proj"
            (folder / "clips").mkdir(parents=True)
            (folder / "song.wav").write_bytes(b"x")
            (folder / "other.wav").write_bytes(b"x")
            s = copy.deepcopy(sess)
            for seg in s["segments"]:
                for key in ("video_path", "rendered_video_path"):
                    if seg.get(key):
                        seg[key] = str(folder / seg[key])
                        Path(seg[key]).write_bytes(b"v")
            if s.get("audio_path"):
                s["audio_path"] = str(folder / s["audio_path"])
            run_params = copy.deepcopy(params)
            if run_params.get("audio_path"):
                run_params["audio_path"] = str(folder / run_params["audio_path"])
            record = {"name": name, "session": sess, "params": params, "strict": strict}
            try:
                payload, summary = orch.build_stitch_payload(str(folder), s, run_params, strict=strict)

                def rel(value):
                    return os.path.relpath(value, folder).replace("\\", "/") if value and str(value).startswith(str(folder)) else value

                payload = dict(payload)
                payload["project_folder"] = "<project>"
                payload["scene_paths"] = [rel(p) for p in payload["scene_paths"]]
                payload["audio_path"] = rel(payload["audio_path"])
                payload["song_path"] = rel(payload["song_path"])
                record["payload"] = payload
                record["summary"] = summary
            except Exception as exc:
                record["error"] = str(exc)
            cases.append(record)
    write(args.out, "stitch_payload", "build_stitch_payload", cases)

    # -- how a finished clip is recorded on its scene
    cases = []
    starts = {
        "empty": {},
        "has_history": {"video_history": ["Z:\\h3\\old.mp4"], "video_thumbnail_history": ["Z:\\h3\\old.jpg"], "video_history_index": 0},
        "history_without_thumbnails": {"video_history": ["Z:\\h3\\old.mp4", "Z:\\h3\\older.mp4"]},
        "already_listed": {"video_history": ["Z:\\H3\\NEW.mp4"], "video_thumbnail_history": ["Z:\\h3\\keep.jpg"]},
        "blank_entries": {"video_history": ["", "Z:\\h3\\a.mp4", "  "]},
    }
    for label, start in starts.items():
        for video, thumb in (("Z:\\h3\\new.mp4", "Z:\\h3\\new.jpg"), ("Z:\\h3\\new.mp4", ""), ("z:/h3/new.mp4", "Z:\\h3\\t.jpg"), ("", "x.jpg")):
            seg = copy.deepcopy(start)
            with frozen(5):
                scene_video.apply_scene_video(seg, video, thumb)
            cases.append({"input": {"start": label, "video": video, "thumbnail": thumb, "segment": start}, "output": seg})
    write(args.out, "apply_scene_video", "scene_video.apply_scene_video", cases)

    # -- the continuity a render really used
    cases = []
    segments = [{"id": "a"}, {"id": "b"}, {"id": "c"}]
    reports = [
        None, {"enabled": True, "predecessor_scene": 1}, {"enabled": True}, {"enabled": False, "reason": "x"},
        {"enabled": True, "predecessor_scene": 9}, {"enabled": True, "predecessor_scene": "2"}, {"enabled": True, "predecessor_scene": "x"},
        {"enabled": True, "predecessor_scene": 0},
    ]
    for report in reports:
        for mode in ("off", "latent_continuation_masked", "latent"):
            for scene_number in (1, 2, 3):
                graph = {} if report is None else {"latent_continuation_settings": report}
                out = orch._continuity_used({"continuity_mode": mode}, graph, segments, scene_number)
                cases.append({"input": {"report": report, "mode": mode, "scene_number": scene_number}, "output": out})
    write(args.out, "continuity_used", "_continuity_used", cases)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
