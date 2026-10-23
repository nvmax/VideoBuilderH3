"""Dev tool: golden MiniMax prompts for the prompt writer (``h3-pipeline::minimax_prompts``), from the pack.

    python tools/make_prompt_writer_fixtures.py --pack <pack folder>

Each case is a project session, a scene and the shots a stand-in LLM answers. The pack's ``generate_scene_prompt`` turns
them into the final prompt. The Rust writer must produce the same text from the same reply. The request text itself is
not compared: the pack adds the Storyboard card JSON to it, which this app has no use for.

Writes ``fixtures/prompt/writer.json``.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

ROOT = "Z:\\h3fixture\\proj"


def base_session(**extra):
    session = {
        "project_folder": ROOT,
        "video_engine": "minimax_h3",
        "minimax_h3_settings": {"audio_mode": "input_audio", "aspect_ratio": "16:9"},
        "segments": [
            {"id": "s1", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "hold me close tonight", "story_beat": "She sings on a roof."},
            {"id": "s2", "start": 4.0, "end": 9.0, "label": "Scene 2", "lyric_text": "[instrumental]", "notes": "slow drift", "i2v_notes": "gentle push in"},
            {"id": "s3", "start": 9.0, "end": 15.5, "label": "Scene 3", "lyric_text": "we run through the night\nand never look back", "camera_motion_speed": 7, "lyric_section": "Chorus"},
        ],
        "flux_reference_builder": {
            "subjects": [{"id": "a", "name": "Ava", "description": "short dark hair, denim jacket", "image": {"path": ROOT + "\\subject_location\\subject\\Ava.png"}}],
            "locations": [{"id": "l1", "name": "Roof", "description": "a rooftop at dusk with string lights", "image": {"path": ROOT + "\\subject_location\\location\\Roof.png"}}],
            "subject_scene_map": {"s1": ["a"], "s2": ["a"], "s3": ["a"]},
            "scene_map": {"s1": "l1", "s2": "l1", "s3": "l1"},
            "use_subject_reference": True,
            "use_location_references": True,
        },
    }
    session.update(extra)
    return session


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "prompt"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    orch = importlib.import_module("vrgdg_pack.agent_api.orchestrator.minimax_prompt_orchestrator")
    sync = importlib.import_module("vrgdg_pack.storyboard.session_sync")
    store = importlib.import_module("vrgdg_pack.storyboard.persistence")
    store._load_storyboard = lambda payload: {"exists": False}
    orch.prepare_llm_payload = lambda body: body
    orch.llm_payload_from_session = lambda session, overrides=None: {}

    state = {"reply": ""}
    orch.vid_gen._generate_builder_t2v_prompt = lambda request: {"prompt": state["reply"]}

    def shots_json(count, extra=""):
        lines = [f"<Subject 1> (Ava) stands on the roof, wind lifting the denim jacket, the camera drifting closer in a steady glide{extra}, shot {i + 1}." for i in range(count)]
        return json.dumps({"shots": [{"description": text} for text in lines]})

    variants = {
        "plain": {},
        "structured": {"use_structured_outputs": True},
        "cuts_5": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 5}},
        "cuts_10_style": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 10, "video_style": "cinematic_realism"}},
        "custom_style_fx": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 3, "video_style": "custom", "video_style_custom": "Neon noir", "fx_preset": "lighting"}},
        "temporal": {"builder_storyboard_defaults": {"temporal_world_effect": "frozen_world", "minimax_h3_cut_frequency": 2}},
        "structured_all": {"use_structured_outputs": True, "builder_storyboard_defaults": {"minimax_h3_cut_frequency": 4, "video_style": "dark_fantasy", "fx_preset": "glitch", "temporal_world_effect": "day_night_sweep"}},
        "built_in_audio": {"minimax_h3_settings": {"audio_mode": "built_in_audio", "aspect_ratio": "9:16 (Portrait Widescreen)"}},
    }
    cases = []
    for name, extra in variants.items():
        session = base_session(**copy.deepcopy(extra))
        cards = sync.scene_cards(session)
        for index, segment in enumerate(session["segments"]):
            defaults = session.get("builder_storyboard_defaults") or {}
            freq = int(defaults.get("minimax_h3_cut_frequency") or 0)
            pa = importlib.import_module("vrgdg_pack.minimax.prompt_assembly")
            duration = segment["end"] - segment["start"]
            count = pa.storyboard_cut_plan_for_duration(duration, freq)["shot_count"]
            state["reply"] = shots_json(count)
            entry = {"name": f"{name}_scene{index + 1}", "session": session, "index": index, "reply": state["reply"]}
            try:
                result = orch.generate_scene_prompt(session, ROOT, segment, cards[index], index, {})
                entry["prompt"] = result["prompt"]
                entry["characters"] = result["characters"]
                entry["shots"] = result["shots"]
            except Exception as exc:  # noqa: BLE001
                entry["error"] = str(exc)
            cases.append(entry)
    # An unmapped scene and a continued scene.
    unmapped = base_session()
    unmapped["flux_reference_builder"]["subject_scene_map"] = {}
    unmapped["flux_reference_builder"]["scene_map"] = {}
    unmapped["flux_reference_builder"]["subjects"] = []
    unmapped["flux_reference_builder"]["locations"] = []
    cards = sync.scene_cards(unmapped)
    state["reply"] = shots_json(1)
    entry = {"name": "unmapped", "session": unmapped, "index": 0, "reply": state["reply"]}
    try:
        orch.generate_scene_prompt(unmapped, ROOT, unmapped["segments"][0], cards[0], 0, {})
    except Exception as exc:  # noqa: BLE001
        entry["error"] = str(exc)
    cases.append(entry)
    (out / "writer.json").write_text(json.dumps({"cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print("writer cases:", len(cases), "errors:", sum(1 for c in cases if "error" in c))
    for c in cases:
        if "error" in c:
            print("  ", c["name"], "->", c["error"][:110])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
