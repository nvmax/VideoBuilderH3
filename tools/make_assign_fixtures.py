"""Dev tool: golden tables for assigning characters and locations to scenes, from the pack.

    python tools/make_assign_fixtures.py --pack <pack folder>

Writes ``fixtures/timeline/assign.json``. The random pattern is not recorded, because Python's generator is not ported.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402


def session(scenes: int, subjects: list, locations: list, maps: dict, flags: dict) -> dict:
    segments = [{"id": f"scene_{i + 1:03d}", "label": f"Scene {i + 1}", **flags.get(i, {})} for i in range(scenes)]
    return {
        "segments": segments,
        "flux_reference_builder": {
            "subjects": subjects,
            "locations": locations,
            "subject_scene_map": maps.get("subjects", {}),
            "scene_map": maps.get("locations", {}),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "timeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    ro = importlib.import_module("vrgdg_pack.agent_api.orchestrator.reference_orchestrator")

    people = [{"id": "s1", "name": "Ava"}, {"id": "s2", "name": "Ben"}, {"id": "s3", "name": "Cy"}, {"id": "s1b", "name": "Ava side", "extra_reference_for": "s1"}]
    places = [{"id": "l1", "name": "Roof"}, {"id": "l2", "name": "Street"}, {"id": "l3", "name": "Club"}]
    existing = {"subjects": {"scene_002": ["s3"], "scene_004": "s2,s1"}, "locations": {"scene_003": "l2"}}
    flags = {1: {"no_character_present": True}}

    cases = []
    patterns = ("rotate", "blocks", "unchanged")
    for scenes in (1, 5, 9):
        for cp in patterns:
            for lp in patterns:
                for replace in (False, True):
                    for scope, extra in (("all", {}), ("range", {"range_start": 2, "range_end": 4}), ("selected", {"scene_ids": ["scene_001", "3"]})):
                        params = {"scope": scope, "character_pattern": cp, "location_pattern": lp, "character_block_size": 2, "location_block_size": 3, "replace_existing": replace, **extra}
                        sess = session(scenes, people, places, existing, flags)
                        try:
                            result = {"plan": ro.plan_scene_assignment(sess, **params)}
                        except Exception as exc:  # noqa: BLE001
                            result = {"error": str(exc)}
                        cases.append({"scenes": scenes, "params": params, "session": sess, "return": result})
    # nothing to choose from, and bad input
    cases.append({"scenes": 3, "params": {"character_pattern": "rotate", "location_pattern": "rotate"}, "session": session(3, [], [], {}, {}), "return": {"plan": ro.plan_scene_assignment(session(3, [], [], {}, {}), character_pattern="rotate", location_pattern="rotate")}})
    for bad in ({"character_pattern": "shuffle"}, {"scope": "selected"}):
        sess = session(3, people, places, {}, {})
        try:
            result = {"plan": ro.plan_scene_assignment(sess, **bad)}
        except Exception as exc:  # noqa: BLE001
            result = {"error": str(exc)}
        cases.append({"scenes": 3, "params": bad, "session": sess, "return": result})

    (out / "assign.json").write_text(json.dumps(cases, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"assign.json: {len(cases)} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
