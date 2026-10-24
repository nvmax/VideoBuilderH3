"""Dev tool: golden cases for the Story Mode acts and scene plan (`storyboard/story_plan.py`), from the pack's own code with the LLM replaced by a recorder.

    python tools/make_story_plan_fixtures.py --pack <pack folder>

Writes ``fixtures/story/plan_steps.json``: for each case the request, the canned LLM replies, what the step asked the LLM and what it returned
or the error it raised. See ``llm_harness.py``.
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
HAT = {"id": "hat", "name": "Red hat", "description": "a red felt hat", "reference_type": "prop"}
LOCATIONS = [{"id": "l1", "name": "Home - Kitchen", "story_setting_id": "setting_home", "description": "warm kitchen"}, {"id": "l2", "name": "Home - Porch", "story_setting_id": "setting_home", "description": "back porch"},
             {"id": "l3", "name": "Platform", "story_setting_id": "setting_station", "description": "a platform"}, {"id": "l4", "name": "Dock", "description": "loaded by the user"}]
REFS = {"subjects": [MARA, JOE, HAT], "locations": LOCATIONS}
SECTIONS = ["Intro", "Verse 1", "Verse 1", "Chorus", "Chorus", "Verse 2", "Verse 2", "Chorus", "Bridge", "Bridge", "Outro", "Outro"]


def scenes():
    out = []
    for index in range(12):
        scene = {"id": f"s{index + 1}", "timeline_start": index * 5.0, "timeline_end": index * 5.0 + 5.0, "lyric_section": SECTIONS[index], "lyrics": f"line {index + 1}" if index not in (0, 10) else "", "lyric_instrumental": index in (0, 10)}
        if index == 3:
            scene["timeline_note"] = "wide shot of the street"
            scene["scene_override"] = "it is raining"
        if index == 5:
            scene["i2v_notes"] = "slow dolly"
            scene["notes"] = "sad"
        out.append(scene)
    return out


BOOK = {"title": "T", "premise": "A walk.", "chapters": [
    {"id": "ch_1", "title": "Home", "setting_id": "setting_home", "paragraphs": ["Mara wakes.", "She reads.", "She leaves."], "purpose": "setup", "presence": {}},
    {"id": "ch_2", "title": "Station", "setting_id": "setting_station", "paragraphs": ["Joe waits.", "The train comes."], "purpose": "", "presence": {}},
    {"id": "ch_3", "title": "Back", "setting_id": "setting_home", "paragraphs": ["They return.", "Quiet.", "The end."], "purpose": "resolution", "presence": {}}]}
LAYER = {"story_flow_version": 2, "story_book": BOOK, "story_settings": [{"id": "setting_home", "name": "Home", "look": "warm"}, {"id": "setting_station", "name": "Station", "look": ""}],
         "story_cast_ids": ["mara", "joe"], "story_main_cast_id": "mara", "story_video_style": "music_video", "lyric_story_strength": 6, "song_story_brief": "A song."}


def acts_layer():
    acts = [
        {"id": "act_1", "chapter_id": "ch_1", "title": "Wake", "summary": "Mara wakes.", "cast_ids": ["mara"], "scene_ids": ["s1", "s2", "s3"]},
        {"id": "act_2", "chapter_id": "ch_1", "title": "Leave", "summary": "She leaves.", "cast_ids": ["mara", "joe"], "scene_ids": ["s4", "s5"], "location_note": "kitchen", "direction": "she cries"},
        {"id": "act_3", "chapter_id": "ch_2", "title": "Wait", "summary": "Joe waits.", "cast_ids": ["joe"], "scene_ids": ["s6", "s7", "s8"]},
        {"id": "act_4", "chapter_id": "ch_3", "title": "Return", "summary": "They return.", "cast_ids": [], "scene_ids": ["s9", "s10", "s11", "s12"]},
    ]
    return {**LAYER, "story_acts": acts}


def plan_reply(numbers, **override):
    items = []
    for number in numbers:
        items.append({"scene": number, "story_slice": f"Slice {number}.", "spot": "Kitchen" if number % 2 else "The porch", "mood": "tense", "performance": "singing", "continues": number % 3 == 0,
                      "continuation": "She sets it down.", "cast": ["Mara", "joe"] if number % 2 else ["Mara"], "performer": "Mara", "interaction": "A glance." if number % 2 else ""})
    items[0].update(override)
    return json.dumps({"scenes": items})


def cases():
    base = {"scenes": scenes(), "reference_builder": REFS, "story_layer": LAYER}
    acts = [
        ({**base}, ['{"acts": [{"first_scene": 1, "title": "Wake", "summary": "Mara wakes.", "cast": ["Mara"]}, {"first_scene": 3, "title": "Out", "summary": "She goes.", "cast": ["mara", "Nobody"]}]}',
                    '{"acts": [{"first_scene": 5, "title": "Wait", "summary": "Joe waits.", "cast": ["Joe"]}]}',
                    '{"acts": [{"first_scene": 7, "title": "Back", "summary": "They return.", "cast": []}, {"first_scene": 10}]}']),
        ({**base}, ["garbage", "more garbage", "nothing"]),
        ({**base}, []),
        ({**base, "chapter_id": "ch_2", "story_layer": acts_layer()}, ['{"acts": [{"first_scene": 6, "title": "Platform", "summary": "Waiting.", "cast": ["Joe"]}]}']),
        ({**base, "story_layer": acts_layer()}, ['{"acts": []}', '{"acts": []}', '{"acts": []}']),
        ({**base, "timeline_markers": [{"start": 8, "end": 17, "note": "a long note", "label": "n"}, {"start": 30, "note": "a point note"}]},
         ['{"acts": [{"first_scene": 1, "title": "A", "summary": "s", "cast": []}, {"first_scene": 2, "title": "B", "summary": "s", "cast": []}]}', "x", "y"]),
        ({**base, "temperature": 0.9, "max_new_tokens": 900}, ["x", "y", "z"]),
        ({**base, "story_layer": {**LAYER, "story_book": {"chapters": []}}}, []),
        ({**base, "scenes": []}, []),
        ({**base, "chapter_id": "zzz"}, []),
        ({**base, "scenes": scenes()[:3]}, ['{"acts": [{"first_scene": 2, "title": "X", "summary": "y", "cast": ["Mara"]}]}', "", ""]),
        ({"scenes": scenes(), "story_layer": LAYER}, ["x", "y", "z"]),
    ]
    plans = []
    full = acts_layer()
    plans.append(({**base, "story_layer": full}, [plan_reply([1, 2, 3]), plan_reply([4, 5]), plan_reply([6, 7, 8]), plan_reply([9, 10, 11, 12])]))
    plans.append(({**base, "story_layer": full}, ["bad", plan_reply([4, 5], performance="Speaking", performer="Joe", cast=["Joe"]), "bad", plan_reply([9, 11], spot="")]))
    plans.append(({**base, "story_layer": full, "act_id": "act_2"}, [plan_reply([4, 5])]))
    plans.append(({**base, "story_layer": full, "scene_ids": ["s2", "s3"], "act_id": "act_1"}, [plan_reply([1, 2, 3])]))
    existing = [{"scene_id": "s4", "act_id": "act_2", "story_slice": "old", "spot": "Hall", "continuation_source": "manual", "continues_previous": True, "continuation_direction": "keep", "cast_source": "manual",
                 "cast_ids": ["joe"], "performer_source": "manual", "performer_ids": ["joe", "mara"], "user_note": "joe cries"}]
    plans.append(({**base, "story_layer": {**full, "story_scene_plan": existing}, "act_id": "act_2"}, [plan_reply([4, 5], performer="Mara")]))
    plans.append(({**base, "story_layer": {**full, "story_user_locations": "Kitchen: warm\nBack porch", "story_spot_min_scenes": 2}, "spot_min_scenes": "4"}, [plan_reply([1, 2, 3]), plan_reply([4, 5]), plan_reply([6, 7, 8]), plan_reply([9, 10, 11, 12])]))
    for style in ("spoken", "story_only"):
        plans.append(({**base, "story_layer": {**full, "story_video_style": style, "story_continuation_mode": "location"}}, [plan_reply([1, 2, 3]), plan_reply([4, 5]), plan_reply([6, 7, 8]), plan_reply([9, 10, 11, 12])]))
    plans.append(({**base, "story_layer": {**full, "story_continuation_mode": "llm"}}, [plan_reply([1, 2, 3]), plan_reply([4, 5]), plan_reply([6, 7, 8]), plan_reply([9, 10, 11, 12])]))
    plans.append(({**base, "story_layer": {**full, "story_acts": []}}, []))
    plans.append(({**base, "story_layer": {**full, "story_acts": [{"id": "act_1", "chapter_id": "ch_1", "scene_ids": ["s1"]}]}}, []))
    plans.append(({**base, "story_layer": full, "act_id": "nope"}, []))
    plans.append(({**base, "story_layer": full, "timeline_markers": [{"start": 15, "end": 22, "note": "rain starts"}]}, [plan_reply([1, 2, 3]), plan_reply([4, 5]), plan_reply([6, 7, 8]), plan_reply([9, 10, 11, 12])]))
    summaries = [
        ({**base, "story_layer": {**full, "story_scene_plan": [{"scene_id": "s4", "act_id": "act_2", "story_slice": "She cries.", "spot": "Kitchen", "cast_ids": ["mara", "joe"], "interaction": "A touch."},
                                                                 {"scene_id": "s5", "act_id": "act_2", "story_slice": "She leaves.", "spot": "", "cast_ids": []}]}, "act_id": "act_2"}, ['{"summary": "She cries and leaves."}']),
        ({**base, "story_layer": {**full, "story_scene_plan": [{"scene_id": "s4", "act_id": "act_2", "story_slice": "x"}]}, "act_id": "act_2"}, ['{"summary": ""}']),
        ({**base, "story_layer": {**full, "story_scene_plan": [{"scene_id": "s4", "act_id": "act_2", "story_slice": "x"}]}, "act_id": "act_2"}, ['{"other": 1}']),
        ({**base, "story_layer": full, "act_id": "act_2"}, []),
        ({**base, "story_layer": full, "act_id": "nope"}, []),
    ]
    return [("acts", c) for c in acts] + [("plan", c) for c in plans] + [("summary", c) for c in summaries]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["storyboard.story_plan"])
    story_plan = modules["storyboard.story_plan"]
    functions = {"acts": story_plan.build_story_acts, "plan": story_plan.build_story_scene_plan, "summary": story_plan.rewrite_act_summary}
    out = []
    for name, (request, replies) in cases():
        outcome = h.run_case(recorder, functions[name], {**RUNNER, **request}, replies)
        out.append({"fn": name, "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "plan_steps.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"plan_steps.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
