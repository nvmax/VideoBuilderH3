"""Dev tool: golden data for the location scout, from the pack.

    python tools/make_location_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/locations.json`` and ``crates/h3-llm/data/location_scout.txt``. The LLM call is replaced by a
function that records the instruction and returns a canned reply, so the prompt and the reply handling are both checked.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

REPLIES = [
    '{"locations": [{"name": "Corner Bodega", "description": "Red awnings glow under amber lights."}, {"name": "Rooftop Garden", "description": "Potted trees ring a tar roof."}]}',
    '```json\n{"locations": [{"name": " Neon Diner, ", "description": "  Chrome   stools\nline a counter. "}]}\n```',
    'Here you go: {"locations": [{"name": "Dock", "description": "Wet planks."},]}',
    "{locations: [{name: 'x', description: \"y\"}]}",
    '{"locations": [{"name": "Dock", "description": "a"}, {"name": "dock", "description": "b"}, {"name": "", "description": "c"}, "junk", {"name": "Pier.-", "description": null}]}',
    '{"locations": []}',
    "no json at all",
    '[{"name": "A", "description": "b"}]',
    '{"locations": [{"name": "A", "description": "b"}\n{"name": "B", "description": "c"}]}',
    "<think>hmm</think>{\"locations\": [{\"name\": \"Cave\", \"description\": \"Dark.\"}]}",
    '{"locations": "nope"}',
]

INPUTS = [
    {"lyrics_text": "walking down the street\nlights fading", "scenes": [], "style_theme": "", "subject_context": "", "existing_locations": []},
    {
        "lyrics_text": "", "scenes": [{"concept": "a rooftop at dusk", "notes": "wind"}, {"concept": "", "notes": ""}, {"concept": "an alley", "notes": ""}],
        "style_theme": "Global theme/style:\nNeon noir\n\nLocation extraction notes:\nNo warehouses", "subject_context": "Ava (character): dark hair",
        "existing_locations": [{"name": "Roof", "description": "x"}, {"name": "  ", "description": ""}, {"name": "Club   Floor"}],
    },
    {"lyrics_text": "x", "scenes": [{"concept": "c", "notes": "n"}], "style_theme": "", "subject_context": "", "existing_locations": []},
    {"lyrics_text": "", "scenes": [], "style_theme": "", "subject_context": "", "existing_locations": []},
]

CONTEXT_TEXTS = [
    "", "plain theme", "line one\n\nline two", "see C:\\music\\themestyle.txt for more", "as in subjectsandscenes.txt only", "I can't do that",
    "Here is the reusable location list:\nRoof\nClub", "Global theme/style:\nNeon\n\nLocation extraction notes:\nNo warehouses", "x" * 150, "x " * 80 + "room",
    "output format: json", "keep it\r\nwindows lines", "the user input says go",
]

REFS = [
    {}, {"subjects": []}, {"subjects": [{"name": "Ava", "description": "dark hair"}, {"name": "", "description": ""}, {"name": "Ben", "reference_type": "extra"}, {"description": "unnamed"}]},
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    rs = Path(__file__).resolve().parents[1]
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    ig = importlib.import_module("vrgdg_pack.llm.image_prompt_generation")
    oc = importlib.import_module("vrgdg_pack.llm.output_checks")
    scout = importlib.import_module("vrgdg_pack.llm.prompts.location_scout")
    ro = importlib.import_module("vrgdg_pack.agent_api.orchestrator.reference_orchestrator")

    (rs / "crates" / "h3-llm" / "data" / "location_scout.txt").write_text(scout._LM_LOCATION_SCOUT_INSTRUCTIONS, encoding="utf-8", newline="")

    captured = {}

    def fake(payload, instruction, **kwargs):
        captured["instruction"] = instruction
        captured["schema"] = kwargs.get("json_schema")
        return captured["reply"], {"used_model": "m", "runner": "own server", "unloaded": False}

    ig._run_builder_text_llm = fake
    scout_cases = []
    for inputs in INPUTS:
        for reply in REPLIES:
            captured["reply"] = reply
            payload = {"model_file": "x", **json.loads(json.dumps(inputs))}
            try:
                result = {"locations": ig._generate_lm_scout_locations(payload)["locations"]}
            except Exception as exc:  # noqa: BLE001
                result = {"error": str(exc)}
            scout_cases.append({"input": inputs, "reply": reply, "instruction": captured.get("instruction"), "return": result})
            captured.pop("instruction", None)
    schema = scout._LM_LOCATION_SCOUT_SCHEMA

    repairs, extracts = [], []
    for reply in REPLIES + ['{"a": 1,}', "{'a': 1}", '{"scene_map": [1] "x": 2}', "// c\n{\"a\": 1}", "{Scene 1: \"x\"}", "", "{}"]:
        repairs.append({"input": reply, "return": oc._repair_builder_json_like_text(reply)})
        try:
            extracts.append({"input": reply, "return": {"value": oc._extract_json_object_from_text(reply)}})
        except Exception as exc:  # noqa: BLE001
            extracts.append({"input": reply, "return": {"error": True, "message": str(exc)}})
    contexts = [{"input": t, "return": oc._clean_location_context_text(t), "meta": oc._looks_like_location_meta_text(t)} for t in CONTEXT_TEXTS]
    subjects = [{"refs": r, "return": ro.subject_context_for_locations(r)} for r in REFS]

    out = {"scout": scout_cases, "schema": schema, "repair": repairs, "extract": extracts, "context": contexts, "subject_context": subjects}
    (rs / "fixtures" / "llm" / "locations.json").write_text(json.dumps(out, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"locations.json: {len(scout_cases)} scout cases, {len(repairs)} repairs, {len(contexts)} contexts")
    return 0


if __name__ == "__main__":
    sys.exit(main())
