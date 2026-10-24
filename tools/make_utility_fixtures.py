"""Dev tool: golden graphs for the clear memory graph, the Beat Mode graph and the MiniMax draft graphs, from the pack.

    python tools/make_utility_fixtures.py --pack <pack folder>

Writes ``fixtures/pipeline/utility_graphs.json``. The draft cases start from the two pass graphs in ``fixtures/graphs``, so run
``make_graph_fixtures.py`` first. Draft ids and folders are random, so they are recorded as ``<ID>`` and ``<FOLDER>``.
"""

import argparse
import glob
import importlib
import json
import os
import sys
import tempfile
import types
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

STAMP = "20260101_000000"


def beat_cases(pc):
    """The Beat Mode graph for several requests. Staging the song is replaced by fixed names."""
    pc._stage_audio_for_upload_node = lambda path: ("song.wav", pack_env.FAKE_INPUT + "\\song.wav")
    pc.time = types.SimpleNamespace(strftime=lambda fmt: STAMP, time=lambda: 0.0)
    requests = [
        {},
        {"full_lyrics": "line one\nline two", "whisper_language": "german", "min_duration": 3, "max_duration": 12, "bias": 0.4},
        {"use_srt_durations": False, "fixed_scene_duration": 6, "empty_segment_text": "Quiet.", "duration_preset": "steady"},
        {"use_srt_durations": "no", "min_duration": 0, "bias": 0, "whisper_language": "  "},
        {"use_srt_durations": "maybe", "empty_segment_text": "  ", "full_lyrics": None},
        {"min_duration": "long"},
    ]
    cases = []
    with tempfile.TemporaryDirectory() as tmp:
        for request in requests:
            payload = {"project_folder": tmp, "audio_path": "song.wav", **request}
            try:
                result = pc._build_whisper_workflow_prompt(payload)
                value = {"prompt": result["prompt"], "source_srt_filename": result["source_srt_filename"]}
            except Exception as exc:  # noqa: BLE001
                value = {"error": str(exc)}
            cases.append({"request": request, "return": value})
    return cases


def normalize(value, folder, draft_id):
    text = json.dumps(value)
    text = text.replace(json.dumps(folder)[1:-1], "<FOLDER>").replace(draft_id, "<ID>")
    return json.loads(text)


def draft_cases(md):
    cases = []
    for path in sorted(glob.glob(str(Path(__file__).resolve().parents[1] / "fixtures" / "graphs" / "*.json"))):
        data = json.loads(Path(path).read_text(encoding="utf-8"))
        result = data.get("result")
        if not isinstance(result, dict) or "125" not in result.get("prompt", {}) or not ("two_pass" in result or "advanced_two_pass" in result):
            continue
        with tempfile.TemporaryDirectory() as tmp:
            project = os.path.realpath(tmp)
            request = {"project_folder": project, "scene_number": 4, "timeline_start_seconds": 12.5, "timeline_end_seconds": 18.0, "draft_restore_audio": {"audio_path": "x.wav"}}
            draft = md._build_minimax_draft(json.loads(json.dumps(result)), request)
            draft_id = draft["draft_id"]
            folder = os.path.join(project, "minimax_drafts", draft_id)
            saved = json.loads(Path(folder, "workflow.json").read_text(encoding="utf-8"))
            Path(folder, "pass1.pt").write_bytes(b"latent")
            models = md._draft_diffusion_models(saved["prompt"])
            finish = {"project_folder": project, "draft_id": draft_id, "scene_number": 4, "timeline_start_seconds": 12.5, "timeline_end_seconds": 18.0}
            final = md._build_minimax_finalize(finish)
            stale = md._build_minimax_finalize({**finish, "diffusion_model_name": "C:\\other\\not_this_model.safetensors"})
            same = md._build_minimax_finalize({**finish, "diffusion_model_name": "dir/" + (models[0] if models else "none.safetensors")})
            errors = {}
            for name, change in (("scene", {"scene_number": 5}), ("timing", {"timeline_end_seconds": 19.0})):
                try:
                    md._build_minimax_finalize({**finish, **change})
                except Exception as exc:  # noqa: BLE001
                    errors[name] = str(exc)
            cases.append({
                "name": Path(path).stem,
                "result": result,
                "request": {k: v for k, v in request.items() if k != "project_folder"},
                "draft": normalize(draft, folder, draft_id),
                "saved": normalize(saved, folder, draft_id),
                "finalize": normalize(final, folder, draft_id),
                "stale": stale,
                "same_model": normalize(same, folder, draft_id),
                "errors": errors,
            })
    return cases


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "pipeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    uw = importlib.import_module("vrgdg_pack.runner.utility_workflows")
    md = importlib.import_module("vrgdg_pack.runner.minimax_drafts")
    pc = importlib.import_module("vrgdg_pack.prompt_creator.nodes")
    data = {
        "clear_memory": uw._build_clear_memory_prompt()["prompt"],
        "beat": beat_cases(pc),
        "drafts": draft_cases(md),
    }
    (out / "utility_graphs.json").write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"utility_graphs.json: clear memory, {len(data['beat'])} beat cases, {len(data['drafts'])} draft cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
