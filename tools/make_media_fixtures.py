"""Dev tool: golden results for the media layer (``h3-media``), made by running the pack's own ffmpeg code.

    python tools/make_media_fixtures.py --pack <pack folder>

Needs ffmpeg and ffprobe on PATH. Every case lists the ffmpeg commands that make its small synthetic inputs (test
pattern video, sine audio), so nothing binary is committed. The Rust tests run the same commands, run their own port of
the operation and compare what ffprobe reports about the result. The pack is not modified.

Writes ``fixtures/media/media_cases.json``.
"""

import argparse
import contextlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

BASE_TIME = 1_700_000_000
# One explicit ffmpeg for everything. Windows starts a program from the application's own folder first, and ComfyUI's
# portable Python folder carries an ffmpeg of its own, which would otherwise win over the one on PATH.
FFMPEG = shutil.which("ffmpeg") or "ffmpeg"
FFPROBE = shutil.which("ffprobe") or "ffprobe"


@contextlib.contextmanager
def frozen(t):
    real_time, real_strftime = time.time, time.strftime
    time.time = lambda: BASE_TIME + t + 0.123
    time.strftime = lambda fmt, *a: f"20260101_{t // 3600 % 24:02d}{t // 60 % 60:02d}{t % 60:02d}"
    try:
        yield
    finally:
        time.time, time.strftime = real_time, real_strftime


