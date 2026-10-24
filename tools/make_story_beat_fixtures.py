"""Dev tool: golden cases for the scene story beat (`_build_story_layer_scene_beat` in `storyboard/story_layer.py`), from the pack's own code with the LLM
replaced by a recorder.

    python tools/make_story_beat_fixtures.py --pack <pack folder>

Writes ``fixtures/story/beat_steps.json``. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
MARA = {"id": "mara", "name": "Mara", "description": "a woman with red hair", "reference_type": "character"}
JOE = {"id": "joe", "name": "Joe", "description": "a tall man in a hat", "reference_type": "character"}
HAT = {"id": "hat", "name": "Red hat", "description": "a red felt hat", "reference_type": "prop", "wears": "mara"}
GOOD = "Mara crosses the small kitchen, lifts a cup from the dish rack, and sets it on the counter as the camera pushes in slowly from the doorway."
SCENE = {"id": "s3", "scene_number": 3, "lyrics": "walking home alone", "lyric_singers": ["Mara"], "subject_refs": [MARA], "location_ref": {"name": "Home - Kitchen", "description": "a warm kitchen with a dish rack"},
         "shot_type": "medium shot", "camera_motion": "slow push in", "character_motion": "calm", "video_prompt_type": "i2v", "vocal_status": {"singers": ["Mara"]}}


def bundle(scene=None, **extra):
    return {"scenes": [{**SCENE, **(scene or {})}], "selected_scene_number": 3, **extra}


STORY_LAYER = {"story_flow_version": 2, "story_book": {"title": "T", "premise": "A walk.", "chapters": [{"id": "ch_1", "title": "Home", "setting_id": "setting_home", "paragraphs": ["Mara wakes."]}]},
               "story_settings": [{"id": "setting_home", "name": "Home", "look": "warm wood"}], "story_acts": [{"id": "act_1", "chapter_id": "ch_1", "title": "Wake", "summary": "Mara wakes.", "scene_ids": ["s2", "s3", "s4"]}],
               "story_scene_plan": [{"scene_id": "s2", "act_id": "act_1", "story_slice": "She rises.", "spot": "Bedroom"}, {"scene_id": "s3", "act_id": "act_1", "story_slice": "She makes tea.", "spot": "Kitchen", "mood": "calm",
                                    "continues_previous": True, "continuation_direction": "She sits.", "interaction": "", "user_note": "tea"}, {"scene_id": "s4", "act_id": "act_1", "story_slice": "She leaves.", "spot": "Porch"}],
               "lyric_story_strength": 4, "song_story_brief": "A quiet song."}
OLD_LAYER = {"user_story_arc": "Verse 1:\nScene 3 (Kitchen) — Mara makes tea.", "song_story_brief": "A song about home.", "lyric_story_strength": 8}


def cases():
    base = {"story_layer": OLD_LAYER, "reference_builder": {"subjects": [MARA, JOE, HAT]}}
    out = [
        ({**base, "storyboard_payload": bundle()}, [GOOD]),
        ({**base, "storyboard_payload": bundle(), "previous_beat": "Mara wakes.", "previous_lyrics": "la", "next_lyrics": "next", "current_lyrics": "walking home now", "temperature": 0.5, "top_p": 0.7, "max_new_tokens": 200}, [GOOD]),
        ({**base, "storyboard_payload": bundle()}, ["Beat: " + GOOD]),
        ({**base, "storyboard_payload": bundle()}, ["Mara sings softly in the kitchen while the camera pushes in. The music swells around her."]),
        ({**base, "storyboard_payload": bundle()}, ["Mara sings softly in the kitchen while the camera pushes in. The music swells around her.", GOOD]),
        ({**base, "storyboard_payload": bundle()}, ["Mara sings softly in the kitchen while the camera pushes in. The music swells around her.", "She sings again and the lyrics fill the room."]),
        ({**base, "storyboard_payload": bundle()}, ["Mara walks through a dark warehouse with concrete walls and a loading dock behind her.", GOOD]),
        ({**base, "storyboard_payload": bundle()}, ["Mara walks through a dark warehouse with concrete walls and a loading dock behind her.", "Another warehouse with steel stairs."]),
        ({**base, "storyboard_payload": bundle()}, ["Mara walks through a dark warehouse with concrete walls and a loading dock behind her.", ""]),
        ({**base, "storyboard_payload": bundle({"extra_subjects": [{"name": "Dancer", "count": 3, "interaction": "alongside", "identity": "in red"}, {"title": "Fan", "interaction": "weird"}]})},
         ["Mara crosses the kitchen with the Dancer group beside her while the Fan watches from the door.", ]),
        ({**base, "storyboard_payload": bundle({"extra_subjects": [{"name": "Dancer", "count": 3}]})}, ["Mara crosses the kitchen alone.", "Mara crosses the kitchen with the Dancer group beside her."]),
        ({**base, "storyboard_payload": bundle({"extra_subjects": [{"name": "Dancer", "count": 3}]})}, ["Mara crosses the kitchen alone.", "Mara crosses the kitchen again."]),
        ({**base, "storyboard_payload": bundle()}, ["Mara crosses the kitchen while Joe watches from the door, his hat in his hands.", GOOD]),
        ({**base, "storyboard_payload": bundle()}, ["Mara crosses the kitchen while Joe watches from the door, his hat in his hands.", "Joe walks in again."]),
        ({**base, "storyboard_payload": bundle()}, ["Joe stands in the kitchen. A stranger waits outside.", "Joe again."]),
        ({**base, "storyboard_payload": bundle()}, ["A stranger stands in the kitchen.", ""]),
        ({**base, "storyboard_payload": bundle()}, ["Mara stands in the kitchen full of grief and memory, and the camera drifts past her.", "Mara stands in the kitchen, her hands on the counter, as the camera drifts past."]),
        ({**base, "storyboard_payload": bundle()}, ["Mara stands in the kitchen full of grief and memory, and the camera drifts past her.", "Still grief and longing and memory."]),
        ({**base, "storyboard_payload": bundle({"no_character_present": True, "subject_refs": []})}, ["The empty kitchen is lit by a low window while the camera drifts across the counter."]),
        ({**base, "storyboard_payload": bundle({"subject_refs": [MARA, HAT], "performer_assignment": {"singing": ["Mara"], "silent": ["Red hat", "Joe"]}})}, [GOOD]),
        ({**base, "storyboard_payload": bundle({"scene_override": "it is night and raining", "lyric_singers": [], "vocal_status": {}, "subject_refs": [{"name": "Lead Singer", "description": "a woman"}]})}, [GOOD]),
        ({**base, "story_layer": STORY_LAYER, "storyboard_payload": bundle()}, [GOOD]),
        ({**base, "story_layer": {**STORY_LAYER, "lyric_story_strength": 9}, "storyboard_payload": bundle({"id": "s2", "scene_number": 2})}, [GOOD]),
        ({**base, "story_layer": STORY_LAYER, "storyboard_payload": bundle({"id": "zz", "scene_number": 9})}, [GOOD]),
        ({**base}, []),
        ({**base, "storyboard_payload": {"scenes": []}}, []),
        ({**base, "storyboard_payload": bundle()}, [""]),
        ({**base, "storyboard_payload": bundle()}, ["Mara sings."]),
        ({"story_layer": OLD_LAYER, "storyboard_payload": bundle({"subject_refs": [MARA, JOE]}), "all_subjects": [{"name": "Amy", "description": "a girl"}]}, ["Mara and Joe cross the kitchen together while Amy waits."]),
        ({"story_layer": OLD_LAYER, "storyboard_payload": bundle({"subject_refs": [MARA, JOE]}), "all_subjects": [{"name": "Amy", "description": "a girl"}]}, ["Mara and Joe cross the kitchen together while Amy waits.", "Mara and Joe cross the kitchen together."]),
    ]
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["storyboard.story_layer"])
    function = modules["storyboard.story_layer"]._build_story_layer_scene_beat
    out = []
    for request, replies in cases():
        outcome = h.run_case(recorder, function, {**RUNNER, **request}, replies)
        out.append({"fn": "beat", "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "beat_steps.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"beat_steps.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
