"""Dev tool: golden RefMod rewires (``h3-graph::refmod``), from the pack.

    python tools/make_refmod_graph_fixtures.py --pack <pack folder>

The pack's ``apply_refmod_pipeline`` runs on graphs that are already golden fixtures, with a stand-in RefMod library.
Writes ``fixtures/timeline/refmod_graphs.json``.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

LIBRARY = {
    "identity/brad": {"name": "identity/brad", "kind": "video", "tokens": 5632},
    "identity/darrel": {"name": "identity/darrel", "kind": "video", "tokens": 2394},
    "background/meadow": {"name": "background/meadow", "kind": "image", "tokens": 600},
    "style/film": {"name": "style/film", "kind": "image", "tokens": 300},
}

GRAPHS = ["r2v_single_scene1", "r2v_two_pass_scene1", "r2v_single_built_in_audio", "r2v_two_pass_built_in_audio", "r2v_single_masked_ctx90_scene3", "r2v_two_pass_lora", "r2v_single_use_te_speed"]

BASE_REFS = [{"name": "identity/brad", "strength": 1.0}, {"name": "identity/darrel", "strength": 0.8}, {"name": "background/meadow", "strength": 1.0}]


def payloads():
    text = "Brad <Video 1> and Darrel <Video 2> in <Picture 1> <Audio 1>"
    yield "base", {"pipeline": "refmod", "prompt": text, "refmod_references": BASE_REFS}
    yield "built_in", {"pipeline": "refmod", "audio_mode": "built_in_audio", "prompt": "Brad <Video 1> Darrel <Video 2> <Picture 1>", "refmod_references": BASE_REFS}
    yield "zero_strength", {"pipeline": "refmod", "prompt": text, "refmod_references": [BASE_REFS[0], dict(BASE_REFS[1], strength=0), BASE_REFS[2]]}
    yield "relabel", {"pipeline": "refmod", "prompt": "Brad <Picture 1> Darrel <Video 1> <Video 2>", "refmod_references": [
        {"name": "identity/brad", "strength": 1.0, "label": "<Picture 1>"}, {"name": "identity/darrel", "strength": 1.0, "label": "<Video 1>"}, {"name": "background/meadow", "strength": 1.0, "label": "<Video 2>"}]}
    yield "swapped_labels", {"pipeline": "refmod", "prompt": "A <Video 1> B <Picture 1>", "refmod_references": [
        {"name": "background/meadow", "strength": 1.0, "label": "<Video 1>"}, {"name": "identity/brad", "strength": 1.0, "label": "<Picture 1>"}]}
    yield "names_and_categories", {"pipeline": "refmod", "prompt": "x", "refmod_references": [
        {"name": "identity/brad", "strength": "0.5", "display_name": "Brad", "category": "character"}, {"name": "style/film", "display_name": "Film", "category": "style"}]}
    yield "eight_mods", {"pipeline": "refmod", "prompt": text, "refmod_references": [BASE_REFS[0]] * 8}
    yield "nine_mods", {"pipeline": "refmod", "prompt": text, "refmod_references": [BASE_REFS[0]] * 9}
    yield "twenty_five_mods", {"pipeline": "refmod", "prompt": text, "refmod_references": [BASE_REFS[0]] * 25}
    yield "unbalanced", {"pipeline": "refmod", "prompt": text, "refmod_references": [{"name": "identity/brad", "strength": 1.0}, {"name": "identity/darrel", "strength": 0.3}]}
    yield "no_references", {"pipeline": "refmod", "prompt": text, "refmod_references": []}
    yield "missing_key", {"pipeline": "refmod", "prompt": text}
    yield "unknown_mod", {"pipeline": "refmod", "prompt": text, "refmod_references": [{"name": "identity/nobody", "strength": 1.0}]}
    yield "blank_name", {"pipeline": "refmod", "prompt": text, "refmod_references": [{"strength": 1.0}]}
    yield "bad_strength", {"pipeline": "refmod", "prompt": text, "refmod_references": [{"name": "identity/brad", "strength": "loud"}]}
    yield "strength_too_high", {"pipeline": "refmod", "prompt": text, "refmod_references": [{"name": "identity/brad", "strength": 1.5}]}
    yield "not_an_object", {"pipeline": "refmod", "prompt": text, "refmod_references": ["identity/brad"]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    rs = Path(__file__).resolve().parents[1]
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    rm = importlib.import_module("vrgdg_pack.runner.minimax_refmod")
    rm.find_refmod = lambda name: LIBRARY.get(name)

    cases = []
    for graph_name in GRAPHS:
        record = json.loads((rs / "fixtures" / "graphs" / f"{graph_name}.json").read_text(encoding="utf-8"))
        built = record["result"]["prompt"]
        for label, payload in payloads():
            prompt = copy.deepcopy(built)
            entry = {"graph": graph_name, "payload_name": label, "payload": payload}
            try:
                summary = rm.apply_refmod_pipeline(prompt, copy.deepcopy(payload))
                entry["return"] = {"prompt": prompt, "summary": summary}
            except Exception as exc:  # noqa: BLE001
                entry["return"] = {"error": str(exc), "type": type(exc).__name__}
            cases.append(entry)
    (rs / "fixtures" / "timeline" / "refmod_graphs.json").write_text(json.dumps({"library": LIBRARY, "cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    errors = sum(1 for c in cases if "error" in c["return"])
    print(f"refmod_graphs.json: {len(cases)} cases, {errors} errors")
    return 0


if __name__ == "__main__":
    sys.exit(main())
