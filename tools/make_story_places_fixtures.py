"""Dev tool: golden cases for Story Mode places (`storyboard/story_locations.py`) and scene revision (`storyboard/story_revise.py`), from the pack's own
code with the LLM replaced by a recorder.

    python tools/make_story_places_fixtures.py --pack <pack folder>

Writes ``fixtures/story/places_steps.json``. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
MARA = {"id": "mara", "name": "Mara", "description": "a woman with red hair", "reference_type": "character"}
JOE = {"id": "joe", "name": "Joe", "description": "a tall man", "reference_type": "character"}
REFS = {"subjects": [MARA, JOE], "locations": []}
MADE = {"subjects": [MARA, JOE], "locations": [{"id": "l1", "name": "Home - Kitchen", "story_setting_id": "setting_home", "description": "warm kitchen"}, {"id": "l2", "name": "Platform", "story_setting_id": "setting_station",
                                                                                                                      "description": "a platform"}, {"id": "l3", "name": "Dock", "description": "user card"}]}
# The same cards after the Reference Builder edited them: none keeps its story link.
LOST_LINKS = {"subjects": [MARA, JOE], "locations": [{k: v for k, v in card.items() if k != "story_setting_id"} for card in MADE["locations"]]}


def scenes():
    return [{"id": f"s{i + 1}", "timeline_start": i * 5.0, "timeline_end": i * 5.0 + 5.0, "lyrics": f"secret lyric {i + 1}"} for i in range(8)]


BOOK = {"title": "T", "premise": "A walk.", "chapters": [
    {"id": "ch_1", "title": "Home", "setting_id": "setting_home", "paragraphs": ["Mara wakes in the kitchen.", "She reads on the porch."], "purpose": "setup", "presence": {}},
    {"id": "ch_2", "title": "Station", "setting_id": "setting_station", "paragraphs": ["Joe waits on the platform."], "purpose": "", "presence": {}},
    {"id": "ch_3", "title": "Back", "setting_id": "", "paragraphs": ["They return."], "purpose": "", "presence": {}}]}
SETTINGS = [{"id": "setting_home", "name": "Home", "look": ""}, {"id": "setting_station", "name": "Station", "look": "grey concrete"}]
ACTS = [{"id": "act_1", "chapter_id": "ch_1", "title": "Wake", "summary": "Mara wakes.", "cast_ids": ["mara"], "scene_ids": ["s1", "s2", "s3"]},
        {"id": "act_2", "chapter_id": "ch_2", "title": "Wait", "summary": "Joe waits.", "cast_ids": ["joe"], "scene_ids": ["s4", "s5"]},
        {"id": "act_3", "chapter_id": "ch_3", "title": "Back", "summary": "They return.", "cast_ids": [], "scene_ids": ["s6", "s7", "s8"]}]


def plan(user_note=""):
    spots = ["Kitchen", "Kitchen", "Porch", "Platform", "Platform", "Kitchen", "", "Kitchen"]
    return [{"scene_id": f"s{i + 1}", "act_id": "act_1" if i < 3 else "act_2" if i < 5 else "act_3", "story_slice": f"Slice {i + 1}.", "spot": spots[i], "mood": "tense", "cast_ids": ["mara"],
             "user_note": user_note if i == 3 else ""} for i in range(8)]


LAYER = {"story_flow_version": 2, "story_book": BOOK, "story_settings": SETTINGS, "story_acts": ACTS, "story_scene_plan": plan(), "story_cast_ids": ["mara", "joe"], "story_main_cast_id": "mara"}
BOOK_ONLY = {"story_flow_version": 2, "story_book": BOOK, "story_settings": SETTINGS}
LOOKS = json.dumps({"settings": [{"name": "Home", "look": "Warm wood and brass, low light."}, {"name": "Station", "look": "Cold grey platform."}, {"name": "Back", "look": "Night."}]})
SPOTS = json.dumps({"spots": [{"name": "Kitchen", "description": "A small kitchen with a dish rack."}, {"name": "Home - Porch", "description": "A wooden porch."}, {"name": "Platform", "description": "A long platform."}]})
PLACES = json.dumps({"settings": [{"name": "Home", "spots": ["Kitchen", "Back Porch", "kitchen"]}, {"name": "Station", "spots": ["Platform"]}, {"name": "Back", "spots": []}]})
REVISE = json.dumps({"scene": {"story_slice": "Joe appears at the door.", "spot": "Porch", "mood": "surprised", "performer": "Joe", "interaction": "He nods."}, "act_summary": "Mara wakes and Joe appears.",
                     "chapter_paragraphs": ["Mara wakes.", "", "Joe appears at the door."], "other_scenes": [{"scene": 2, "reason": "Joe is there now"}, {"scene": 9, "reason": "x"}, {"scene": 1, "reason": "same"}, "no", {"scene": "x"}]})


def cases():
    base = {"scenes": scenes(), "reference_builder": REFS, "lyrics": "secret lyrics", "style_theme": "noir, no warehouses"}
    places = [
        ({**base, "story_layer": LAYER}, [LOOKS, SPOTS, SPOTS, SPOTS]),
        ({**base, "story_layer": LAYER, "regenerate_looks": True}, [LOOKS, SPOTS, SPOTS, SPOTS]),
        ({**base, "story_layer": LAYER}, ["bad", "bad", "bad", "bad"]),
        ({**base, "story_layer": LAYER, "reference_builder": MADE, "map_only": True}, []),
        ({**base, "story_layer": {**LAYER, "story_user_locations": "Kitchen: warm and small\nBack porch: where she reads"}}, [LOOKS, SPOTS, SPOTS, SPOTS]),
        ({**base, "story_layer": {**BOOK_ONLY}, "scenes": []}, [PLACES, LOOKS, SPOTS, SPOTS, SPOTS]),
        ({**base, "story_layer": {**BOOK_ONLY, "story_user_locations": "Kitchen: warm\nGarage"}, "scenes": []}, [PLACES, LOOKS, SPOTS, SPOTS, SPOTS]),
        ({**base, "story_layer": {**BOOK_ONLY}, "scenes": []}, ["bad", "bad", "bad", "bad", "bad"]),
        ({**base, "story_layer": {"story_flow_version": 2, "story_book": {"chapters": [{"id": "ch_1", "title": "Opening", "paragraphs": ["She runs."]}, {"id": "ch_2", "title": "Ending", "paragraphs": ["She stops."]}]}}, "scenes": []},
         [json.dumps({"settings": [{"name": "Opening", "spots": ["Street"]}, {"name": "Ending", "spots": ["Bridge"]}]}), LOOKS, SPOTS, SPOTS]),
        ({**base, "story_layer": {"story_flow_version": 2, "story_user_locations": "Kitchen: warm\nBack porch"}, "scenes": []}, []),
        ({**base, "story_layer": {"story_flow_version": 2, "story_book": {"chapters": [{"id": "c", "title": "", "paragraphs": ["x"]}]}}, "scenes": []}, []),
        ({**base, "story_layer": {"story_flow_version": 2}, "scenes": []}, []),
        ({**base, "story_layer": LAYER, "reference_builder": MADE, "temperature": 0.3, "max_new_tokens": 800}, [LOOKS, SPOTS, SPOTS, SPOTS]),
        # Map only: cards that lost their story link are still places to map to, and the first chapter without a setting maps to any card.
        ({**base, "story_layer": LAYER, "reference_builder": LOST_LINKS, "map_only": True}, []),
        ({**base, "story_layer": {**LAYER, "story_book": {**BOOK, "chapters": [{**BOOK["chapters"][0], "setting_id": ""}, *BOOK["chapters"][1:]]}}, "reference_builder": MADE, "map_only": True}, []),
    ]
    revise = [
        ({**base, "story_layer": {**LAYER, "story_scene_plan": plan("Joe comes in")}, "scene_id": "s4"}, [REVISE.replace('"scene": 2', '"scene": 5')]),
        ({**base, "story_layer": {**LAYER, "story_scene_plan": plan("Joe comes in")}, "scene_id": "s1"}, [REVISE]),
        ({**base, "story_layer": {**LAYER, "story_scene_plan": [{**e, "performer_source": "manual", "performer_ids": ["joe", "mara"], "cast_ids": ["mara", "joe"]} if e["scene_id"] == "s2" else e for e in plan()]}, "scene_id": "s2"}, [REVISE]),
        ({**base, "story_layer": LAYER, "scene_id": "s1"}, [json.dumps({"scene": {"story_slice": ""}})]),
        ({**base, "story_layer": LAYER, "scene_id": "s1"}, ['{"scene": {"story_slice": "Only a slice."}}']),
        ({**base, "story_layer": LAYER, "scene_id": "nope"}, []),
        ({**base, "story_layer": {**LAYER, "story_acts": []}, "scene_id": "s1"}, []),
        ({**base, "story_layer": LAYER, "scene_id": ""}, []),
    ]
    return [("places", c) for c in places] + [("revise", c) for c in revise]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["storyboard.story_locations", "storyboard.story_revise"])
    functions = {"places": modules["storyboard.story_locations"].build_story_locations, "revise": modules["storyboard.story_revise"].revise_story_from_scene}
    out = []
    for name, (request, replies) in cases():
        outcome = h.run_case(recorder, functions[name], {**RUNNER, **request}, replies)
        out.append({"fn": name, "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "places_steps.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"places_steps.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
