"""Dev tool: regenerate every fixture from the pack, read-only.

    python tools/make_all.py --pack <pack folder> [--comfy-url http://127.0.0.1:8188] [--sessions <ComfyUI output folder>]

Runs the generators in order, copies the pack's own case files, and writes ``fixtures/MANIFEST.json`` with the pack commit
the fixtures came from. Run it again after the pack changes, then run ``cargo test`` to see which Rust ports drifted.
The pack is never modified. ``--comfy-url`` refreshes the node snapshot, ``--sessions`` refreshes the session shape.
"""

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
FIXTURES = TOOLS.parent / "fixtures"

# Case files the pack's own tests use, copied unchanged (see COPIED_FROM.md).
CASE_FILES = [
    "minimax_craft_cases.json", "refmod_scene_cases.json", "story_acts_cases.json", "story_locations_cases.json",
    "story_mode_cases.json", "temporal_effect_cases.json", "video_style_cases.json",
]

GENERATORS = ["make_graph_fixtures.py", "make_core_fixtures.py", "make_scene_fixtures.py", "make_prompt_fixtures.py", "make_project_fixtures.py", "make_media_fixtures.py", "make_pipeline_fixtures.py", "make_image_fixtures.py", "make_timeline_fixtures.py", "make_scene_ops_fixtures.py", "make_assign_fixtures.py", "make_timed_lines_fixtures.py", "make_timestamp_graph_fixtures.py", "make_llm_fixtures.py", "make_style_fixtures.py", "make_prompt_writer_fixtures.py", "make_location_fixtures.py", "make_story_settings_fixtures.py", "make_masked_fixtures.py", "make_lyric_free_fixtures.py", "make_markers_fixtures.py", "make_refmod_graph_fixtures.py", "make_refmod_library_fixtures.py", "make_assembly_fixtures.py", "make_pipeline_plan_fixtures.py", "make_utility_fixtures.py", "make_grade_fixtures.py", "make_image_writer_fixtures.py", "make_reference_writer_fixtures.py", "make_storyboard_fixtures.py", "make_storyboard_disk_fixtures.py", "make_story_prompt_fixtures.py", "make_story_book_fixtures.py", "make_story_plan_fixtures.py", "make_story_places_fixtures.py", "make_story_beat_fixtures.py", "make_story_image_fixtures.py", "make_enhance_fixtures.py", "make_enhance_writer_fixtures.py", "make_motion_notes_fixtures.py"]


def run(*args) -> None:
    print(">", " ".join(str(a) for a in args))
    subprocess.run([sys.executable, "-W", "ignore", *map(str, args)], check=True)


def pack_commit(pack: Path) -> str:
    try:
        return subprocess.run(["git", "-C", str(pack), "rev-parse", "--short", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--comfy-url", default="")
    parser.add_argument("--sessions", type=Path, default=None)
    args = parser.parse_args()
    pack = args.pack.resolve()

    if args.comfy_url:
        run(TOOLS / "capture_object_info.py", "--pack", pack, "--url", args.comfy_url)
    for generator in GENERATORS:
        run(TOOLS / generator, "--pack", pack)
    if args.sessions:
        run(TOOLS / "session_shape.py", "--root", args.sessions)

    cases_dir = FIXTURES / "cases"
    cases_dir.mkdir(parents=True, exist_ok=True)
    for name in CASE_FILES:
        shutil.copyfile(pack / "tests" / name, cases_dir / name)

    counts = {}
    for folder in ("graphs", "core", "scene", "prompt", "project", "pipeline", "image", "timeline", "cases"):
        counts[folder] = len(list((FIXTURES / folder).glob("*.json")))
    manifest = {"pack_commit": pack_commit(pack), "python": sys.version.split()[0], "files": counts}
    (FIXTURES / "MANIFEST.json").write_text(json.dumps(manifest, indent=1), encoding="utf-8")
    print("manifest:", manifest)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
