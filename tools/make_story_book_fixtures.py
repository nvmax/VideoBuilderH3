"""Dev tool: golden cases for the Story Mode book steps (`storyboard/story_book.py`), from the pack's own code with the LLM replaced by a recorder.

    python tools/make_story_book_fixtures.py --pack <pack folder>

Writes ``fixtures/story/book_steps.json``: for each case the request, the canned LLM replies, what the step asked the LLM and what it returned
or the error it raised. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
MARA = {"id": "mara", "name": "Mara", "description": "a woman with red hair", "reference_type": "character", "image": {"path": "C:/mara.png"}}
JOE = {"id": "joe", "name": "Joe", "description": "a tall man", "reference_type": "character"}
HAT = {"id": "hat", "name": "Red hat", "description": "a red felt hat", "reference_type": "prop"}
REFS = {"subjects": [MARA, JOE, HAT], "locations": [{"id": "l1", "name": "Dock", "description": "wet planks, low light"}, {"id": "l2", "name": "Home - Kitchen", "story_setting_id": "setting_home", "description": "x"}]}
SCENES = [
    {"id": "s1", "lyrics": "walking home alone", "lyric_section": "Verse 1", "timeline_start": 0, "timeline_end": 12},
    {"id": "s2", "lyric_text": "", "lyric_instrumental": True, "timeline_start": 12, "timeline_end": 20},
    {"id": "s3", "lyrics": "the rain keeps falling", "lyric_section": "Chorus", "timeline_start": 20, "timeline_end": 41.5},
    {"id": "s4", "lyrics": "[Instrumental]", "start": 41.5, "end": 60},
    "not a scene",
]
BOOK_TEXT = (
    "Title: The Last Train North\nPremise: Mara follows her father's trail.\n\nAct 1: The Station\nSetting: The Night Train\n\nMara waits on the platform. Joe watches her.\n\nThe train arrives.\n\n"
    "Act 2: Home\nSetting: Home\n\nThe kitchen is cold. Mara reads the letter.\n\nShe leaves."
)
BOOK = {"title": "The Last Train North", "premise": "Mara follows.", "chapters": [
    {"id": "ch_1", "title": "The Station", "setting_id": "setting_the_night_train", "paragraphs": ["Mara waits.", "The train arrives."], "purpose": "setup", "presence": {"mara": "featured"}},
    {"id": "ch_2", "title": "Home", "setting_id": "setting_home", "paragraphs": ["The kitchen is cold."], "purpose": "", "presence": {}}]}
LAYER = {"story_flow_version": 2, "story_book": BOOK, "story_settings": [{"id": "setting_home", "name": "Home", "look": "warm"}], "story_acts": [
    {"id": "act_1", "chapter_id": "ch_1", "title": "Wait", "scene_ids": ["s1"]}, {"id": "act_2", "chapter_id": "ch_2", "title": "Read", "scene_ids": ["s2", "s3"]}],
    "story_scene_plan": [{"scene_id": "s1", "act_id": "act_1"}, {"scene_id": "s2", "act_id": "act_2"}], "story_cast_ids": ["mara", "joe"], "story_main_cast_id": "joe",
    "story_cast_roles": {"mara": "love_interest"}, "story_relationships": [{"a": "mara", "b": "joe", "type": "exes", "note": "estranged"}], "story_character_notes": {"mara": "guarded"},
    "story_user_locations": "Kitchen: warm and small\nBack porch", "story_tone_notes": "noir", "story_video_style": "spoken", "story_act_count": 2, "user_story_arc": "Verse 1:\nScene 1 (Kitchen) - She wakes.\n\n[Chorus]\nShe leaves.",
    "overall_story_idea": "A walk."}
NONE_FOUND = '{"characters": []}'
FOUND = json.dumps({"characters": [{"name": "Zed", "role": "weird", "description": "an old man in a coat"}, {"name": "mara", "role": "main", "description": "dup of cast"}, {"name": "Amy", "role": "background", "description": "a girl"}, "x", {"name": "Zed", "role": "main"}]})
REWRITTEN = BOOK_TEXT.replace("Joe watches her.", "A stranger watches her.")
SPLIT = json.dumps({"chapters": [{"start_paragraph": 1, "title": "Opening", "setting": "Home"}, {"start_paragraph": 3, "title": "Road", "setting": "The Road"}]})


def cases():
    base = {"scenes": SCENES, "reference_builder": REFS, "story_layer": {"story_cast_ids": ["mara", "joe"]}}
    draft = [
        ({**base}, [BOOK_TEXT, NONE_FOUND]),
        ({**base, "story_idea": "A long night.", "tone_notes": "noir", "previous_book": "Old book", "seed": "7", "temperature": 0.9, "top_p": 0.5, "max_new_tokens": 3000}, [BOOK_TEXT, NONE_FOUND]),
        ({**base, "story_layer": LAYER}, [BOOK_TEXT, NONE_FOUND]),
        ({**base, "story_layer": {**LAYER, "story_flow_version": 0, "lyric_story_strength": 9}}, [BOOK_TEXT, NONE_FOUND]),
        ({**base}, [BOOK_TEXT, FOUND, REWRITTEN]),
        ({**base}, [BOOK_TEXT, FOUND, "Act 1: only one\n\ntext"]),
        ({**base}, [BOOK_TEXT, "not json at all"]),
        ({"scenes": SCENES}, [BOOK_TEXT]),
        ({**base}, ["nothing useful here"]),
        ({**base, "scenes": []}, []),
        ({**base, "scene_count": 12, "song_seconds": 200, "scenes": []}, [BOOK_TEXT, NONE_FOUND]),
        ({**base, "lyrics": "la la la", "scenes": [{"id": "s1"}]}, [BOOK_TEXT, NONE_FOUND]),
    ]
    chapter = [
        ({**base, "story_layer": LAYER, "chapter_id": "ch_2", "direction": "Make it darker."}, ["Act 2: Home Again\nSetting: Home\n\nThe kitchen is dark. Mara cries.", NONE_FOUND]),
        ({**base, "story_layer": LAYER, "chapter_id": "ch_1"}, ["Act 1: The Platform\n\nMara waits. A stranger waits too.", FOUND, "Title: x\n\nAct 1: Platform\n\nMara waits alone."]),
        ({**base, "story_layer": LAYER, "chapter_id": "nope"}, []),
        ({**base, "story_layer": LAYER, "chapter_id": "ch_2"}, ["no chapter here"]),
        ({**base, "story_layer": {**LAYER, "story_flow_version": 0}, "chapter_id": "ch_1"}, ["Act 1: X\n\ntext", NONE_FOUND]),
    ]
    split = [
        ({**base, "text": "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.\n\nFourth."}, [SPLIT]),
        ({**base, "text": "Chapter 1: Start\n\nOne.\n\nTwo.\n\nChapter 2: End\nSetting: Beach\n\nThree."}, [SPLIT]),
        ({**base, "text": "Title: Mine\nPremise: A walk.\n\nAct 1: A\n\none\n\nAct 2: B\n\ntwo"}, ["not json"]),
        ({**base, "story_text": "Just one paragraph.", "title": "Given"}, ["not json"]),
        ({**base, "text": ""}, []),
        ({**base, "text": "x", "scene_count": 20}, [json.dumps({"chapters": [{"start_paragraph": 5}, {"start_paragraph": "x"}]})]),
        ({**base, "text": "A\n\nB\n\nC", "story_layer": {"overall_story_idea": "kept"}}, [json.dumps({"chapters": [{"start_paragraph": 2, "title": "T", "setting": "S"}]})]),
    ]
    convert = [
        ({**base, "story_layer": LAYER}, [SPLIT]),
        ({**base, "story_layer": {"user_story_arc": ""}}, []),
    ]
    remove = [
        ({**base, "story_layer": LAYER, "names": ["Zed", " Amy "]}, [REWRITTEN]),
        ({**base, "story_layer": LAYER, "names": ["Zed"]}, ["Act 1: one\n\ntext"]),
        ({**base, "story_layer": LAYER, "names": []}, []),
        ({**base, "story_layer": {}, "names": ["Zed"]}, []),
    ]
    scan = [
        ({**base, "story_layer": LAYER}, [FOUND]),
        ({**base, "story_layer": LAYER}, [NONE_FOUND]),
        ({**base, "story_layer": LAYER, "reference_builder": {**REFS, "extras": [{"name": "Zed"}, "x"]}}, [FOUND]),
        ({**base, "story_layer": {}}, []),
        ({**base, "story_layer": LAYER}, ["[1, 2]"]),
    ]
    return [("draft", c) for c in draft] + [("chapter", c) for c in chapter] + [("split", c) for c in split] + [("convert", c) for c in convert] + [("remove", c) for c in remove] + [("scan", c) for c in scan]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["storyboard.story_book"])
    story_book = modules["storyboard.story_book"]
    functions = {
        "draft": story_book.draft_story_book,
        "chapter": story_book.rewrite_story_chapter,
        "split": story_book.split_pasted_story,
        "convert": story_book.convert_story_arc,
        "remove": story_book.remove_story_people,
        "scan": story_book.scan_story_cast,
    }
    out = []
    for name, (request, replies) in cases():
        outcome = h.run_case(recorder, functions[name], {**RUNNER, **request}, replies)
        out.append({"fn": name, "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "book_steps.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"book_steps.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
