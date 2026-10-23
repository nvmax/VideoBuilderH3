"""Dev tool: record the shape of real Video Builder sessions without copying any of their content.

    python tools/session_shape.py --root <ComfyUI output folder>

Scans ``<root>/*/vrgdg_builder_session.json`` and writes ``fixtures/session_shape.json``: every key path with the value
types seen and how many sessions or list items had it. Lyrics, prompts, names and paths are never stored, only key
names, types and counts, so the file is safe to commit. P3 uses it to build the session model that keeps unknown keys.
"""

import argparse
import json
import re
from collections import defaultdict
from pathlib import Path


def type_name(value) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int):
        return "int"
    if isinstance(value, float):
        return "float"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    return "object"


ID_KEY = re.compile(r"^(?:[A-Za-z]+[_-])?(?:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{8,})$|^[0-9]+$")


def walk(value, path, seen):
    """Record ``path`` -> type counts. Arrays use ``[]``. A dict keyed mostly by ids (uuids, numbers) uses ``{}``."""
    seen[path][type_name(value)] += 1
    if isinstance(value, dict):
        keys = list(value)
        id_map = len(keys) >= 3 and sum(1 for k in keys if ID_KEY.match(k)) >= 0.6 * len(keys)
        for key, child in value.items():
            walk(child, f"{path}.{'{}' if id_map else key}", seen)
    elif isinstance(value, list):
        for child in value:
            walk(child, f"{path}[]", seen)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "session_shape.json")
    args = parser.parse_args()

    seen = defaultdict(lambda: defaultdict(int))
    sessions = 0
    for path in sorted(args.root.glob("*/vrgdg_builder_session.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict):
            continue
        sessions += 1
        walk(data, "$", seen)
    shape = {
        "sessions_scanned": sessions,
        "paths": {p: dict(sorted(t.items())) for p, t in sorted(seen.items())},
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(shape, indent=1, ensure_ascii=False), encoding="utf-8")
    top = sorted({p.split(".")[1].split("[")[0] for p in shape["paths"] if p.count(".") >= 1})
    print(f"{sessions} sessions, {len(shape['paths'])} key paths, {len(top)} top-level keys -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
