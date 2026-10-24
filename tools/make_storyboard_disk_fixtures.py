"""Dev tool: golden flows for the Storyboard file (load, save, export prompts, import a reference picture), from the pack's own code.

    python tools/make_storyboard_disk_fixtures.py --pack <pack folder>

Writes ``fixtures/story/disk.json``: for each flow its steps (function and request, with ``<TMP>`` for the project folder), what each
step returned or raised, and the files the flow left in the project.
"""

import argparse
import importlib
import json
import re
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402
import pack_env  # noqa: E402

SCENES = [
    {"id": "s1", "label": "Verse 1", "lyrics": "walking home", "image_prompt": "A street at night.", "video_prompt": "Slow push in on the street.", "subjects": ["Ann"], "timeline_start": 0, "timeline_end": 4.5, "project_video_engine": "minimax_h3",
     "minimax_h3_pass2_prompt": "Pass two text."},
    {"id": "s2", "label": "", "lyric_text": "la la", "t2i_prompt": "Rain.", "i2v_prompt": "Rain falls.", "image_path": "C:/a.png", "start": 4.5, "end": 9, "project_video_engine": "minimax_h3"},
    {"scene_number": 5, "lyrics": "x", "video_prompt": "Ann sings with quiet intensity.", "subjects": ["Ann"], "project_video_engine": "ltx"},
]
STORYBOARD = {"created_at": "2026-01-01T00:00:00", "mode": "storyboard_prompts", "project_video_engine": "minimax_h3", "performance_mode": "Singing", "camera_flow": "dynamic",
              "send_adjacent_lyric_context": 1, "custom_camera_flow_sequence": [{"shot": "wide", "camera": "push"}, "x"], "minimax_h3_cut_frequency": "3", "minimax_h3_cut_style": "continuous",
              "fx_preset": "grain", "fx_custom_json": '{"a": 1}', "camera_motion_speed": 0, "character_motion_speed": "7", "story_layer": {"overall_story_idea": "A walk.", "lyricStoryStrength": 4,
              "story_flow_version": 2, "story_book": {"title": "T", "chapters": [{"title": "One", "paragraphs": ["p"]}]}}, "script_import": {}, "reference_builder": {"subjects": [{"name": "Ann"}]},
              "source_scene_ids": ["s1", 2], "scenes": SCENES}
PIXEL = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="


def flows():
    return [
        ("load without a file", [("load", {"project_folder": "<TMP>/a", "scenes": SCENES, "project_video_engine": "minimax_h3", "camera_motion_speed": 0, "temporalAllowBackgroundExtras": False})]),
        ("save, save again, conflict, load", [
            ("save", {"project_folder": "<TMP>/b", "storyboard": STORYBOARD}),
            ("save", {"project_folder": "<TMP>/b", "storyboard": {**STORYBOARD, "camera_flow": "calm"}, "expected_revision": 1}),
            ("save", {"project_folder": "<TMP>/b", "storyboard": STORYBOARD, "expected_revision": 1}),
            ("save", {"project_folder": "<TMP>/b", "storyboard": STORYBOARD, "expected_revision": "x"}),
            ("load", {"project_folder": "<TMP>/b"}),
        ]),
        ("save with a bad body", [("save", {"project_folder": "<TMP>/c", "storyboard": "no"}), ("save", {"project_folder": "<TMP>/c"})]),
        ("export keeps pass 2 prompts of an earlier export", [
            ("export", {"project_folder": "<TMP>/d", "storyboard": STORYBOARD}),
            ("export", {"project_folder": "<TMP>/d", "storyboard": {**STORYBOARD, "scenes": [{**SCENES[0], "minimax_h3_pass2_prompt": ""}, {**SCENES[1]}]}}),
            ("export", {"project_folder": "<TMP>/d", "storyboard": STORYBOARD, "expected_revision": 9}),
        ]),
        ("reference pictures", [
            ("import_image", {"project_folder": "<TMP>/e", "kind": "subject", "name": "Ann Lee", "description": " a tall singer ", "image_data": PIXEL}),
            ("import_image", {"project_folder": "<TMP>/e", "kind": "subject", "name": "Ann Lee", "image_data": PIXEL, "id": "fixed_id"}),
            ("import_image", {"project_folder": "<TMP>/e", "kind": "Location", "image_data": PIXEL}),
            ("import_image", {"project_folder": "<TMP>/e", "kind": "weird", "name": "???", "data": PIXEL.replace("image/png", "image/jpeg")}),
            ("import_image", {"project_folder": "<TMP>/e", "image_data": "data:image/png;base64,"}),
            ("import_image", {"project_folder": "<TMP>/e", "image_data": "***"}),
        ]),
    ]


def slashes(value):
    """Every backslash of a path in the JSON value becomes a slash, so both apps compare equal on Windows."""
    if isinstance(value, str):
        return value.replace("\\", "/")
    if isinstance(value, list):
        return [slashes(item) for item in value]
    if isinstance(value, dict):
        return {key.replace("\\", "/"): slashes(item) for key, item in value.items()}
    return value


def snapshot(root):
    files = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        relative = str(path.relative_to(root)).replace("\\", "/")
        data = path.read_bytes()
        if path.suffix == ".json":
            files[relative] = json.loads(data.decode("utf-8"))
        elif path.suffix == ".txt":
            files[relative] = data.decode("utf-8")
        else:
            files[relative] = {"bytes": len(data)}
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    persistence = importlib.import_module("vrgdg_pack.storyboard.persistence")
    functions = {"load": persistence._load_storyboard, "save": persistence._save_storyboard, "export": persistence._export_storyboard_prompts, "import_image": persistence._import_storyboard_reference_image}
    out = []
    for name, steps in flows():
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            results = []
            for function, request in steps:
                payload = json.loads(json.dumps(request).replace("<TMP>", str(root).replace("\\", "/")))
                try:
                    results.append({"return": functions[function](payload)})
                except persistence.StoryboardConflictError as exc:
                    results.append({"conflict": True, "error": str(exc), "current_revision": exc.current_revision})
                except Exception as exc:  # noqa: BLE001
                    results.append({"error": str(exc)})
            record = {"name": name, "steps": [{"fn": f, "request": r} for f, r in steps], "results": results, "files": snapshot(root)}
            out.append(slashes(h.scrub(record, str(root))))
    text = json.dumps({"flows": out}, ensure_ascii=False, separators=(",", ":"))
    text = re.sub(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d", "<T>", text)
    text = re.sub(r"\d{14}", "<T14>", text)
    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "disk.json"
    target.write_text(text, encoding="utf-8")
    print(f"disk.json: {len(out)} flows, {sum(len(f['results']) for f in out)} steps")
    return 0


if __name__ == "__main__":
    sys.exit(main())
