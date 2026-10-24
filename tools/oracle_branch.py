"""Dev tool: Save Project As, Branch Project, export and import through the live pack and this app on two copies of a scratch
project, comparing the replies, the files and the sessions each one leaves.

    python tools/oracle_branch.py --source <scratch project> [--pack http://127.0.0.1:8188] [--app http://127.0.0.1:8190]

The source project is copied to `<work>/pack/Heat` and `<work>/app/Heat` (work defaults to C:\\h3scratch\\branch). Every path in
the replies and sessions has its side's work folder replaced by `<W>` before comparing. Nothing touches the source. Use a short
work folder: Windows stops at 260 characters.
"""

import argparse
import json
import re
import shutil
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_files import close  # noqa: E402


def post(base, path, body):
    request = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=1800) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        text = error.read().decode("utf-8", "replace")
        try:
            return error.code, json.loads(text)
        except ValueError:
            return error.code, {"_raw": text[:200]}


def normalize(value, work):
    text = json.dumps(value, sort_keys=True)
    text = text.replace(str(work).replace("\\", "\\\\"), "<W>").replace(str(work), "<W>")
    text = re.sub(r"\d{8}[-_]\d{6}", "<T>", text)
    return json.loads(text)


def listing(root):
    skip = ("session_backups",)
    names = sorted(str(p.relative_to(root)).replace("\\", "/") for p in root.rglob("*") if p.is_file() and not any(part in skip for part in p.relative_to(root).parts))
    return sorted(re.sub(r"\d{8}[-_]\d{6}(_\d\d)?", "<T>", n) for n in names)


def drop_volatile(session):
    for key in ("updated", "builder_save_revision", "render_logs"):
        session.pop(key, None)
    return session


def compare(label, pack_reply, app_reply, pack_work, app_work, extra=()):
    p, a = normalize(pack_reply[1], pack_work), normalize(app_reply[1], app_work)
    for side in (p, a):
        if isinstance(side, dict) and isinstance(side.get("session"), dict):
            drop_volatile(side["session"])
        for key in ("updated", "elapsed"):
            side.pop(key, None)
    differences = [] if pack_reply[0] == app_reply[0] else [f"status {pack_reply[0]} (pack) vs {app_reply[0]} (app)"]
    differences += close(p, a)
    differences += list(extra)
    print(("PASS " if not differences else "FAIL ") + label + (f" ({len(differences)} differences)" if differences else ""))
    for line in differences[:14]:
        print("     ", line)
    return not differences


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True)
    parser.add_argument("--pack", default="http://127.0.0.1:8188")
    parser.add_argument("--app", default="http://127.0.0.1:8190")
    parser.add_argument("--work", default=r"C:\h3scratch\branch")
    args = parser.parse_args()
    root = Path(args.work)
    shutil.rmtree(root, ignore_errors=True)
    sides = {"pack": args.pack, "app": args.app}
    works, projects = {}, {}
    for side in sides:
        works[side] = root / side
        projects[side] = works[side] / "Heat"
        shutil.copytree(args.source, projects[side], ignore=shutil.ignore_patterns("session_backups"))
    ok = True

    def run(label, route, build, keep_files=True):
        replies = {}
        for side, base in sides.items():
            replies[side] = post(base, route, build(side))
        extra = []
        if keep_files and replies["pack"][0] == 200 and replies["app"][0] == 200:
            targets = {side: Path(replies[side][1]["project_folder"]) for side in sides}
            names = {side: listing(targets[side]) for side in sides}
            only_pack = sorted(set(names["pack"]) - set(names["app"]))
            only_app = sorted(set(names["app"]) - set(names["pack"]))
            extra += [f"file only in pack: {n}" for n in only_pack[:6]] + [f"file only in app: {n}" for n in only_app[:6]]
        return compare(label, replies["pack"], replies["app"], works["pack"], works["app"], extra), replies

    def opened(side):
        status, reply = post(sides[side], "/vrgdg/music_builder/load_session", {"project_folder": str(projects[side])})
        if status != 200:
            sys.exit(f"{side} could not open the copy: {reply}")
        return reply["session"]

    def audio_of(side):
        found = sorted((projects[side] / "project_audio").glob("project_audio.*"))
        return str(found[0]) if found else ""

    cases = [
        ("Save Project As keeps everything", {}),
        ("Branch keeping only lyrics and notes", {"keep": {"lyrics": True, "notes": True}}),
        ("Branch keeping prompts and mappings", {"keep": {"prompts": True, "mappings": True, "overlays": True}}),
        ("Branch keeping images only", {"keep": {"images": True}}),
        ("Branch keeping videos only", {"keep": {"videos": True}}),
        ("Branch keeping nothing", {"keep": {}}),
    ]
    for number, (label, extra) in enumerate(cases, start=1):
        def build(side, number=number, extra=extra):
            body = {"source_project_folder": str(projects[side]), "target_project_folder": str(works[side] / f"copy{number}"), "session": opened(side), "audio_path": audio_of(side)}
            body.update(extra)
            return body

        passed, _ = run(label, "/vrgdg/music_builder/save_project_as", build)
        ok &= passed

    def inside(side):
        return {"source_project_folder": str(projects[side]), "target_project_folder": str(projects[side] / "inside"), "session": opened(side)}

    passed, _ = run("a target inside the project is refused", "/vrgdg/music_builder/save_project_as", inside, keep_files=False)
    ok &= passed

    # Export on the pack's own copy and ours, then import each package into its own side and compare the opened sessions.
    for side, base in sides.items():
        query = urllib.parse.urlencode({"project_folder": str(projects[side])})
        with urllib.request.urlopen(f"{base}/vrgdg/music_builder/export_project?{query}", timeout=1800) as response:
            (works[side] / "export.zip").write_bytes(response.read())
    print("exported:", {side: (works[side] / "export.zip").stat().st_size for side in sides})
    ok &= compare_zips(works)
    return 0 if ok else 1


def compare_zips(works):
    import zipfile

    names = {}
    sessions = {}
    for side in works:
        with zipfile.ZipFile(works[side] / "export.zip") as archive:
            names[side] = sorted(re.sub(r"\d{8}[-_]\d{6}(_\d\d)?", "<T>", n) for n in archive.namelist() if not n.startswith("session_backups"))
            sessions[side] = normalize(json.loads(archive.read("vrgdg_builder_session.json").decode("utf-8-sig")), works[side])
    differences = []
    only_pack = sorted(set(names["pack"]) - set(names["app"]))
    only_app = sorted(set(names["app"]) - set(names["pack"]))
    differences += [f"file only in pack zip: {n}" for n in only_pack[:6]] + [f"file only in app zip: {n}" for n in only_app[:6]]
    for session in sessions.values():
        drop_volatile(session)
    differences += close(sessions["pack"], sessions["app"])
    print(("PASS " if not differences else "FAIL ") + "export package" + (f" ({len(differences)} differences)" if differences else ""))
    for line in differences[:14]:
        print("     ", line)
    return not differences


if __name__ == "__main__":
    raise SystemExit(main())
