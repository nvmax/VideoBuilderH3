"""Dev tool: golden cases for timed Timeline Notes (``h3-core::timeline_markers``), from the pack.

    python tools/make_markers_fixtures.py --pack <pack folder>

Writes ``fixtures/timeline/markers.json``. Generated ids are ``mark_1``, ``mark_2``... in the order they are asked for.
"""

import argparse
import copy
import importlib
import itertools
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

RAW = [
    [],
    None,
    "not a list",
    [{"id": "a", "start": 5, "end": 8, "type": "beat", "label": "Drop", "note": " big "}, {"start": 2}, {"id": "c", "start": -4, "end": 1}],
    [{"id": "p", "start": "3.5", "end": "3.5"}, {"id": "q", "start": None, "end": "x"}, 7, {"id": "r", "start": "inf"}, {"id": "s", "start": 2, "end": "nan"}],
    [{"id": 12, "start": 1, "label": "  ", "type": "", "note": None}, {"start": True}],
]

CREATES = [
    {"start": 1},
    {"start": 2, "end": 5, "type": "beat", "label": "Hit", "note": "snare"},
    {"id": "fixed", "start": 0},
    {"id": "fixed", "start": 3},
    {"end": 4},
    {"start": "x"},
    {"start": True},
    {"start": -1},
    {"start": 5, "end": 5},
    {"start": 5, "end": 4},
    {"start": 1, "end": ""},
    {"start": 1, "label": "x" * 501},
    {"start": 1, "type": ["list"]},
    {"start": 1, "note": 12.5},
    {"start": 1, "mystery": 1, "another": 2},
    {"start": 1, "label": None},
    {"start": 7, "label": "   "},
    "nope",
]

UPDATES = [
    ("a", {"label": "Renamed"}),
    ("a", {"start": 20}),
    ("a", {"start": 9}),
    ("a", {"end": None}),
    ("a", {"end": ""}),
    ("a", {"end": 4}),
    ("a", {"id": "a", "note": "same id is fine"}),
    ("a", {"id": "b"}),
    ("a", {"nope": 1}),
    ("zzz", {"label": "x"}),
    ("a", {"start": -2}),
    ("a", {"type": ""}),
    ("a", "nope"),
]

SEGMENTS = [
    {"id": "s1", "start": 0, "end": 4},
    {"id": "s2", "start": 4, "end": 8},
    {"id": "s3", "start": 8, "end": 12},
    {"id": "s4", "start": 12, "end": 12},
    {"id": "", "start": 12, "end": 14},
    "junk",
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "timeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    tm = importlib.import_module("vrgdg_pack.builder.timeline_markers")

    def fresh_ids():
        counter = itertools.count(1)
        tm.new_marker_id = lambda: f"mark_{next(counter)}"

    cases = []
    for raw in RAW:
        fresh_ids()
        cases.append({"op": "normalize", "raw": raw, "return": tm.normalize_markers(copy.deepcopy(raw))})
    base = [{"id": "a", "start": 5, "end": 8, "type": "beat", "label": "Drop", "note": ""}, {"id": "b", "start": 1, "end": None, "type": "note", "label": "Intro", "note": "x"}]
    for fields in CREATES:
        fresh_ids()
        markers = tm.normalize_markers(copy.deepcopy(base))
        try:
            note = tm.create_marker(markers, copy.deepcopy(fields))
            result = {"note": note, "markers": markers}
        except tm.TimelineMarkerError as exc:
            result = {"error": str(exc)}
        cases.append({"op": "create", "markers": base, "fields": fields, "return": result})
    for marker_id, fields in UPDATES:
        fresh_ids()
        markers = tm.normalize_markers(copy.deepcopy(base))
        try:
            note = tm.update_marker(markers, marker_id, copy.deepcopy(fields))
            result = {"note": note, "markers": markers}
        except tm.TimelineMarkerError as exc:
            result = {"error": str(exc)}
        except KeyError as exc:
            result = {"missing": str(exc.args[0])}
        cases.append({"op": "update", "markers": base, "id": marker_id, "fields": fields, "return": result})
    for marker_id in ("a", "b", "zzz"):
        markers = tm.normalize_markers(copy.deepcopy(base))
        try:
            note = tm.delete_marker(markers, marker_id)
            result = {"note": note, "markers": markers}
        except KeyError as exc:
            result = {"missing": str(exc.args[0])}
        cases.append({"op": "delete", "markers": base, "id": marker_id, "return": result})
    for marker in tm.normalize_markers(copy.deepcopy(base)) + [{"id": "p", "start": 4, "end": None}, {"id": "q", "start": 3.9, "end": 4.1}, {"id": "r", "start": 11, "end": 20}, {"id": "t", "start": 20, "end": None}, {"id": "u", "start": 0, "end": 100}]:
        cases.append({"op": "overlap", "marker": marker, "segments": SEGMENTS, "return": tm.overlapping_scene_ids(marker, copy.deepcopy(SEGMENTS))})
    (out / "markers.json").write_text(json.dumps(cases, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"markers.json: {len(cases)} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
