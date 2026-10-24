"""Dev tool: golden cases for the motion note writer (`_generate_builder_motion_notes`), from the pack's own code with the LLM replaced by a recorder.

    python tools/make_motion_notes_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/motion_notes.json``. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
GOOD = json.dumps({"Motion1": "Slow push in as she turns toward the window while rain streaks the glass.", "Motion2": "Camera drifts over the empty street, neon flickering in puddles."})
LONG = "word " * 100

SCENES = [
    {"scene_number": 1, "label": "Verse", "concept_prompt": "A woman at a window. Location: apartment.", "director_note": "Quiet and tense.", "timeline_notes": [{"label": "rain", "note": "rain starts", "start": 0, "end": 2.5}, {"type": "cut", "note": "hard cut", "start": 2.5, "end": 3}]},
    {"scene_number": 2, "concept_prompt": "Empty street at night. Location: street."},
]


def cases():
    return [
        ({"scenes": SCENES}, [GOOD]),
        ({"scenes": SCENES, "story_idea": "A long night.", "theme_style": "Noir.", "previous_summary": "Motion1: slow push"}, [GOOD]),
        ({"scenes": SCENES, "source_mode": "concept_director"}, [GOOD]),
        ({"scenes": SCENES, "source_mode": "concept_timeline", "video_mode": "t2v"}, [GOOD]),
        ({"scenes": SCENES, "source_mode": "ALL", "video_mode": " I2V "}, [GOOD]),
        ({"scenes": SCENES, "source_mode": "unknown"}, [GOOD]),
        ({"scenes": SCENES, "source_mode": "   "}, [GOOD]),
        ({"scenes": SCENES, "temperature": 0.8, "top_p": 0.5, "max_new_tokens": 700}, [GOOD]),
        ({"scenes": SCENES}, ["```json\n" + GOOD + "\n```"]),
        ({"scenes": SCENES}, ["Answer: " + GOOD]),
        ({"scenes": SCENES}, ['{"scene 1": "Camera pans left.", "Scene2": "Handheld drift.", "other": "ignored", "Motion3": "  "}']),
        ({"scenes": SCENES}, ['{Motion1: "Camera pans left.", Motion2: "Handheld drift.",}']),
        ({"scenes": SCENES}, ['Motion1: "Plain one" Motion 2: "Plain two" scene 03: "Plain three"']),
        ({"scenes": SCENES}, ['{"Motion1": "A", "Motion01": "B", "Motion10": "C", "Motion2": 5}']),
        ({"scenes": SCENES}, ['["not", "an", "object"]']),
        ({"scenes": SCENES}, ["nothing useful"]),
        ({"scenes": SCENES}, [""]),
        ({"scenes": []}, []),
        ({}, []),
        ({"scenes": "x"}, []),
        ({"scenes": [5, "x", None]}, []),
        ({"scenes": [{"scene_number": "x", "label": "  ", "concept_prompt": LONG * 40, "director_note": LONG * 30}], "source_mode": "all"}, ['{"Motion1": "ok"}']),
        ({"scenes": [{"number": 0}, {"scene_number": -3, "timeline_notes": "x"}, {"scene_number": 2.7, "timeline_notes": [5, {"note": LONG * 20}, {"label": "", "type": "beat", "start": "a", "end": None}]}]}, [GOOD]),
        ({"scenes": [{"scene_number": i, "label": f"S{i}", "concept_prompt": f"Concept {i}"} for i in range(1, 26)]}, [GOOD]),
        ({"scenes": [{"scene_number": 1, "timeline_notes": [{"label": f"n{i}", "note": f"note {i}"} for i in range(12)]}]}, [GOOD]),
        ({"scenes": SCENES}, [json.dumps({f"Motion{i}": LONG for i in range(1, 12)})]),
        ({"scenes": SCENES, "temperature": "x"}, []),
        ({"scenes": [{"scene_number": 1, "label": "Café été", "concept_prompt": "Une scène — “quote”"}]}, [GOOD]),
    ]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.video_prompt_generation"])
    function = modules["llm.video_prompt_generation"]._generate_builder_motion_notes
    out = []
    for request, replies in cases():
        outcome = h.run_case(recorder, function, {**RUNNER, **request}, replies)
        out.append({"fn": "motion", "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "motion_notes.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"motion_notes.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
