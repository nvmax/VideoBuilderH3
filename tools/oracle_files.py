"""Dev tool: run the same file operations through the live pack and this app on two copies of a scratch project and compare
the replies and the files each one leaves.

    python tools/oracle_files.py --source <scratch project> [--pack http://127.0.0.1:8188] [--app http://127.0.0.1:8190]

The source project is copied to `<work>/pack/Heat` and `<work>/app/Heat` (work defaults to C:\\h3scratch\\oracle). Nothing
touches the source. Every reply has its project folder replaced by `<P>` and its time stamps by `<T>` before comparing.
"""

import argparse
import json
import re
import shutil
import sys
import urllib.error
import urllib.request
from pathlib import Path


def post(base, path, body):
    request = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=900) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        text = error.read().decode("utf-8", "replace")
        try:
            return error.code, json.loads(text)
        except ValueError:
            return error.code, {"_raw": text[:200]}


def normalize(value, project):
    text = json.dumps(value, sort_keys=True)
    text = text.replace(str(project).replace("\\", "\\\\"), "<P>").replace(str(project), "<P>")
    text = re.sub(r"\d{8}[-_]\d{6}", "<T>", text)
    return json.loads(text)


def close(a, b, path=""):
    """Equal, treating numbers within a small tolerance as equal. Returns a list of differences."""
    if isinstance(a, dict) and isinstance(b, dict):
        out = []
        for key in sorted(a.keys() | b.keys()):
            if key not in a or key not in b:
                out.append(f"{path}.{key}: only in {'app' if key in b else 'pack'}")
            else:
                out += close(a[key], b[key], f"{path}.{key}")
        return out
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return [f"{path}: {len(a)} items (pack) vs {len(b)} (app)"]
        return [d for x, y in zip(a, b) for d in close(x, y, path + "[]")]
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool) and not isinstance(b, bool):
        return [] if abs(a - b) <= max(0.06, abs(a) * 0.01) else [f"{path}: {a} (pack) vs {b} (app)"]
    return [] if a == b else [f"{path}: {str(a)[:70]!r} (pack) vs {str(b)[:70]!r} (app)"]


def make_png():
    """A small valid PNG, as base64."""
    import base64
    import struct
    import zlib

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    rows = b"".join(bytes([0]) + bytes([200, 40, 40] * 8) for _ in range(6))
    png = bytes([137]) + b"PNG" + bytes([13, 10, 26, 10]) + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 6, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
    return base64.b64encode(png).decode()


def listing(project):
    skip = ("audio_masks", "session_backups")
    return sorted(str(p.relative_to(project)).replace("\\", "/") for p in project.rglob("*") if p.is_file() and not any(part in skip for part in p.relative_to(project).parts))


