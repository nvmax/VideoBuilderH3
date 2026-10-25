"""Dev tool: golden cases for Scene Defaults, the Theatrical presets and the fresh prompt context (``h3-core::scene_defaults``, ``theatrical``,
``fresh_context``), from the pack.

    python tools/make_scene_defaults_fixtures.py --pack <pack folder>

Writes ``fixtures/prompt/scene_defaults.json``.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

NORMALIZE = [
    {},
    {"camera_flow": "medium_shots", "camera_motion_speed": 0, "minimax_h3_cut_frequency": 4.5, "performance_style": ""},
    {"temporal_allow_background_extras": False, "temporal_background_intensity": 10, "custom_camera_flow_sequence": [{"shot": "a"}, "b"]},
    {"custom_camera_flow_sequence": [1]},
    {"custom_camera_flow_sequence": "x"},
    {"camera_motion_speed": 11},
    {"camera_motion_speed": -1},
    {"camera_motion_speed": True},
    {"camera_motion_speed": "5"},
    {"temporal_allow_background_extras": 1},
    {"video_style": 5},
    {"video_style_custom": "x" * 20001},
    {"video_style_custom": "x" * 20000},
    {"unknown": 1, "camera_flow": "close_ups"},
    {"b_unknown": 1, "a_unknown": 2},
    {"fx_custom_json": "{}", "fx_preset": "none", "image_aesthetic": "film", "image_shot_flow": "mixed", "global_consistency_phrase": "same look"},
    {"facial_performance": "theatrical", "facial_performance_custom": "wry", "performance_style": "theatrical", "video_style": "noir", "video_style_custom": "grain"},
    {"temporal_protected_characters": ["a"], "temporal_protected_custom": "p"},
    [],
    "text",
    None,
    7,
]

CARDS = [
    {},
    {"camera_flow": "pretty_little_malfunction"},
    {"camera_flow": "pretty_little_malfunction", "motion_summary": "slow push in"},
    {"camera_flow": "medium_shots"},
    {"performance_style": "theatrical"},
    {"performance_style": "calm"},
    {"facial_performance": "theatrical"},
    {"facial_performance": "theatrical", "facial_performance_custom": "wry brows"},
    {"facial_performance": "pop_flirty"},
    {"camera_flow": "pretty_little_malfunction", "performance_style": "theatrical", "facial_performance": "theatrical"},
    {"scene_defaults": {"performance_style": "theatrical"}},
    {"scene_defaults": {"facial_performance": "theatrical", "camera_flow": "pretty_little_malfunction"}},
    {"scene_defaults": {"camera_flow": "close_ups"}, "camera_flow": "pretty_little_malfunction"},
    {"scene_defaults": "broken", "performance_style": "theatrical"},
    {"camera_flow": 5, "performance_style": ["theatrical"]},
]

DEFAULTS = [
    None,
    {},
    {"camera_flow": "pretty_little_malfunction"},
    {"camera_flow": "pretty_little_malfunction", "performance_style": "theatrical", "facial_performance": "theatrical"},
    {"performance_style": "calm", "facial_performance": "theatrical", "facial_performance_custom": ""},
    {"facial_performance": "theatrical", "facial_performance_custom": "soft eyes"},
    {"facial_performance": None, "facial_performance_custom": None},
]

SESSIONS = [
    {},
    {"default_facial_performance": "theatrical"},
    {"default_facial_performance": "theatrical", "default_facial_performance_custom": "soft eyes"},
    {"builder_storyboard_defaults": {"camera_flow": "pretty_little_malfunction", "facial_performance": "calm"}, "default_facial_performance": "theatrical"},
    {"builder_storyboard_defaults": "broken", "default_facial_performance": "off"},
    {"builder_storyboard_defaults": {}},
]

SCENE_CARDS = [
    {"label": "A", "video_prompt": "old", "image_prompt": "old still", "story_beat": "b"},
    {"label": "A", "nested": [{"minimax_h3_prompt": "y", "keep": 1}, 3, "x", None], "video_prompt_origin": "gemma", "i2v_prompt_origin": "manual", "has": True},
    {"scenes": [{"id": "s1", "t2i_prompt": "a", "flux_prompt": "b", "nb_prompt": "c", "ernie_prompt": "d", "flow_gpt_prompt": "e", "text_to_image_prompt": "f", "label": "L"},
                {"id": "s2", "t2v_prompt": "g", "i2v_prompt": "h", "minimax_h3_pass2_prompt": "i", "pass2_prompt": "j", "minimax_h3_prompt_origin": "k", "motion_summary": "m"}]},
    [],
    "text",
    None,
    3,
    1.5,
]


def card_text(card):
    return json.dumps(card, indent=2, ensure_ascii=False)


TEXTS = [
    "",
    "no card here",
    "Complete scene_card: not json",
    "Complete scene_card:\n[1, 2]",
    "Complete scene_card:\n" + card_text({"label": "A", "video_prompt": "old", "story_beat": "b"}),
    "Notes first.\n\nComplete scene_card:\n" + card_text({"label": "A", "image_prompt": "old", "story_beat": "b"}) + "\n\nAfter the card.",
    "Two cards.\nComplete scene_card:\n" + card_text({"a": 1, "t2i_prompt": "x"}) + "\nand\nComplete scene_card:   " + card_text({"b": [1.5, 2.0, 1e-05], "i2v_prompt": "y"}) + " tail",
    "Complete scene_card:\n" + card_text({"label": "café ☃", "video_prompt": "x", "emoji": "\U0001f3b5", "tab": "a\tb", "quote": "say \"hi\"", "empty_list": [], "empty_obj": {}}),
    "Complete scene_card:\n" + card_text({"inner": "Complete scene_card:\n{}", "video_prompt": "x"}),
    "Complete scene_card:\n{\"label\": \"compact\", \"video_prompt\": \"x\"} trailing text",
    "Complete scene_card:\n{\"label\": \"broken\", \"video_prompt\": ",
    "Complete scene_card: \n" + card_text({"label": "nbsp", "video_prompt": "x"}),
    "Complete scene_card: " + card_text({"label": "emspace", "video_prompt": "x"}),
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "prompt"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    sd = importlib.import_module("vrgdg_pack.storyboard.scene_defaults")
    th = importlib.import_module("vrgdg_pack.storyboard.theatrical_presets")
    sync = importlib.import_module("vrgdg_pack.storyboard.session_sync")

    data = {"normalize": [], "defaults": [], "lines": [], "fresh_scene": [], "fresh_text": []}
    for value in NORMALIZE:
        for strict in (False, True):
            entry = {"value": value, "strict": strict}
            try:
                entry["return"] = sd.normalize_scene_defaults(copy.deepcopy(value), strict=strict)
            except ValueError as exc:
                entry["error"] = str(exc)
            data["normalize"].append(entry)
    for session in SESSIONS:
        data["defaults"].append({"session": session, "return": th.theatrical_project_defaults(copy.deepcopy(session))})
    for card in CARDS:
        for defaults in DEFAULTS:
            data["lines"].append({"card": card, "defaults": defaults, "return": th.theatrical_direction_lines(copy.deepcopy(card), copy.deepcopy(defaults))})
    for value in SCENE_CARDS:
        data["fresh_scene"].append({"value": value, "return": sync.fresh_prompt_scene_context(copy.deepcopy(value))})
    for text in TEXTS:
        data["fresh_text"].append({"text": text, "return": sync.fresh_prompt_context_text(text)})

    (out / "scene_defaults.json").write_text(json.dumps(data, indent=1, ensure_ascii=False), encoding="utf-8")
    print("scene_defaults.json:", {k: len(v) for k, v in data.items()})
    return 0


if __name__ == "__main__":
    sys.exit(main())
