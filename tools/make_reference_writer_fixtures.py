"""Dev tool: golden cases for the Reference Builder's LLM writers, from the pack's own code.

    python tools/make_reference_writer_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/reference_writers.json``: for each case the request, the canned LLM replies, what the writer asked the LLM
(instruction text, sampling values, picture sizes) and what it returned or the error it raised. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
JUNK = "thought thought thought thought thought thought thought thought thought thought thought thought"

SCENES = [
    {"id": "s1", "label": "Scene 1", "concept": "She wakes in a small bedroom with a lamp", "notes": ""},
    {"id": "s2", "label": "Scene 2", "concept": "Walking through the foggy forest", "notes": "pine trees"},
    {"id": "s3", "concept": "Singing on an empty stage"},
    {"id": "s4", "label": "Scene 4", "concept": "", "notes": ""},
    "not a scene",
]
LOCATIONS = [
    {"name": "Bedroom", "description": "small bedroom with lamp"},
    {"name": "Foggy forest", "description": "pine trees in fog"},
    {"name": "Empty stage", "description": "stage with lights"},
]
LINES = "1|bedroom|small bedroom with a lamp and an unmade bed\n2|foggy forest|pine trees in thick fog, wet ground\n3|empty stage|bare stage with a single spotlight"
BULLETS = "Music Video Locations:\n\n- Abandoned motel pool, turquoise water under buzzing neon signs\n- Foggy pine road, wet asphalt and fading headlights\n- Empty theater stage, dusty boards under one spotlight"
JSON_LOCATIONS = json.dumps({"locations": [{"name": "dark wood hallway", "description": "dark wood hallway with a long runner"}, {"name": "Lace veil", "description": "a veil"}, {"name": "Rooftop", "description": "rooftop at dusk"}]})
PEOPLE = "1|young singer|a girl in a red coat with short black hair\n2|masked man|tall man in a black mask and grey suit"


def cases(picture, folder):
    describe = [
        ({"image_data": picture, "reference_type": "subject", "name": "Ann"}, ["Ann has long red hair, a black leather jacket with silver zippers, dark jeans and worn boots."]),
        ({"image_data": picture, "reference_type": "character", "name": "the singer"}, ["The character wears a blue coat, with pale skin, and has dark hair and a silver necklace."]),
        ({"image_data": picture, "reference_type": "face"}, ["a photo of a young woman with an oval face, brown eyes, thick eyebrows, a small nose, full lips and a soft jawline"]),
        ({"image_data": picture, "reference_type": "face"}, ["A young woman with an oval face, brown eyes, thick eyebrows, a small nose and full lips."]),
        ({"image_data": picture, "reference_type": "face"}, ["a photo of a woman a photo of a man with a wide jaw", "a photo of a woman with green eyes, a narrow nose, thin lips and a pointed chin"]),
        ({"image_data": picture, "reference_type": "face"}, ["a photo of a woman, let me think about it, with green eyes and thin lips", "a photo of a woman with green eyes and thin lips and a round face"]),
        ({"image_data": picture, "reference_type": "extra", "count": 12, "style": "western", "name": "Dancer"}, ["A man with short brown hair in a tan coat and boots with a thin beard."]),
        ({"image_data": picture, "reference_type": "extra", "count": "abc"}, ["A woman with a blonde bob in a green dress and flat shoes."]),
        ({"image_data": picture, "reference_type": "extra", "count": 250.5}, ["A woman with a blonde bob in a green dress and flat shoes."]),
        ({"image_data": picture, "reference_type": "location", "name": "Dock"}, ["Location: A weathered wooden dock over still water at dusk, grey planks, rope coils, low orange light and drifting mist."]),
        ({"image_data": picture, "reference_type": "prop", "name": "Silver key"}, ["An ornate silver key with a looped bow, fine engraving along the shaft and a worn brass tag."]),
        ({"image_data": picture, "reference_type": "Vehicle"}, ["A matte black van with a dented side door, rusted wheel arches and a cracked roof light."]),
        ({"image_data": picture, "reference_type": "weird", "name": "X"}, ["A tall woman with short silver hair and a long grey coat over black boots."]),
        ({"image_data": picture, "reference_type": "subject", "lmstudio_model": "other-model", "temperature": 0.7, "top_p": 0.5, "max_new_tokens": 300}, ["A woman with a braided crown and a green cloak fastened with a brass pin."]),
        ({"image_data": picture, "reference_type": "subject"}, [JUNK, "A woman with a braided crown and a green cloak fastened with a brass pin."]),
        ({"image_data": picture, "reference_type": "subject"}, [JUNK, JUNK]),
        ({"image_data": picture, "reference_type": "subject"}, ["", "A woman with a braided crown and a green cloak fastened with a brass pin."]),
        ({"reference_type": "subject"}, []),
        ({"image_data": picture, "reference_type": "subject", "text_runner": "builtin"}, []),
    ]
    maps = [
        ({"scenes": SCENES, "existing_locations": LOCATIONS}, ["Scene1=1\nScene2=2\nScene3=3"]),
        ({"scenes": SCENES, "existing_locations": LOCATIONS}, ["Scene1=1\nScene2=1\nScene3=1"]),
        ({"scenes": SCENES, "existing_locations": LOCATIONS}, ["I think the first one fits."]),
        ({"scenes": SCENES, "existing_locations": LOCATIONS}, ["Scene1=Bedroom\nScene 2 = 2\ns3: 3,"]),
        ({"scenes": SCENES, "existing_locations": LOCATIONS}, ["Scene2=2"]),
        ({"scenes": SCENES, "existing_locations": LOCATIONS, "used_location_counts": {"bedroom": 3, "Empty stage": "x", "Nowhere": 2}, "previous_assignments": ["Foggy forest", {"location": "Bedroom"}, {"location_name": "empty stage"}, 5]}, ["Scene1=1\nScene2=1\nScene3=1"]),
        ({"scenes": SCENES, "existing_locations": LOCATIONS + [{"name": "BEDROOM", "description": "dup"}, {"name": ""}, 7], "subject_scene_text": "A lonely singer.", "temperature": 0.5, "max_new_tokens": 100}, ["Scene1=3\nScene2=2\nScene3=1"]),
        ({"scenes": SCENES[:1], "existing_locations": LOCATIONS[:1]}, ["Scene1=1"]),
        ({"scenes": [], "existing_locations": LOCATIONS}, []),
        ({"scenes": "x", "existing_locations": LOCATIONS}, []),
        ({"scenes": [SCENES[3]], "existing_locations": LOCATIONS}, []),
        ({"scenes": SCENES, "existing_locations": []}, []),
        ({"scenes": SCENES, "existing_locations": LOCATIONS, "text_runner": "builtin"}, []),
    ]
    locs = [
        ({"scenes": SCENES}, [LINES]),
        ({"scenes": SCENES, "style_theme": "Noir and rain.", "subject_context": "The singer: Ann.", "subject_scene_text": "A lonely night.", "existing_locations": LOCATIONS, "max_locations": 2}, [LINES]),
        ({"scenes": SCENES}, [BULLETS]),
        ({"scenes": SCENES}, [JSON_LOCATIONS]),
        ({"scenes": SCENES}, ["Sorry, I cannot help with that.", LINES]),
        ({"scenes": SCENES}, ["nothing at all", "1) still nothing", BULLETS]),
        ({"scenes": SCENES, "temperature": 0.3}, ["nothing at all", "none either", BULLETS]),
        ({"scenes": SCENES}, ["nothing at all", "none either", "nope", "nada"]),
        ({"scenes": SCENES}, ["nothing at all", "none either", "nope"]),
        ({"scenes": SCENES}, ["1|razor|a shaving razor"]),
        ({"scenes": SCENES, "temperature": 0.9, "top_p": 0.5, "max_new_tokens": 500, "max_locations": "x"}, []),
        ({"scenes": SCENES, "max_locations": 99}, [LINES]),
        ({"scenes": [SCENES[3]]}, []),
        ({"scenes": []}, []),
        ({"scenes": SCENES, "text_runner": "builtin"}, []),
    ]
    wizard = [
        ({"lyrics_text": "I walk the empty halls at night\nThe rain keeps falling on the glass"}, [BULLETS]),
        ({"lyrics": "Dancing in the neon light", "user_input": "dark mood", "style_theme": "Noir.", "subject_context": "The singer: Ann.", "existing_locations": LOCATIONS, "max_locations": 2}, [BULLETS]),
        ({"lyrics_text": "Dancing in the neon light"}, [JSON_LOCATIONS]),
        ({"lyrics_text": "Dancing in the neon light"}, ["", BULLETS]),
        ({"lyrics_text": "Dancing in the neon light"}, ["Sorry, I cannot do that.", "nothing here"]),
        ({"lyrics_text": "Dancing in the neon light"}, ["1|bedroom|small bedroom with a lamp", "still nothing"]),
        ({"lyrics_text": "Dancing in the neon light"}, ["- Lace veil\n- Silver frame"]),
        ({"lyrics_text": ""}, []),
        ({"lyrics_text": "x", "max_locations": "x"}, []),
        ({"lyrics_text": "Dancing", "text_runner": "builtin"}, []),
    ]
    scout_reply = json.dumps({"locations": [{"name": "Motel pool", "description": "turquoise water, neon"}, {"name": "motel pool", "description": "dup"}, {"name": "Pine road", "description": "wet asphalt"}, "x"]})
    scout = [
        ({"lyrics_text": "Neon and rain", "style_theme": "Noir", "subject_context": "Ann", "existing_locations": LOCATIONS}, [scout_reply]),
        ({"lyrics_text": "", "scenes": SCENES, "temperature": 0.3, "top_p": 0.4, "max_new_tokens": 600}, [scout_reply]),
        ({"lyrics_text": "Neon and rain"}, ["```json\n{locations: [{name: 'A', description: 'B',}],}\n```"]),
        ({"lyrics_text": "Neon and rain"}, ["Here are some places."]),
        ({"lyrics_text": "", "scenes": []}, []),
        ({"lyrics_text": "x", "text_runner": "builtin"}, []),
    ]
    subjects = [
        ({"scenes": SCENES}, [PEOPLE]),
        ({"scenes": SCENES, "requested_count": 5, "subject_scene_text": "A band.", "existing_subjects": [{"name": "Ann", "description": "a singer"}, {"name": "x"}, 3], "temperature": 0.4}, [PEOPLE + "\n1|Young Singer|duplicate"]),
        ({"scenes": SCENES, "project_folder": folder}, ["- masked man: tall, black mask\nno colon line"]),
        ({"scenes": [{"id": "a", "concept": "", "notes": ""}, {"id": "b", "concept": "", "notes": ""}], "project_folder": folder}, [PEOPLE]),
        ({"scenes": [{"id": "a", "concept": "", "notes": ""}, {"id": "b", "concept": "", "notes": ""}, {"id": "c", "concept": "", "notes": ""}], "project_folder": folder + "/missing"}, []),
        ({"scenes": SCENES}, ["no subjects here"]),
        ({"scenes": SCENES, "requested_count": 99}, [PEOPLE]),
        ({"scenes": []}, []),
        ({"scenes": SCENES, "text_runner": "builtin"}, []),
    ]
    zimage = [
        ({"reference_type": "subject", "source_text": "A tall woman with silver hair."}, ["A clean three-panel character reference sheet on a neutral studio background, showing the same tall woman with silver hair in all panels with consistent face and outfit."]),
        ({"reference_type": "location", "source_text": "A foggy dock at dusk.", "style_theme": "Noir."}, ["A clear cinematic environment reference image of a foggy wooden dock at dusk, grey planks, rope coils, low orange light, drifting mist, no people, no text."]),
        ({"reference_type": "Subject", "source_text": "A tall woman.", "temperature": 0.9, "max_new_tokens": 500}, ['{"prompt": "x"}']),
        ({"reference_type": "subject", "source_text": "A tall woman."}, [JUNK]),
        ({"reference_type": "subject", "source_text": "A tall woman."}, [""]),
        ({"reference_type": "prop", "source_text": "A key."}, []),
        ({"reference_type": "subject", "source_text": ""}, []),
        ({"reference_type": "subject", "source_text": "x", "text_runner": "builtin"}, []),
    ]
    return [("describe", c) for c in describe] + [("map", c) for c in maps] + [("locs", c) for c in locs] + [("wizard", c) for c in wizard] + [("scout", c) for c in scout] + [("subjects", c) for c in subjects] + [("zimage", c) for c in zimage]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.image_prompt_generation"])
    ipg = modules["llm.image_prompt_generation"]
    functions = {
        "describe": ipg._generate_builder_reference_description,
        "map": ipg._generate_flux_reference_location_map,
        "locs": ipg._generate_flux_reference_locations,
        "wizard": ipg._generate_wizard_locations_from_lyrics,
        "scout": ipg._generate_lm_scout_locations,
        "subjects": ipg._generate_flux_reference_subjects,
        "zimage": ipg._generate_flux_reference_zimage_prompt,
    }
    folder, _ = h.temp_files({"SceneNotes.json": json.dumps({"SceneNote1": " rain on the window ", "SceneNote3": "empty street", "Other": "x"})})
    picture = h.png_data_url(40, 30)
    out = []
    with folder:
        for name, (request, replies) in cases(picture, str(Path(folder.name).resolve())):
            payload = {**RUNNER, **request}
            outcome = h.run_case(recorder, functions[name], payload, replies)
            out.append({"fn": name, "request": h.scrub(request, folder.name), "replies": replies, **h.scrub(outcome, folder.name)})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "reference_writers.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"reference_writers.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