def tidy(names):
    return sorted(re.sub(r"\d{8}[-_]\d{6}(_\d\d)?", "<T>", n) for n in names)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True)
    parser.add_argument("--pack", default="http://127.0.0.1:8188")
    parser.add_argument("--app", default="http://127.0.0.1:8190")
    parser.add_argument("--work", default=r"C:\h3scratch\oracle")
    args = parser.parse_args()
    work = Path(args.work)
    shutil.rmtree(work, ignore_errors=True)
    projects = {}
    for side in ("pack", "app"):
        projects[side] = work / side / "Heat"
        shutil.copytree(args.source, projects[side], ignore=shutil.ignore_patterns("audio_masks", "session_backups", "latents"))
    P = projects["app"]
    session = json.loads((P / "vrgdg_builder_session.json").read_text(encoding="utf-8-sig"))
    clip1 = P / "rendered_scene_videos" / "video_0001-audio.mp4"
    clip2 = P / "rendered_scene_videos" / "video_0002-audio.mp4"
    pictures = sorted(p for p in P.rglob("*") if p.suffix.lower() in (".png", ".jpg", ".jpeg", ".webp") and "scene_video_thumbnails" in p.parts or p.parent.name == "zimage_approved")
    image1, image2 = (pictures + [None, None])[:2]
    audio = next(iter(sorted((P / "project_audio").glob("project_audio.*"))))
    print("scenes:", len(session["segments"]), "| clips:", clip1.is_file(), clip2.is_file(), "| images:", bool(image1), bool(image2))

    def rel(side, path):
        return str(projects[side] / Path(path).relative_to(P))

    # (route, payload with paths relative to the project written as {"@": "relative"})
    def pay(side, spec):
        def conv(v):
            if isinstance(v, dict) and set(v) == {"@"}:
                return rel(side, P / v["@"])
            if isinstance(v, dict):
                return {k: conv(x) for k, x in v.items()}
            if isinstance(v, list):
                return [conv(x) for x in v]
            return v
        out = conv(spec)
        out.setdefault("project_folder", str(projects[side]))
        return out

    steps = [
        ("find_scene_video_output", {"scene_number": 2, "video_mode": "i2v"}),
        ("prepare_scene_audio_clip", {"audio_path": {"@": f"project_audio/{audio.name}"}, "scene_number": 3, "start_seconds": 1.25, "duration_seconds": 2.5}),
        ("trim_scene_video", {"source_path": {"@": "rendered_scene_videos/video_0001-audio.mp4"}, "scene_number": 1, "start": 0.2, "duration": 1.5, "label": "Oracle Test!", "mark_as_audio_video": False}),
        ("collect_scene_video", {"source_path": {"@": "rendered_scene_videos/video_0002-audio.mp4"}, "scene_number": 3, "existing_action": "backup"}),
        ("collect_scene_video", {"source_path": {"@": "rendered_scene_videos/video_0001-audio.mp4"}, "scene_number": 3, "existing_action": "backup"}),
        ("match_scene_video_start_color", {"video_path": {"@": "rendered_scene_videos/video_0003-audio.mp4"}, "reference_video_path": {"@": "rendered_scene_videos/video_0002-audio.mp4"}, "fade_seconds": 0.8, "strength": 0.7}) if clip2.is_file() else None,
        ("stitch_scene_videos", {"scene_paths": [{"@": "rendered_scene_videos/video_0002-audio.mp4"}, {"@": "rendered_scene_videos/video_0003-audio.mp4"}], "audio_path": {"@": f"project_audio/{audio.name}"}, "audio_start": 4.2, "audio_duration": 6.0, "output_prefix": "ORACLE_STITCH"}),
        ("render_image_slideshow", {"image_items": [{"path": {"@": str(image1.relative_to(P)).replace("\\", "/")}, "duration": 1.2}, {"path": {"@": str(image2.relative_to(P)).replace("\\", "/")}, "duration": 0.8}], "audio_path": {"@": f"project_audio/{audio.name}"}, "width": 640, "height": 360, "output_prefix": "ORACLE_SLIDES"}) if image1 and image2 else None,
    ]
    png = "data:image/png;base64," + make_png()
    steps += [
        ("music_builder/save_project_srt", {"srt_text": "1\n00:00:00,000 --> 00:00:02,500\nFirst line\n\n2\n00:00:02,500 --> 00:00:05,000\nSecond\nline\n"}),
        ("music_builder/save_single_scene_srt", {"scene_number": 4, "start_time": 12.5, "duration": 3.2, "label": "Chorus"}),
        ("music_builder/create_silent_audio", {"duration": 2.5}),
        ("music_builder/create_silent_audio", {"duration": 1.25, "scope": "scene", "scene_number": 5}),
        ("music_builder/save_scene_audio", {"scene_number": 6, "source_path": {"@": f"project_audio/{audio.name}"}}),
        ("music_builder/trim_scene_audio", {"source_path": {"@": f"project_audio/{audio.name}"}, "scene_number": 7, "start": 3.0, "duration": 2.0}),
        ("music_builder/trim_scene_audio", {"source_path": {"@": f"project_audio/{audio.name}"}, "scene_number": 7, "start": 9999.0, "duration": 2.0}),
        ("music_builder/prepare_scene_audio_mix", {"global_audio_path": {"@": f"project_audio/{audio.name}"}, "segments": [{"start": 1.0, "end": 3.0, "label": "a"}, {"start": 5.0, "end": 6.5, "label": "b"}], "srt_text_field": "label"}),
        ("music_builder/prepare_audio_clip_mix", {"duration": 6, "clips": [{"path": {"@": f"project_audio/{audio.name}"}, "start": 1.0, "source_start": 2.0, "duration": 2.0, "volume": 0.5}]}),
        ("music_builder/save_scene_image", {"scene_number": 2, "image_data": png}),
        ("music_builder/archive_scene_image", {"scene_number": 2, "image_data": png}),
        ("music_builder/save_flux_reference_image", {"reference_type": "subject", "name": "Oracle Singer", "image_data": png}),
        ("music_builder/extract_video_final_frame", {"source_path": {"@": "rendered_scene_videos/video_0002-audio.mp4"}, "scene_number": 3}),
        ("music_builder/delete_project_media", {"path": {"@": "rendered_scene_videos_backup"}}),
    ]
    failed = 0
    for step in steps:
        if step is None:
            continue
        route, spec = step
        path = "/vrgdg/" + route if "/" in route else "/vrgdg/workflow_runner/" + route
        results = {}
        for side, base in (("pack", args.pack), ("app", args.app)):
            results[side] = post(base, path, pay(side, spec))
        (status_p, reply_p), (status_a, reply_a) = results["pack"], results["app"]
        np, na = normalize(reply_p, projects["pack"]), normalize(reply_a, projects["app"])
        problems = [] if status_p == status_a else [f"status {status_p} (pack) vs {status_a} (app)"]
        problems += close(np, na)
        if problems:
            failed += 1
            print(f"DIFF  {route}")
            for line in problems[:10]:
                print("        " + line)
        else:
            print(f"same  {route}")
    # The cached edit mix is named from a hash of the request. Asked of the app on the pack's own folder, the same request
    # must find the file the pack made, which shows both apps hash alike.
    clip_request = pay("pack", {"duration": 6, "clips": [{"path": {"@": f"project_audio/{audio.name}"}, "start": 1.0, "source_start": 2.0, "duration": 2.0, "volume": 0.5}]})
    _, from_pack = post(args.pack, "/vrgdg/music_builder/prepare_audio_clip_mix", clip_request)
    before = set(listing(projects["pack"]))
    _, from_app = post(args.app, "/vrgdg/music_builder/prepare_audio_clip_mix", clip_request)
    same_name = from_pack.get("audio_path") == from_app.get("audio_path") and set(listing(projects["pack"])) == before
    print(("same  " if same_name else "DIFF  ") + "audio edit cache name")
    failed += not same_name
    files_pack = tidy(listing(projects["pack"]))
    files_app = tidy(listing(projects["app"]))
    only_pack = sorted(set(files_pack) - set(files_app))
    only_app = sorted(set(files_app) - set(files_pack))
    print(f"files: {len(files_pack)} (pack) {len(files_app)} (app); only in pack {only_pack[:6]}; only in app {only_app[:6]}")
    failed += bool(only_pack or only_app)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
