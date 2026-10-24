"""Dev tool: every backend route the Builder front end calls, the pack handler behind it and who calls it.

    python tools/inventory_builder_routes.py --pack <pack folder>

Writes ``fixtures/api/builder_routes.json``:
  {"routes": [{"method", "path", "handler", "file", "line", "callers": [module...], "doc": first docstring line}]}
Only routes that the front end (web/music_video_builder, web/storyboard_builder and the top level VRGDG_*.js it needs)
reaches are listed, plus the pack routes nothing calls (``callers`` empty) so nothing is lost.
"""

import argparse
import ast
import json
import re
import sys
from pathlib import Path

ROUTE_FILES = [
    "builder/routes.py", "builder/video_editor.py", "runner/routes.py", "core/system_routes.py", "minimax/refmod_library.py",
    "minimax/refmod_picker.py", "minimax/refmod_studio.py", "post_process/lut_video_tools.py", "post_process/face_fix.py",
    "prompt_creator/nodes.py", "storyboard/nodes.py", "browser/nodes.py",
]
DECORATOR = re.compile(r"@(?:\w+\.)*routes\.(get|post|put|delete|patch)\(\s*f?[\"'](/vrgdg/[^\"']+)[\"']")


def parse_routes(pack: Path):
    found = []
    for rel in ROUTE_FILES:
        path = pack / rel
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8")
        lines = text.splitlines()
        tree = ast.parse(text)
        funcs = {n.lineno: n for n in ast.walk(tree) if isinstance(n, (ast.AsyncFunctionDef, ast.FunctionDef))}
        for index, line in enumerate(lines, start=1):
            match = DECORATOR.search(line)
            if not match:
                continue
            handler, doc = "", ""
            for j in range(index, min(index + 8, len(lines)) + 1):
                if j in funcs:
                    handler = funcs[j].name
                    doc = (ast.get_docstring(funcs[j]) or "").splitlines()[0] if ast.get_docstring(funcs[j]) else ""
                    break
            found.append({"method": match.group(1).upper(), "path": match.group(2), "handler": handler, "file": rel, "line": index, "doc": doc})
    return found


# Routes registered from a constant (`_LOG_ROUTE + "/append"`), which the decorator pattern above cannot see.
EXTRA = [
    ("POST", "/vrgdg/music_builder/error_log", "error_log", "builder/error_log_routes.py", "Read or create the project's error log."),
    ("POST", "/vrgdg/music_builder/error_log/append", "append_error_log", "builder/error_log_routes.py", "Append a redacted browser error."),
    ("POST", "/vrgdg/music_builder/error_log/clear", "clear_error_log", "builder/error_log_routes.py", "Clear the project's error report."),
]


def frontend_calls(pack: Path):
    """path pattern -> modules that mention it."""
    calls = {}
    web = pack / "web"
    files = list((web / "music_video_builder").glob("*.mjs")) + list((web / "storyboard_builder").glob("*.mjs")) + list(web.glob("VRGDG_*.js"))
    pattern = re.compile(r"[\"'`](/vrgdg/[A-Za-z0-9_/\-\.]+)")
    for file in files:
        text = file.read_text(encoding="utf-8", errors="replace")
        for match in pattern.finditer(text):
            calls.setdefault(match.group(1).rstrip("/"), set()).add(file.name)
    return calls


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    routes = parse_routes(args.pack)
    routes += [{"method": m, "path": path, "handler": h, "file": f, "line": 0, "doc": d} for m, path, h, f, d in EXTRA]
    calls = frontend_calls(args.pack)
    for route in routes:
        route["callers"] = sorted(calls.get(route["path"].rstrip("/"), []))
        if not route["callers"] and "{" in route["path"]:
            prefix = route["path"].split("{")[0].rstrip("/")
            route["callers"] = sorted({m for p, ms in calls.items() if p.startswith(prefix) for m in ms})
    out = Path(__file__).resolve().parents[1] / "fixtures" / "api"
    out.mkdir(exist_ok=True)
    (out / "builder_routes.json").write_text(json.dumps({"routes": routes}, indent=1), encoding="utf-8")
    called = sum(1 for r in routes if r["callers"])
    print(f"builder_routes.json: {len(routes)} pack routes, {called} reached by the front end")
    unknown = sorted(p for p in calls if not any(r["path"].rstrip("/") == p for r in routes) and not any(r["path"].split("{")[0].rstrip("/") == p.split("{")[0].rstrip("/") for r in routes if "{" in r["path"]))
    print(f"front-end paths with no pack route found: {len(unknown)}")
    for p in unknown[:40]:
        print("  ", p, sorted(calls[p])[:2])
    return 0


if __name__ == "__main__":
    sys.exit(main())