def ffmpeg(work: Path, args):
    result = subprocess.run([FFMPEG, "-y", "-v", "error", *args], cwd=work, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(result.stderr)


def probe(path: Path) -> dict:
    """What ffprobe says about a file: frames, frame rate, durations and size. Empty when it does not exist."""
    if not path.is_file():
        return {}
    video = subprocess.run(
        [FFPROBE, "-v", "error", "-select_streams", "v:0", "-count_packets", "-show_entries",
         "stream=nb_read_packets,r_frame_rate,width,height,duration", "-of", "json", str(path)],
        capture_output=True, text=True)
    audio = subprocess.run(
        [FFPROBE, "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=duration,sample_rate", "-of", "json", str(path)],
        capture_output=True, text=True)
    fmt = subprocess.run([FFPROBE, "-v", "error", "-show_entries", "format=duration", "-of", "json", str(path)], capture_output=True, text=True)
    v = (json.loads(video.stdout or "{}").get("streams") or [{}])[0]
    a = (json.loads(audio.stdout or "{}").get("streams") or [{}])[0]
    f = json.loads(fmt.stdout or "{}").get("format") or {}
    return {
        "frames": int(v.get("nb_read_packets") or 0),
        "fps": v.get("r_frame_rate"),
        "width": v.get("width"),
        "height": v.get("height"),
        "video_duration": round(float(v.get("duration") or 0), 3),
        "has_audio": bool(a),
        "audio_duration": round(float(a.get("duration") or 0), 2) if a else 0,
        "format_duration": round(float(f.get("duration") or 0), 2),
    }


def audio_hash(path: Path) -> str:
    """SHA1 of the decoded audio (16 bit stereo 48 kHz), so two encodes of the same samples compare equal."""
    result = subprocess.run(
        [FFMPEG, "-v", "error", "-i", str(path), "-map", "0:a:0", "-ac", "2", "-ar", "48000", "-f", "s16le", "-"],
        capture_output=True)
    import hashlib
    return hashlib.sha1(result.stdout).hexdigest() if result.returncode == 0 and result.stdout else ""


def rel(root: Path, path) -> str:
    return os.path.relpath(path, root).replace("\\", "/") if path else ""


def tree(root: Path) -> list:
    out = []
    for base, dirs, files in os.walk(root):
        for name in sorted(files):
            out.append(os.path.relpath(os.path.join(base, name), root).replace("\\", "/"))
    return sorted(out)


# Inputs: a 14 s song, scene clips cut from it (their audio is the song), and clips with other audio.
SONG = {"out": "song.wav", "args": ["-f", "lavfi", "-i", "sine=frequency=330:duration=14", "-f", "lavfi", "-i", "sine=frequency=550:duration=14",
                                     "-filter_complex", "[0][1]amix=inputs=2:duration=first,volume=1.5", "-ar", "44100", "-ac", "2"]}


def song_clip(name: str, start: float, duration: float, size="320x180", fps=24, rate_offset=0) -> dict:
    """A clip whose audio is the song from ``start``, picture a test pattern."""
    return {"out": name, "args": [
        "-f", "lavfi", "-i", f"testsrc2=size={size}:rate={fps}:duration={duration + 0.5}",
        "-ss", f"{start}", "-t", f"{duration}", "-i", "song.wav",
        "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-c:a", "aac", "-shortest"]}


def other_clip(name: str, duration: float, freq=880, size="320x180", fps=24) -> dict:
    return {"out": name, "args": [
        "-f", "lavfi", "-i", f"testsrc2=size={size}:rate={fps}:duration={duration}",
        "-f", "lavfi", "-i", f"sine=frequency={freq}:duration={duration}",
        "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-c:a", "aac", "-shortest"]}


def silent_clip(name: str, duration: float, size="320x180", fps=24) -> dict:
    return {"out": name, "args": [
        "-f", "lavfi", "-i", f"testsrc2=size={size}:rate={fps}:duration={duration}",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast"]}


def make_assets(work: Path, assets: list):
    for asset in assets:
        ffmpeg(work, [*asset["args"], asset["out"]])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "media")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    import importlib

    vf = importlib.import_module("vrgdg_pack.runner.video_files")
    media_module = importlib.import_module("vrgdg_pack.builder.media")
    vf._find_ffmpeg_path = lambda: FFMPEG
    media_module._find_ffmpeg_path = lambda: FFMPEG
    cases = []

    def run_case(name, assets, steps):
        """``steps`` is a list of ``{"op": ..., ...}``. Each op runs in the pack and records what it did."""
        # One encoder thread, so the same command gives the same file in every process. A threaded x264 can place
        # key frames differently from run to run, which moves where a later seek lands.
        assets = [{**a, "args": a["args"] + (["-threads", "1"] if a["out"].endswith(".mp4") else [])} for a in assets]
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "proj").mkdir()
            make_assets(work, assets)
            results = []
            for step in steps:
                op = step["op"]
                with frozen(step.get("t", 0)):
                    results.append(OPS[op](vf, work, step))
            cases.append({"name": name, "assets": assets, "steps": steps, "results": results, "tree": [t for t in tree(work) if not t.startswith(("song", "scene", "raw", "other", "silent", "insert"))]})

    # -- operations ---------------------------------------------------------------------------------------------------------
    def op_trim(vf, work, step):
        result = vf._trim_scene_video({
            "project_folder": str(work / "proj"), "scene_number": step["scene"], "source_path": str(work / step["source"]),
            "start": step["start"], "duration": step["duration"], "frames": step.get("frames", 0), "label": step.get("label", "trim"),
            "mark_as_audio_video": step.get("mark_audio", False),
        })
        video = Path(result["video_path"])
        return {"video": rel(work, video), "thumbnail": rel(work, result["thumbnail_path"]), "probe": probe(video), "audio_hash": audio_hash(video)}

    def op_collect(vf, work, step):
        result = vf._collect_scene_video({
            "project_folder": str(work / "proj"), "scene_number": step["scene"], "source_path": str(work / step["source"]),
            "existing_action": step.get("existing_action", "backup"),
        })
        video = Path(result["video_path"])
        return {"video": rel(work, video), "thumbnail": rel(work, result["thumbnail_path"]), "backup": rel(work, result["backup_path"]),
                "backup_thumbnail": rel(work, result["backup_thumbnail_path"]), "probe": probe(video)}

    def op_final_frame(vf, work, step):
        media = importlib.import_module("vrgdg_pack.builder.media")
        result = media._extract_video_final_frame_as_scene_image({
            "project_folder": str(work / "proj"), "source_path": str(work / step["source"]), "scene_number": step["scene"]})
        path = Path(result["saved_path"])
        return {"image": rel(work, path), "exists": path.is_file() and path.stat().st_size > 0}

    def op_color_match(vf, work, step):
        result = vf._apply_scene_start_color_match({
            "project_folder": str(work / "proj"), "video_path": str(work / step["video"]), "reference_video_path": str(work / step["reference"]),
            "fade_seconds": step.get("fade", 1.0), "strength": step.get("strength", 0.85)})
        video = Path(result["video_path"])
        return {"applied": result["applied"], "video": rel(work, video), "probe": probe(video), "audio_hash": audio_hash(video),
                "thumbnail": rel(work, result.get("thumbnail_path", ""))}

    def op_stitch(vf, work, step):
        payload = dict(step["payload"])
        payload["project_folder"] = str(work / "proj")
        for key in ("scene_paths",):
            payload[key] = [str(work / p) for p in payload[key]]
        for key in ("audio_path", "song_path"):
            if payload.get(key):
                payload[key] = str(work / payload[key])
        payload["overlay_items"] = [{**o, "path": str(work / o["path"])} for o in payload.get("overlay_items", [])]
        result = vf._stitch_scene_videos(payload)
        final = Path(result["final_video_path"])
        return {
            "final": rel(work, final), "probe": probe(final), "audio_hash": audio_hash(final),
            "embedded_audio_mode": result["embedded_audio_mode"], "normalized_canvas": result["normalized_canvas"],
            "timeline_frame_sync": result["timeline_frame_sync"], "timeline_frame_count": result["timeline_frame_count"],
            "scene_count": result["scene_count"], "insert_count": result["insert_count"], "used_scene_audio": result["used_scene_audio"],
            "output_width": result["output_width"], "output_height": result["output_height"],
        }

    OPS = {"trim": op_trim, "collect": op_collect, "final_frame": op_final_frame, "color_match": op_color_match, "stitch": op_stitch}

    # -- trim, collect, final frame, color match ----------------------------------------------------------------------------
    raw = song_clip("raw.mp4", 2.0, 3.5)
    run_case("trim_exact_frames", [SONG, raw], [{"op": "trim", "source": "raw.mp4", "scene": 2, "start": 0.25, "duration": 2.0, "frames": 48, "label": "minimax_exact", "mark_audio": True, "t": 5}])
    run_case("trim_without_frame_count", [SONG, raw], [{"op": "trim", "source": "raw.mp4", "scene": 3, "start": 0.0, "duration": 1.5, "label": "Odd Label!", "t": 6}])
    run_case("trim_twice_same_second", [SONG, raw], [
        {"op": "trim", "source": "raw.mp4", "scene": 1, "start": 0.0, "duration": 1.0, "frames": 24, "label": "minimax_exact", "mark_audio": True, "t": 7},
        {"op": "trim", "source": "raw.mp4", "scene": 1, "start": 0.5, "duration": 1.0, "frames": 24, "label": "minimax_exact", "mark_audio": True, "t": 7}])
    run_case("collect_replace_with_backup", [SONG, raw], [
        {"op": "trim", "source": "raw.mp4", "scene": 4, "start": 0.0, "duration": 1.0, "frames": 24, "label": "minimax_exact", "mark_audio": True, "t": 1},
        {"op": "collect", "scene": 4, "source": "proj/rendered_scene_videos/video_0004-minimax_exact_20260101_000001-audio.mp4", "t": 2},
        {"op": "trim", "source": "raw.mp4", "scene": 4, "start": 1.0, "duration": 1.0, "frames": 24, "label": "minimax_exact", "mark_audio": True, "t": 3},
        {"op": "collect", "scene": 4, "source": "proj/rendered_scene_videos/video_0004-minimax_exact_20260101_000003-audio.mp4", "t": 4},
        {"op": "trim", "source": "raw.mp4", "scene": 4, "start": 1.5, "duration": 1.0, "frames": 24, "label": "minimax_exact", "mark_audio": True, "t": 5},
        {"op": "collect", "scene": 4, "source": "proj/rendered_scene_videos/video_0004-minimax_exact_20260101_000005-audio.mp4", "existing_action": "overwrite", "t": 6}])
    run_case("final_frame", [SONG, raw], [
        {"op": "trim", "source": "raw.mp4", "scene": 1, "start": 0.0, "duration": 1.0, "frames": 24, "label": "x", "t": 1},
        {"op": "final_frame", "source": "proj/rendered_scene_videos/video_0001-x_20260101_000001.mp4", "scene": 2, "t": 2},
        {"op": "final_frame", "source": "proj/rendered_scene_videos/video_0001-x_20260101_000001.mp4", "scene": 2, "t": 2}])
    pattern_a = {"out": "proj/scene_a.mp4", "args": ["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=24:duration=2", "-f", "lavfi", "-i", "sine=frequency=500:duration=2",
                                                 "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-c:a", "aac", "-shortest"]}
    pattern_b = {"out": "proj/scene_b.mp4", "args": ["-f", "lavfi", "-i", "smptebars=size=320x180:rate=24:duration=2", "-f", "lavfi", "-i", "sine=frequency=700:duration=2",
                                                 "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-c:a", "aac", "-shortest"]}
    run_case("color_match_opening", [pattern_a, pattern_b], [
        {"op": "color_match", "video": "proj/scene_b.mp4", "reference": "proj/scene_a.mp4", "fade": 1.0, "strength": 0.85, "t": 1}])

    # -- stitch -------------------------------------------------------------------------------------------------------------
    s1, s2, s3 = song_clip("scene1.mp4", 0.0, 3.0), song_clip("scene2.mp4", 3.0, 2.5), song_clip("scene3.mp4", 5.5, 4.0)
    base = {"scene_paths": ["scene1.mp4", "scene2.mp4", "scene3.mp4"], "audio_path": "song.wav", "song_path": "", "scene_audio_paths": [], "scene_audio_items": [],
            "overlay_items": [], "width": 0, "height": 0, "audio_start": 0, "audio_duration": 0, "output_prefix": "FINAL_VIDEO", "timeline_fps": 0,
            "scene_timing_items": [], "use_embedded_scene_audio": False}
    timing = [{"start": 0.0, "end": 3.0}, {"start": 3.0, "end": 5.5}, {"start": 5.5, "end": 9.5}]
    assets3 = [SONG, s1, s2, s3]
    run_case("stitch_project_song_plain", assets3, [{"op": "stitch", "payload": base}])
    run_case("stitch_project_song_timeline", assets3, [{"op": "stitch", "payload": {**base, "scene_timing_items": timing, "timeline_fps": 24}}])
    run_case("stitch_timeline_uneven_frames", assets3, [{"op": "stitch", "payload": {**base, "scene_timing_items": [{"start": 0.0, "end": 3.02}, {"start": 3.02, "end": 5.49}, {"start": 5.49, "end": 9.51}], "timeline_fps": 24}}])
    run_case("stitch_preview_window", assets3, [{"op": "stitch", "payload": {**base, "audio_start": 3.0, "audio_duration": 6.5, "scene_paths": ["scene2.mp4", "scene3.mp4"],
                                                                              "scene_timing_items": timing[:2], "timeline_fps": 24}}])
    run_case("stitch_embedded_continuous_song", assets3, [{"op": "stitch", "payload": {**base, "use_embedded_scene_audio": True, "audio_path": "", "song_path": "song.wav",
                                                                                    "scene_timing_items": timing, "timeline_fps": 24}}])
    run_case("stitch_embedded_continuous_song_no_timing", assets3, [{"op": "stitch", "payload": {**base, "use_embedded_scene_audio": True, "audio_path": "", "song_path": "song.wav"}}])
    o1, o2 = other_clip("scene1.mp4", 3.0, 600), other_clip("scene2.mp4", 2.5, 900)
    run_case("stitch_embedded_per_scene_own_audio", [SONG, o1, o2], [{"op": "stitch", "payload": {**base, "scene_paths": ["scene1.mp4", "scene2.mp4"], "use_embedded_scene_audio": True, "audio_path": "",
                                                                                              "song_path": "song.wav", "scene_timing_items": timing[:2], "timeline_fps": 24}}])
    short = other_clip("scene2.mp4", 1.2, 900)
    run_case("stitch_embedded_audio_runs_short", [SONG, o1, short], [{"op": "stitch", "payload": {**base, "scene_paths": ["scene1.mp4", "scene2.mp4"], "use_embedded_scene_audio": True,
                                                                                               "audio_path": "", "song_path": "song.wav",
                                                                                               "scene_timing_items": [{"start": 0.0, "end": 3.0}, {"start": 3.0, "end": 5.5}], "timeline_fps": 24}}])
    run_case("stitch_embedded_silent_clip_uses_song", [SONG, o1, silent_clip("scene2.mp4", 2.5)], [{"op": "stitch", "payload": {**base, "scene_paths": ["scene1.mp4", "scene2.mp4"], "use_embedded_scene_audio": True,
                                                                                                                           "audio_path": "", "song_path": "song.wav",
                                                                                                                           "scene_timing_items": timing[:2], "timeline_fps": 24}}])
    run_case("stitch_embedded_no_song", [SONG, o1, o2], [{"op": "stitch", "payload": {**base, "scene_paths": ["scene1.mp4", "scene2.mp4"], "use_embedded_scene_audio": True, "audio_path": "",
                                                                                   "song_path": "", "scene_timing_items": timing[:2], "timeline_fps": 24}}])
    ins = other_clip("insert1.mp4", 2.0, 300)
    run_case("stitch_with_insert", assets3 + [ins], [{"op": "stitch", "payload": {**base, "overlay_items": [{"path": "insert1.mp4", "start": 4.0, "end": 5.0, "source_start": 0.5}],
                                                                                "scene_timing_items": timing, "timeline_fps": 24}}])
    big = song_clip("scene1.mp4", 0.0, 3.0, size="400x300")
    run_case("stitch_canvas_normalized", [SONG, big, s2], [{"op": "stitch", "payload": {**base, "scene_paths": ["scene1.mp4", "scene2.mp4"], "width": 320, "height": 180,
                                                                                    "scene_timing_items": timing[:2], "timeline_fps": 24}}])
    run_case("stitch_second_output_gets_a_free_name", assets3, [{"op": "stitch", "payload": {**base, "scene_timing_items": timing, "timeline_fps": 24}, "t": 1},
                                                              {"op": "stitch", "payload": {**base, "scene_timing_items": timing, "timeline_fps": 24}, "t": 2}])
    version = subprocess.run([FFMPEG, "-version"], capture_output=True, text=True).stdout.splitlines()[0]
    (args.out / "media_cases.json").write_text(json.dumps({"ffmpeg_version": version, "cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"media_cases: {len(cases)} cases")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
