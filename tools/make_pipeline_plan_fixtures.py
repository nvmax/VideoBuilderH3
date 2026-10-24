"""Dev tool: golden pipeline plans and song scene boundaries (``h3-pipeline::pipeline_plan``), from the pack.

    python tools/make_pipeline_plan_fixtures.py --pack <pack folder>

Writes ``fixtures/pipeline/plan.json``. A session audio path of ``@exists@`` stands for a real file the test makes.
"""

import argparse
import copy
import importlib
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

SESSIONS = {
    "empty": {"segments": []},
    "no_audio": {"segments": [{"id": "s1", "start": 0, "end": 4}]},
    "missing_audio": {"audio_path": "Y:\\nowhere\\song.wav", "segments": [{"id": "s1", "start": 0, "end": 4}]},
    "mixed": {
        "audio_path": "@exists@", "video_engine": "minimax_h3", "video_mode": "minimax_h3",
        "segments": [
            {"id": "s1", "start": 0, "end": 4, "t2i_prompt": "a roof", "minimax_h3_prompt": "p", "approved_image_path": "x.png", "video_path": "v.mp4"},
            {"id": "s2", "start": 4, "end": 9, "selected": True, "custom_image_path": "y.png"},
            {"id": "s3", "start": 9, "end": 9, "image_history": ["a.png"], "video_history": ["b.mp4"], "i2v_prompt": "ltx"},
            {"start": 9, "end": 12.5, "t2i_prompt": "   ", "minimax_h3_prompt": "  "},
            {"id": "s5", "start": 12.5, "end": 20, "selected": True, "rendered_video_path": "r.mp4", "t2i_prompt": "x", "minimax_h3_prompt": "y"},
        ],
    },
    "ltx": {"audio_path": "@exists@", "video_mode": "i2v", "image_model_mode": "flux_klein", "segments": [{"id": "a", "start": 0, "end": 3}, {"id": "b", "start": 3, "end": 6}]},
    "nested_image_mode": {"audio_path": "@exists@", "settings": {"image_model_mode": "FLUX_X"}, "video_engine": "ltx", "segments": [{"id": "a", "start": 0, "end": 3}]},
    "reference_mode": {"audio_path": "@exists@", "video_mode": "rtv", "segments": [{"id": "a", "start": 0, "end": 3}, {"id": "b", "start": 3, "end": 6}]},
}

PARAMS = [
    {},
    {"build_mode": "fresh_rebuild"},
    {"build_mode": "redo_videos"},
    {"build_mode": "redo_i2v_prompts_videos"},
    {"build_mode": " Resume_Missing "},
    {"scope": "selected"},
    {"scope": "from_selected"},
    {"scope": "bogus"},
    {"scene_ids": ["s2", "4"]},
    {"scene_ids": "s1, 3 ,"},
    {"scene_ids": []},
    {"scene_ids": ["nope"]},
    {"stitch": False},
    {"stitch": "false"},
    {"video_mode": "t2v"},
    {"video_mode": "RTV"},
    {"video_mode": "MiniMax_H3"},
    {"pipeline": "flf"},
    {"pipeline": "Full_Video", "image_mode": "Flux2"},
    {"build_mode": "fresh_rebuild", "scope": "selected", "stitch": True},
]

DURATIONS = [0, -3, 0.5, 1, 1.9, 3.99, 4, 4.5, 5, 5.01, 7.9, 8, 8.99, 9, 12.34, 59.9, 183.27, 240]
SCENE_SECONDS = [None, 0, 0.2, 1, 2, 3.5, 4, 6, 10, 400]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "pipeline"
    out.mkdir(exist_ok=True)
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    po = importlib.import_module("vrgdg_pack.agent_api.orchestrator.pipeline_orchestrator")

    plans = []
    with tempfile.TemporaryDirectory() as tmp:
        audio = Path(tmp) / "song.wav"
        audio.write_bytes(b"RIFF")
        for name, base in SESSIONS.items():
            for params in PARAMS:
                session = json.loads(json.dumps(base).replace("@exists@", str(audio).replace("\\", "\\\\")))
                po._get_active_session_and_folder = lambda project_id, session=session: ("Z:\\h3fixture", copy.deepcopy(session))
                result = po.get_pipeline_plan("proj", params)
                plans.append({"session": name, "params": params, "return": result})
    boundaries = []
    for duration in DURATIONS:
        for seconds in SCENE_SECONDS:
            args_ = (duration,) if seconds is None else (duration, seconds)
            boundaries.append({"duration": duration, "scene_seconds": seconds, "return": po.plan_scene_boundaries(*args_)})
    data = {"sessions": SESSIONS, "plans": plans, "boundaries": boundaries}
    (out / "plan.json").write_text(json.dumps(data, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"plan.json: {len(plans)} plans, {len(boundaries)} boundary cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
