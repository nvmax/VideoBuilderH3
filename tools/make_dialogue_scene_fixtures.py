"""Dev tool: golden cases for "Auto-Generate Scenes" of the Speaking mode (`storyboard/dialogue_scenes.py`, MiniMax profile), from the pack's own code.

    python tools/make_dialogue_scene_fixtures.py --pack <pack folder>

Writes three files in ``fixtures/story``:
  dialogue_prompts.json  the planner instruction, schema and repair instruction
  dialogue_helpers.json  the image prompt, camera plan, scene normalizer and the script lock
  dialogue_steps.json    ``_build_minimax_dialogue_scenes`` with the LLM replaced by a recorder (see ``llm_harness.py``)

A reply that is not JSON where the pack parses JSON, and a repair that fails, are left out of the step cases: the parse error text of Python differs from the Rust one.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
OUT = Path(__file__).resolve().parents[1] / "fixtures" / "story"

MARA = {"id": "mara", "name": "Mara", "description": "a woman with red hair and a green coat", "reference_type": "character", "image": {"path": "C:/refs/mara.png", "name": "mara.png"}}
JOE = {"id": "joe", "name": "Joe", "description": "a tall man in a hat", "reference_type": "character"}
KITCHEN = {"id": "kitchen", "name": "Kitchen", "description": "a warm kitchen with a dish rack", "image": {"path": "C:/refs/kitchen.png", "name": "kitchen.png"}}
PORCH = {"id": "porch", "name": "Porch", "description": "a wooden back porch at dusk"}
RICH = ("A cinematic medium close-up of Mara in her green coat at the kitchen window, shallow depth of field, warm tungsten lighting on her red hair, steam rising from a cup, "
        "wet glass behind her with soft bokeh from the street lamps, muted teal and amber palette, fine film texture, calm attentive expression, shot on an 85mm lens, crisp facial detail.")
SCRIPT = {
    "enabled": True, "raw_text": "MARA: Hello.\nJOE: Hi there.\nMARA: Where were you?",
    "cues": [{"speaker_alias": "MARA", "text": "Hello.", "speaker_id": "mara"}, {"speaker_alias": "JOE", "text": "Hi there.", "speaker_id": "joe"}, {"speaker_alias": "MARA", "text": "Where were you?", "speaker_id": "mara"}],
    "scene_plan": {"scenes": [
        {"index": 1, "label": "Arrival", "duration_seconds": 6, "timeline_start_seconds": 0, "timeline_end_seconds": 6, "participant_ids": ["mara", "joe"], "participant_names": ["Mara", "Joe"],
         "speaker_assignments": [{"speaker_id": "mara", "speaker_name": "Mara", "text": "Hello."}, {"speaker_id": "joe", "speaker_name": "Joe", "text": "Hi there."}]},
        {"index": 2, "duration_seconds": 5.5, "timeline_start_seconds": 6, "timeline_end_seconds": 11.5, "continuation_of_previous": True, "participant_ids": ["mara"],
         "speaker_assignments": [{"speaker_id": "mara", "speaker_name": "Mara", "text": "Where were you?"}]},
        {"index": 3, "duration_seconds": 4, "timeline_start_seconds": 11.5, "timeline_end_seconds": 15.5, "speaker_assignments": [{"speaker_alias": "JOE", "text": "Out."}]},
    ]},
}


def plan(scenes, title="The Long Night", premise="Two people meet in a kitchen."):
    return json.dumps({"title": title, "premise": premise, "scenes": scenes})


def scene(n, cues, **extra):
    return {"label": f"Scene {n}", "dialogue_cues": cues, "location_id": "kitchen", "story_beat": f"Beat {n}.", "visual_direction": "Close on the speaker.", "image_prompt": RICH, "shot_type": "medium close-up",
            "camera_motion": "slow push in", "character_motion": "she steps to the sink", "facial_performance": "guarded", "delivery": "quiet", "audio_direction": "rain on glass", "continuity": "green coat", **extra}


CUES = [{"character_id": "mara", "speaker": "Mara", "dialogue": "Where were you?"}, {"character_id": "joe", "speaker": "Joe", "dialogue": "Out."}]


def prompt_cases():
    base = {"has_authoritative_script": False, "camera_flow": "balanced", "camera_motion_speed": 4, "character_motion_speed": 4, "scene_count": 6, "story_source": "", "script_mapper_plan_json": "[none]",
            "story_layer_json": "{}", "project_motion_settings_json": "{}", "subjects_json": "[none provided]", "locations_json": "[none provided]", "compact_existing_json": "[none]"}
    out = [("_storyboard_dialogue_planner_instruction", {"is_minimax": True, **base})]
    out.append(("_storyboard_dialogue_planner_instruction", {"is_minimax": True, **{**base, "has_authoritative_script": True, "story_source": "MARA: Hello.", "script_mapper_plan_json": "{\"scenes\": []}"}}))
    for flow in ("balanced", "Smooth 'flow'", 'He said "go"', "it's \"mixed\"", "back\\slash", "tab\there\nnewline", "caf\u00e9", "nbsp\u00a0here", "zero\u200bwidth", ""):
        out.append(("_storyboard_dialogue_planner_instruction", {"is_minimax": True, **{**base, "camera_flow": flow, "camera_motion_speed": 9, "character_motion_speed": 0, "scene_count": 12, "story_source": "A story."}}))
    out.append(("_storyboard_dialogue_plan_schema", {"is_minimax": True}))
    out.append(("_storyboard_dialogue_json_repair_instruction", {"is_minimax": True, "malformed_text": '{"title": "x", scenes: [}'}))
    out.append(("_storyboard_dialogue_json_repair_instruction", {"is_minimax": True, "malformed_text": ""}))
    return out


def helper_cases():
    out = []

    def add(name, *args):
        out.append((name, list(args)))

    subject = {**MARA}
    location = {**KITCHEN}
    for item in (
        {"image_prompt": RICH}, {"image_prompt": "short prompt"}, {}, {"visual_prompt": RICH}, {"image_prompt": "word " * 60}, {"image_prompt": "word " * 60 + " cinematic"},
        {"character_name": "Ann", "setting": "Roof", "shot_type": "wide", "visual_direction": "She looks over the edge.", "facial_performance": "afraid", "delivery": "whispered"},
        {"speaker": "Bob", "location_name": "Garage", "summary": "He fixes a car.", "emotion": "calm", "character_description": "a mechanic", "location_description": "oily floor"},
        {"story_beat": "She leaves.", "facial_performance_custom": "tears"}, {"beat": "He stays."}, {"image_prompt": "x" * 3500},
    ):
        for subject_ref, location_ref in ((None, None), (subject, None), (None, location), (subject, location), ({"name": "Joe", "description": "a tall man"}, {"name": "Porch", "description": "dusk"}), ("x", 5)):
            add("_id_lora_structured_image_prompt", item, subject_ref, location_ref)
    for value in ("", "slow push in", "dolly in on her face", "zoom-in", "tracking forward", "drift closer", "pull back", "pulls out", "dolly out", "zoom out", "tracking backward", "orbit left", "arc around", "pan right", "lateral drift",
                  "truck left", "rack focus", "focus pull", "locked-off hold", "static", "handheld shake", "an unusual move", "PUSH-IN", None):
        add("_minimax_camera_motion_family", value)
    seq = lambda moves: [{"scene_number": n, "camera_motion": m} for n, m in enumerate(moves, start=1)]  # noqa: E731
    pushes = ["slow push in"] * 8
    for moves in (pushes, ["slow orbit", "", "push in", "pull back", "dolly in", "pan", "zoom in", "static", "tracking in", "orbit"], [""] * 5, ["slow pan", "gentle drift", "subtle handheld movement", "locked-off shot"] * 2, ["restrained pan"] * 3):
        for flow, speed in (("balanced", 4), ("off", 4), ("Off ", 4), ("balanced", 0), ("balanced", 8), ("balanced", 10), ("", 3)):
            add("_rebalance_generated_minimax_camera_motion", seq(moves), flow, speed)
    add("_rebalance_generated_minimax_camera_motion", [5, {"camera_motion": "push in"}, "x", {"camera_motion": "push in"}], "balanced", 4)
    add("_rebalance_generated_minimax_camera_motion", "x", "balanced", 4)
    add("_rebalance_generated_minimax_camera_motion", [], "balanced", 4)
    subjects, locations = [MARA, JOE], [KITCHEN, PORCH]
    raw = [scene(1, CUES), scene(2, [CUES[0]], location_id="porch", camera_motion="dolly in"), scene(3, [CUES[1]], location_id="nowhere"), scene(4, [{"character_id": "ghost", "speaker": "Ghost", "dialogue": "Boo."}])]
    for mode in (None, "text_to_video", "Image-To-Video", "reference to video", "video_to_video", "weird", ""):
        add("_normalize_generated_minimax_dialogue_scenes", raw, subjects, locations, mode, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", raw, subjects, locations, "reference_to_video", "off", 9)
    add("_normalize_generated_minimax_dialogue_scenes", raw, subjects, locations, "text_to_video", "balanced", 0)
    add("_normalize_generated_minimax_dialogue_scenes", raw, [], [], "text_to_video", "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [{"dialogue": "Plain line.", "character_id": "mara", "label": "", "title": "Titled", "id": "custom", "location_name": "Roof"}], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [{"line": "A line", "speaker": "Zed", "beat": "b", "summary": "s", "video_notes": "v", "action": "runs", "emotion": "angry", "delivery": "loud", "duration": 3, "timeline_start": 1.5, "timeline_end": 4.5}], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [{"dialogue_cues": [CUES[0]], "participant_ids": ["joe", "mara", "ghost", "", None, "joe"]}], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [{"dialogue_cues": [{"character_id": "mara", "speaker": "Mara", "dialogue": ""}, 5, {"subject_id": "joe", "text": "Via text."}, {"speaker_id": "mara", "line": "Via line", "character_name": "Mara B"}]}], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [{"dialogue_cues": [], "dialogue": "Fallback."}, {"dialogue_cues": "x", "line": "Other."}, {"dialogue_cues": [{"dialogue": ""}]}], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [scene(n, [CUES[0]]) for n in range(1, 90)], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [5, None, scene(1, CUES)], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [{"dialogue_cues": [{"dialogue": ""}]}], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [], subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", None, subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", "x", subjects, locations, None, "balanced", 4)
    add("_normalize_generated_minimax_dialogue_scenes", [{"dialogue_cues": [CUES[0]], "image_prompt": "short", "audio_direction": "a" * 5000, "notes": "n", "continuity": "c"}], subjects, locations, None, "balanced", 4)
    import copy
    normalized_script = importlib.import_module("vrgdg_pack.storyboard.persistence")._normalize_script_import(SCRIPT)
    for generated in (None, [], [scene(1, CUES)], [scene(1, CUES, location_id="porch"), {"location_id": "kitchen"}, 5], [{"label": "Mine", "notes": "keep"}, {"location_id": "porch"}, {"location_id": "kitchen"}], "x", [scene(n, CUES) for n in range(1, 8)]):
        add("_apply_authoritative_script_plan", generated, copy.deepcopy(normalized_script))
    add("_apply_authoritative_script_plan", [scene(1, CUES, location_id="kitchen")], {"scene_plan": {"scenes": [{"index": 1, "speaker_assignments": [{"speaker_alias": "A", "text": "x"}], "continuation_of_previous": True}, {"index": 2, "continuation_of_previous": True, "speaker_assignments": []}]}})
    add("_apply_authoritative_script_plan", [], None)
    add("_apply_authoritative_script_plan", [], {})
    return out


def step_cases():
    ref = {"subjects": [MARA, JOE], "locations": [KITCHEN, PORCH]}
    base = {"reference_builder": ref}
    good = plan([scene(1, CUES), scene(2, [CUES[0]], location_id="porch")])
    out = [
        ({**base}, [good]),
        ({**base, "story_source": "Two people argue in a kitchen.", "scene_count": 3, "camera_flow": "Smooth", "camera_motion_speed": 8, "character_motion_speed": 2}, [good]),
        ({**base, "storySource": "From camel case.", "sceneCount": "5", "cameraFlow": "off", "cameraMotionSpeed": 0, "characterMotionSpeed": 9}, [good]),
        ({**base, "story_layer": {"user_story_arc": "Verse 1: she waits.", "song_story_brief": "A brief."}}, [good]),
        ({**base, "story_layer": {"song_story_brief": "Only a brief."}, "storyboard": {"camera_flow": "From storyboard", "camera_motion_speed": 6, "character_motion_speed": 7}}, [good]),
        ({**base, "scenes": [{"scene_number": 1, "label": "Starter", "lyrics": "Hi", "story_beat": "Opening"}, 5, {"scene_number": 2}]}, [good]),
        ({**base, "minimax_h3_mode": "image-to-video"}, [good]),
        ({**base, "temperature": 0.3, "top_p": 0.6, "max_new_tokens": 3000}, [good]),
        ({**base, "scene_count": 0}, [good]),
        ({**base, "scene_count": "x"}, [good]),
        ({**base, "scene_count": 100}, [good]),
        ({**base, "scene_count": 1}, [good]),
        ({}, [good]),
        ({**base, "text_runner": "llm_api"}, [good]),
        ({**base}, ["not json at all", good]),
        ({**base, "text_runner": "llm_api"}, ["not json at all", good]),
        ({**base, "max_new_tokens": 900}, ["{broken", plan([scene(1, CUES)])]),
        ({**base}, ["not json at all", "[1, 2]"]),
        ({**base}, ["[1, 2]"]),
        ({**base}, ['"quoted"']),
        ({**base}, [json.dumps({"title": "x", "premise": "p"})]),
        ({**base}, [json.dumps({"title": "x", "premise": "p", "scenes": "nope"})]),
        ({**base}, [plan([])]),
        ({**base}, [plan([5, "x"])]),
        ({**base}, [plan([{"dialogue_cues": [{"dialogue": ""}]}])]),
        ({**base}, [json.dumps({"scenes": [scene(1, CUES)]})]),
        ({**base, "story_source": "A premise."}, [json.dumps({"scenes": [scene(1, CUES)]})]),
        ({**base}, [plan([scene(1, CUES)], title="T" * 300, premise="P" * 5000)]),
        ({**base, "script_import": SCRIPT}, [plan([scene(1, CUES, location_id="kitchen"), scene(2, CUES, location_id="porch")])]),
        ({**base, "scriptImport": SCRIPT, "scene_count": 2}, [plan([])]),
        ({**base, "storyboard": {"script_import": SCRIPT}}, [plan([scene(1, CUES, location_id="porch"), {"location_id": "kitchen"}, {}])]),
        ({**base, "script_import": {**SCRIPT, "raw_text": ""}, "camera_motion_speed": 7}, [plan([scene(n, CUES) for n in range(1, 4)])]),
        ({**base, "script_import": {**SCRIPT, "enabled": False}}, [good]),
        ({**base, "script_import": SCRIPT}, ["not json at all", plan([scene(1, CUES)])]),
        ({**base, "temperature": "hot"}, []),
        ({**base, "max_new_tokens": "many"}, []),
        ({**base}, []),
        ({**base}, [""]),
    ]
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["storyboard.dialogue_scenes"])
    dialogue = modules["storyboard.dialogue_scenes"]
    prompts = importlib.import_module("vrgdg_pack.llm.prompts.storyboard")
    OUT.mkdir(parents=True, exist_ok=True)

    cases = [{"fn": name, "args": kwargs, "return": getattr(prompts, name)(**kwargs)} for name, kwargs in prompt_cases()]
    (OUT / "dialogue_prompts.json").write_text(json.dumps({"cases": cases}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"dialogue_prompts.json: {len(cases)} cases")

    helpers = []
    for name, args_list in helper_cases():
        function = getattr(dialogue, name)
        try:
            result = {"return": function(*json.loads(json.dumps(args_list)))}
        except Exception as exc:  # noqa: BLE001
            result = {"error": str(exc)}
        helpers.append({"fn": name, "args": args_list, **result})
    (OUT / "dialogue_helpers.json").write_text(json.dumps({"cases": helpers}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"dialogue_helpers.json: {len(helpers)} cases, errors {sum(1 for c in helpers if 'error' in c)}")

    steps = []
    for request, replies in step_cases():
        payload = {**RUNNER, **request}
        outcome = h.run_case(recorder, dialogue._build_minimax_dialogue_scenes, payload, replies)
        steps.append({"fn": "dialogue", "request": request, "replies": replies, **outcome})
    (OUT / "dialogue_steps.json").write_text(json.dumps({"cases": steps}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"dialogue_steps.json: {len(steps)} cases, errors {sum(1 for c in steps if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
