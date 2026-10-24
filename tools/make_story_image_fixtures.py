"""Dev tool: golden cases for the Storyboard image prompt (`_build_storyboard_image_prompt` in `storyboard/scene_prompts.py`), from the pack's own code with
the LLM replaced by a recorder.

    python tools/make_story_image_fixtures.py --pack <pack folder>

Writes ``fixtures/story/image_prompt.json``. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
MARA = {"id": "mara", "name": "Mara", "description": "a woman with red hair", "image": {"path": "C:/mara.png"}}
GOOD = "They are standing in a warm kitchen. They look at the camera. Their hands rest on the counter and the light is soft."
SCENE = {"id": "s3", "scene_number": 3, "lyrics": "walking home", "subject_refs": [MARA], "location_ref": {"name": "Kitchen", "description": "a warm kitchen", "image": {"path": "C:/kitchen.png"}},
         "shot_type": "medium shot", "story_layer": {"flf_start_state": "She stands at the sink.", "flf_transformation": "The light turns gold.", "flf_end_state": "She sits down.", "flf_carry_forward": "The gold light stays."}}


def bundle(scene=None, **extra):
    return {"scenes": [{**SCENE, **(scene or {})}], "selected_scene_number": 3, **extra}


def cases():
    return [
        ({"storyboard_payload": bundle()}, [GOOD]),
        ({"storyboard_payload": bundle(), "builder_instruction_key": "zimage_t2i", "image_world_style": "Full_Surreal", "image_custom_style_direction": " neon cathedral ", "temperature": 0.6, "max_new_tokens": 500}, [GOOD]),
        ({"scene_bundle": bundle(), "image_world_style": "weird"}, [GOOD]),
        ({"gpt_payload": bundle(image_model_mode="nano_banana")}, [GOOD]),
        ({"storyboard_payload": bundle({"location_ref": {"name": "Dock", "description": "wet planks"}, "subject_refs": [{"name": "Joe", "description": "a man"}, {"name": "Mara", "image": {"data": "x"}}]}, imageMode="flux_klein")},
         ["Using the provided character reference image, create a man on a dock. They are tired."]),
        ({"storyboard_payload": bundle({"subject_refs": [{"name": "Joe", "description": "a man", "image": {"path": "j.png"}}]}, image_model_mode="flow_gpt")}, ["create A man stands. They are tall. Their coat is long."]),
        ({"storyboard_payload": bundle()}, ['{"prompt": "She stands by the sink in a warm kitchen."}']),
        ({"storyboard_payload": bundle()}, ['{"scenes": [{"scene_number": 3, "prompt": "Three."}, {"scene_number": 1, "prompt": "One."}]}']),
        ({"storyboard_payload": bundle(), "flf_image_target": "start"}, [GOOD]),
        ({"storyboard_payload": bundle(), "flf_image_target": "end"}, [GOOD]),
        ({"storyboard_payload": bundle({"story_layer": None, "flf_end_state": "She leaves."}), "flf_image_target": "end"}, [GOOD]),
        ({"storyboard_payload": bundle()}, [""]),
        ({"storyboard_payload": bundle()}, ["Assistant: " + GOOD]),
        ({"storyboard_payload": {"scenes": []}}, []),
        ({"storyboard_payload": "x"}, []),
        ({}, []),
        ({"storyboard_payload": {"scenes": [{"scene_number": 1, "subjects": ["Ann"]}, {"scene_number": 2, "subjects": ["Bob"]}], "selected_scene_number": 2}}, ["They are walking. Their coat is wet."]),
    ]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["storyboard.scene_prompts"])
    function = modules["storyboard.scene_prompts"]._build_storyboard_image_prompt
    out = []
    for request, replies in cases():
        outcome = h.run_case(recorder, function, {**RUNNER, **request}, replies)
        out.append({"fn": "image", "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "image_prompt.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"image_prompt.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
