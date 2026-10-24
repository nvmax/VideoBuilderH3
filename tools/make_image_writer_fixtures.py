"""Dev tool: golden cases for the Builder's image prompt writers, from the pack's own code.

    python tools/make_image_writer_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/image_writers.json``: for each case the request, the canned LLM replies, what the writer asked the LLM
(instruction text, sampling values, picture sizes) and what it returned or the error it raised. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
GOOD = "A cinematic medium shot of a woman in a rain soaked neon alley, wet asphalt reflecting pink signs, shallow depth of field, soft haze and warm rim light."
JSON_REPLY = json.dumps({"scenes": [{"scene_number": 1, "prompt": "Scene one prompt text."}, {"scene_number": 2, "prompt": GOOD}]})
JUNK = "thought thought thought thought thought thought thought thought thought thought thought thought"
TEMPLATE = "A shot of [Subject] in [setting/environment] during [time/weather] with [Camera Motion]."
REPAIRED = "A woman stands in a rain soaked neon alley at night, slow push in, pink reflections on wet asphalt."

CONTEXT = {"subject_description": "a tall woman with short silver hair and a black leather jacket", "location_name": "Neon alley", "location_description": "a narrow wet alley lit by pink and blue signs", "has_subject_reference": True, "has_location_reference": True, "subject_reference_count": 2}


def cases(files, picture):
    theme, story, subject = files["theme.txt"], files["story.txt"], files["subject.txt"]
    t2i = [
        ({"user_notes": "She waits under a flickering sign."}, [GOOD]),
        ({"user_notes": "She waits.", "theme_style_path": theme, "story_idea_path": story, "subject_scene_path": subject}, [GOOD]),
        ({"user_notes": "Walking home.", "builder_instruction_key": "zimage_t2i"}, [GOOD]),
        ({"user_notes": "Walking home.", "prompt_mode": "flux_klein", "reference_context": CONTEXT}, [GOOD]),
        ({"user_notes": "Walking home.", "prompt_mode": "flux_klein", "reference_context": {"subject_description": "a man in a red coat"}, "builder_instruction_key": "flux_klein_t2i"}, [GOOD]),
        ({"user_notes": "Walking home.", "prompt_mode": "nano_banana", "reference_context": CONTEXT}, [GOOD]),
        ({"user_notes": "Walking home.", "prompt_mode": "nano_banana", "reference_context": {"has_location_reference": True, "location_name": "Dock"}}, [GOOD]),
        ({"user_notes": "Walking home.", "prompt_mode": "nano_banana"}, [GOOD]),
        ({"user_notes": "Match this look.", "use_vision": True, "ref_image_data": picture}, [GOOD]),
        ({"use_vision": True, "ref_image_data": picture, "temperature": 0.9}, [JSON_REPLY]),
        ({"user_notes": "Walking home.", "scene_number": 2}, [JSON_REPLY]),
        ({"user_notes": "Walking home."}, [JUNK, REPAIRED]),
        ({"user_notes": "Walking home."}, [TEMPLATE, REPAIRED]),
        ({"user_notes": "Walking home.", "lyric_text": "She walks home alone in the rain tonight"}, ["She walks home alone in the rain tonight", REPAIRED]),
        ({"user_notes": "Walking home.", "video_type": "no_lip_sync"}, ["A woman sings softly while walking. A quiet alley at night with warm lamps. No people look at the camera."]),
        ({"user_notes": ""}, []),
        ({"use_vision": True, "user_notes": "x"}, []),
        ({"user_notes": "Walking home."}, [""]),
    ]
    edit = [
        ({"current_prompt": "A woman in an alley.", "edit_request": "Make it night."}, [GOOD]),
        ({"current_prompt": "A woman in an alley.", "edit_request": "Make it night.", "mode_label": "Flux/Klein", "prompt_mode": "flux_klein", "reference_context": CONTEXT}, [GOOD]),
        ({"current_prompt": "Using the provided character reference image, create a woman.", "edit_request": "Add rain.", "prompt_mode": "nano_banana", "reference_context": CONTEXT}, [GOOD]),
        ({"current_prompt": "A woman.", "edit_request": "Add rain.", "prompt_mode": "flow_gpt", "reference_context": {"has_location_reference": True}}, ["Using the provided scene reference, make a woman in rain."]),
        ({"current_prompt": "A woman.", "edit_request": "Add rain.", "prompt_mode": "krea2_2pass"}, [GOOD]),
        ({"current_prompt": "A woman.", "edit_request": "Add rain.", "use_full_scene_context": True, "scene_context": {"label": "Scene 3", "scene_notes": "rainy", "lyric_text": "la la", "no_character_present": False}}, [GOOD]),
        ({"current_prompt": "A woman.", "edit_request": "Add rain.", "use_vision_reference": True, "ref_image_data": picture}, [GOOD]),
        ({"current_prompt": "A woman.", "edit_request": "Add rain.", "use_vision_reference": True}, []),
        ({"current_prompt": "", "edit_request": "x"}, []),
        ({"current_prompt": "A woman.", "edit_request": ""}, []),
    ]
    flux = [
        ({"user_notes": "She looks up.", "image_ingredients": [{"data": picture}], "reference_context": CONTEXT}, [GOOD]),
        ({"user_notes": "", "image_ingredients": [{"data": picture}, {"data": picture}, {"data": picture}]}, [GOOD]),
        ({"user_notes": "Close up.", "image_ingredients": [{"data": picture}] * 5, "builder_instruction_key": "flux_klein_t2i"}, [JUNK, REPAIRED]),
        ({"user_notes": "Close up.", "image_ingredients": []}, [GOOD]),
        ({"user_notes": "Close up.", "image_ingredients": "Z:/missing/a.png"}, []),
        ({"user_notes": "Close up.", "image_ingredients": {"a": 1}}, []),
    ]
    nano = [
        ({"user_notes": "Walking home.", "reference_context": CONTEXT}, [GOOD]),
        ({"user_notes": "Walking home.", "reference_context": {"has_subject_reference": True, "subject_reference_count": 1, "subject_description": "a man"}}, ["Using the provided character reference and location reference, create a man walking."]),
        ({"user_notes": "Walking home.", "reference_context": {"has_location_reference": True, "location_name": "Dock", "location_description": "wet planks"}}, ["Using the provided character reference and location reference, create a dock scene. and the character reference image fades"]),
        ({"user_notes": "Walking home."}, ["Using the provided character reference, create a quiet street."]),
        ({"user_notes": "Walking home.", "image_ingredients": [{"data": picture}]}, ["Using the provided location reference, create a quiet street."]),
        ({"user_notes": "Walking home.", "image_ingredients": [{"data": picture}], "reference_context": CONTEXT}, ["CAMERA COMPOSIITION (PRIORITY)\nCHARACTER REFERENCCE\nSCEENE REFERENCCE a reference imagae of a street"]),
        ({"user_notes": "Walking home.", "prompt_mode": "flow_gpt", "reference_context": CONTEXT}, [GOOD]),
        ({"user_notes": "Walking home.", "prompt_mode": "flow_gpt", "image_ingredients": [{"data": picture}]}, [GOOD]),
        ({"user_notes": "Walking home.", "reference_context": {"has_subject_reference": True}}, [""]),
        ({"user_notes": "Walking home.", "reference_context": {"has_subject_reference": True}}, [JUNK, REPAIRED]),
    ]
    stories = [
        ({"user_notes": "Characters and places.", "image_ingredients": [{"data": picture}]}, [GOOD]),
        ({"user_notes": "", "image_ingredients": [{"data": picture}] * 6}, [GOOD, "Second batch notes."]),
        ({"image_ingredients": [{"data": picture}] * 9}, [GOOD, GOOD, JUNK]),
        ({"image_ingredients": []}, []),
    ]
    concept_scenes = [
        {"scene_number": 1, "label": "Verse", "lyric_text": "la la", "lyric_singers": ["Ann"], "mapped_subjects": "Ann", "timeline_notes": [{"label": "note", "note": "rain", "start": 0, "end": 2}]},
        {"scene_number": 2, "lyric_instrumental": True, "no_character_present": True, "director_note": "empty street", "location_reference_mode": "map"},
    ]
    concepts = [
        ({"scenes": concept_scenes, "story_idea": "A long night.", "theme_style": "Noir.", "previous_summary": "Scene1: rain"}, ['{"Scene1": "A woman in the rain. Location: alley.", "Scene2": "Empty street. Location: street."}']),
        ({"scenes": concept_scenes, "source_mode": "lyrics"}, ['```json\n{scene 1: "x", Scene2: "y",}\n```']),
        ({"scenes": concept_scenes, "source_mode": "unknown"}, ['Scene1: "plain one" Scene 2: "plain two"']),
        ({"scenes": concept_scenes}, ["nothing useful"]),
        ({"scenes": []}, []),
        ({"scenes": [5]}, []),
        ({"scenes": [{"scene_number": "x"}], "temperature": 0.8, "max_new_tokens": 500}, ['{"Scene1": "ok"}']),
    ]
    return [("t2i", c) for c in t2i] + [("edit", c) for c in edit] + [("flux", c) for c in flux] + [("nano", c) for c in nano] + [("stories", c) for c in stories] + [("concepts", c) for c in concepts]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.image_prompt_generation"])
    ipg = modules["llm.image_prompt_generation"]
    functions = {
        "t2i": ipg._generate_builder_t2i_prompt,
        "edit": ipg._edit_builder_image_prompt,
        "flux": ipg._generate_flux_klein_prompt,
        "nano": ipg._generate_nb_image_prompt,
        "stories": ipg._analyze_builder_story_references,
        "concepts": ipg._generate_builder_concept_prompts,
    }
    folder, files = h.temp_files({"theme.txt": "Noir, neon, rain.\n", "story.txt": "A woman walks home.\n", "subject.txt": "The singer: Ann.\n"})
    picture = h.png_data_url(40, 30)
    out = []
    with folder:
        for name, (request, replies) in cases(files, picture):
            payload = {**RUNNER, **request}
            outcome = h.run_case(recorder, functions[name], payload, replies)
            out.append({"fn": name, "request": h.scrub(request, folder.name), "replies": replies, **h.scrub(outcome, folder.name)})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "image_writers.json"
    target.write_text(json.dumps({"picture": picture, "cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"image_writers.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
