"""Dev tool: golden cases for the story settings (``h3-project::story``), made by the pack's own code.

    python tools/make_story_settings_fixtures.py --pack <pack folder>

Each case saves a session, runs the pack's ``set_story_settings`` and records the result and the three keys it writes.
Writes ``fixtures/timeline/story_settings.json``.
"""

import argparse
import importlib
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

SESSIONS = {
    "empty": {"project_name": "S", "revision": 4, "builder_save_revision": 4, "segments": []},
    "with_layer": {
        "project_name": "S", "revision": 7, "builder_save_revision": 7, "segments": [],
        "builder_story_layer": {"overall_story_idea": "  a boy and a fox ", "lyric_story_strength": 3, "image_world_style": "", "extra_key": 1},
        "builder_storyboard_defaults": {"camera_flow": "slow", "video_style": "noir", "keep": True},
    },
    "camel_layer": {"project_name": "S", "revision": 2, "segments": [], "builderStoryLayer": {"enabled": False, "song_story_brief": " brief "}},
}

PARAMS = [
    {},
    {"defaults": {"camera_motion_speed": 6, "video_style": "noir"}},
    {"defaults": {"camera_motion_speed": "7.5", "character_motion_speed": 0, "temporal_background_intensity": 10}},
    {"defaults": {"camera_motion_speed": 11}},
    {"defaults": {"camera_motion_speed": "fast"}},
    {"defaults": {"camera_motion_speed": True}},
    {"defaults": {"nonsense": 1, "alpha": 2}},
    {"defaults": {"temporal_allow_background_extras": True, "temporal_environment_time_passage": False}},
    {"defaults": {"temporal_allow_background_extras": "yes"}},
    {"defaults": {"temporal_protected_characters": "lead_only"}},
    {"defaults": {"temporal_protected_characters": "everyone"}},
    {"defaults": {"temporal_protected_characters": None}},
    {"defaults": {"short_film_planning_mode": "Guided Film"}},
    {"defaults": {"short_film_planning_mode": "custom"}},
    {"defaults": {"short_film_planning_mode": "fully-custom"}},
    {"defaults": {"short_film_planning_mode": "random"}},
    {"defaults": {"story_arc_detail": "rich"}},
    {"defaults": {"story_arc_detail": "huge"}},
    {"story": {"overall_story_idea": "A boy and a fox", "lyric_story_strength": 9, "image_world_style": "surreal_subject"}},
    {"story": {"enabled": "no"}},
    {"story": {"enabled": False}},
    {"story": {"image_world_style": "pastel"}},
    {"story": {"lyric_story_strength": 12}},
    {"story": {"user_story_arc": "x", "song_story_brief": "y", "image_custom_style_direction": "z"}},
    {"story": {"mystery": 1}},
    {"defaults": {"video_style": "a"}, "story": {"overall_story_idea": "b"}},
    {"defaults": "not a dict", "story": ["nor this"]},
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "timeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    mut = importlib.import_module("vrgdg_pack.agent_api.mutations")
    project = importlib.import_module("vrgdg_pack.builder.project")
    project._rehydrate_builder_session = lambda folder, session: session
    mut.notify_project_changed = lambda *a, **k: None
    so = importlib.import_module("vrgdg_pack.agent_api.orchestrator.storyboard_orchestrator")

    cases = []
    for name, session in SESSIONS.items():
        for params in PARAMS:
            for if_match in (None, int(session["revision"]), 99):
                with tempfile.TemporaryDirectory() as tmp:
                    folder = Path(tmp) / "p"
                    folder.mkdir()
                    (folder / "vrgdg_builder_session.json").write_text(json.dumps(session, indent=2), encoding="utf-8")
                    mut.resolve_project_folder = lambda pid, r=str(folder): r
                    entry = {"session": name, "params": params, "if_match": if_match}
                    try:
                        result = so.set_story_settings("p", json.loads(json.dumps(params)), if_match_revision=if_match)
                        saved = json.loads((folder / "vrgdg_builder_session.json").read_text(encoding="utf-8"))
                        entry["return"] = {
                            "result": result,
                            "after": {k: saved.get(k) for k in ("builder_storyboard_defaults", "builder_story_layer", "revision", "builder_save_revision")} | {"has_camel": "builderStoryLayer" in saved},
                        }
                    except Exception as exc:  # noqa: BLE001
                        entry["return"] = {"error": str(exc), "code": getattr(exc, "code", None)}
                    cases.append(entry)
    (out / "story_settings.json").write_text(json.dumps({"sessions": SESSIONS, "cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"story_settings.json: {len(cases)} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
