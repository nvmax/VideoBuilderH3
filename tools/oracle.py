"""Dev tool: send the same read-only request to the live pack (ComfyUI) and to this app, and compare the replies.

    python tools/oracle.py [--pack http://127.0.0.1:8188] [--app http://127.0.0.1:8190] [--only substring]

Only routes that do not change anything are listed, so it is safe to point at real projects. A reply is compared as JSON.
Values of private fields (a session's text) are never printed, only the path of the key that differs.
A case marked ``shape`` compares keys and value types only, for readings that change every call.
"""

import argparse
import collections
import json
import re
import sys
import urllib.error
import urllib.request

PRIVATE_ROOTS = ("session",)


def call(base, method, path, body=None):
    data = None if body is None else json.dumps(body).encode("utf-8")
    request = urllib.request.Request(base + path, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return response.status, json.loads(response.read().decode("utf-8") or "null")
    except urllib.error.HTTPError as error:
        text = error.read().decode("utf-8", "replace")
        try:
            return error.code, json.loads(text)
        except ValueError:
            return error.code, {"_raw": text[:200]}
    except Exception as error:  # connection problems
        return 0, {"_error": str(error)[:200]}


def shape(value):
    if isinstance(value, dict):
        return {key: shape(item) for key, item in value.items()}
    if isinstance(value, list):
        return [shape(value[0])] if value else []
    return type(value).__name__ if not isinstance(value, bool) else "bool"


def diff(a, b, path="", out=None, private=False):
    out = [] if out is None else out
    if type(a) is not type(b) and not (isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool) and not isinstance(b, bool)):
        out.append(f"{path or '.'}: type {type(a).__name__} (pack) vs {type(b).__name__} (app)")
    elif isinstance(a, dict):
        for key in a.keys() | b.keys():
            if key not in a:
                out.append(f"{path}.{key}: only in app")
            elif key not in b:
                out.append(f"{path}.{key}: only in pack")
            else:
                diff(a[key], b[key], f"{path}.{key}", out, private or (not path and key in PRIVATE_ROOTS))
    elif isinstance(a, list):
        if len(a) != len(b):
            out.append(f"{path}: {len(a)} items (pack) vs {len(b)} (app)")
        for index, (x, y) in enumerate(zip(a, b)):
            diff(x, y, f"{path}[{index}]", out, private)
    elif a != b and not (isinstance(a, (int, float)) and isinstance(b, (int, float)) and abs(a - b) < 1e-6):
        shown = "(private)" if private else f"{str(a)[:60]!r} (pack) vs {str(b)[:60]!r} (app)"
        out.append(f"{path}: {shown}")
    return out


def project_folders(pack):
    status, listing = call(pack, "GET", "/vrgdg/music_builder/list_projects")
    return [p["project_folder"] for p in listing.get("projects", [])] if status == 200 else []


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", default="http://127.0.0.1:8188")
    parser.add_argument("--app", default="http://127.0.0.1:8190")
    parser.add_argument("--only", default="")
    args = parser.parse_args()

    cases = [
        ("GET", "/vrgdg/music_builder/model_defaults", None, ""),
        ("GET", "/vrgdg/music_builder/list_projects", None, ""),
        ("POST", "/vrgdg/music_builder/list_video_profiles", {}, ""),
        ("POST", "/vrgdg/music_builder/list_ui_profiles", {}, ""),
        ("POST", "/vrgdg/music_builder/load_default_ui_layout", {}, ""),
        ("POST", "/vrgdg/music_builder/list_scene_defaults_presets", {}, ""),
        ("GET", "/vrgdg/music_builder/post_process/adjust/presets", None, ""),
        ("GET", "/vrgdg/workflow_runner/model_root", None, ""),
        ("GET", "/vrgdg/workflow_runner/i2v_choices", None, ""),
        ("GET", "/vrgdg/workflow_runner/lora_list", None, ""),
        ("GET", "/vrgdg/music_builder/gemma_choices", None, ""),
        ("GET", "/vrgdg/resource-monitor", None, "shape"),
        ("GET", "/vrgdg/video_builder/custom_nodes/status", None, ""),
    ]
    for index, folder in enumerate(project_folders(args.pack), start=1):
        cases.append(("POST", "/vrgdg/music_builder/load_session", {"project_folder": folder}, f"project {index}"))

    failed = 0
    for method, path, body, note in cases:
        label = f"{method} {path.replace('/vrgdg/', '')} {note}".strip()
        if args.only and args.only not in label:
            continue
        pack_status, pack_reply = call(args.pack, method, path, body)
        app_status, app_reply = call(args.app, method, path, body)
        if note == "shape":
            pack_reply, app_reply = shape(pack_reply), shape(app_reply)
        problems = [] if pack_status == app_status else [f"status {pack_status} (pack) vs {app_status} (app)"]
        problems += diff(pack_reply, app_reply)
        if problems:
            failed += 1
            print(f"DIFF  {label}")
            # Differences that only differ by list index are one finding.
            grouped = collections.OrderedDict()
            for line in problems:
                grouped.setdefault(re.sub(r"\[\d+\]", "[]", line.split(": ", 1)[0]) + ": " + (line.split(": ", 1)[1] if ": " in line else ""), 0)
                grouped[re.sub(r"\[\d+\]", "[]", line.split(": ", 1)[0]) + ": " + (line.split(": ", 1)[1] if ": " in line else "")] += 1
            for line, count in list(grouped.items())[:12]:
                print(f"        {line}" + (f"  (x{count})" if count > 1 else ""))
            if len(grouped) > 12:
                print(f"        ... {len(grouped) - 12} more")
        else:
            print(f"same  {label}")
    print(f"{failed} of {len(cases)} cases differ")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
