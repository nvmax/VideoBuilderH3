"""Dev tool: split one scene with this app (ComfyUI's Get Stems node), then build the masked mix with this app and with the
live pack on the same stems and compare the two wav files.

    python tools/oracle_mask.py --project <scratch project folder> [--pack http://127.0.0.1:8188] [--app http://127.0.0.1:8190]

Use a scratch copy of a project. The scene's `audio_masks` folder is created, used and removed.
"""

import argparse
import json
import shutil
import sys
import urllib.error
import urllib.request
import wave
from pathlib import Path


def post(base, path, body):
    request = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=900) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raise SystemExit(f"{path}: HTTP {error.code} {error.read().decode('utf-8', 'replace')[:300]}")


def samples(path):
    with wave.open(str(path), "rb") as handle:
        return handle.getnchannels(), handle.getframerate(), handle.getnframes(), handle.readframes(handle.getnframes())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", required=True)
    parser.add_argument("--pack", default="http://127.0.0.1:8188")
    parser.add_argument("--app", default="http://127.0.0.1:8190")
    args = parser.parse_args()
    project = Path(args.project)
    session = post(args.app, "/vrgdg/music_builder/load_session", {"project_folder": str(project)})["session"]
    segment = session["segments"][0]
    scene = segment["id"]
    audio = session["audio_path"]
    print("scene length", round(segment["end"] - segment["start"], 2), "seconds")
    folder = project / "audio_masks" / scene
    try:
        state = post(args.app, "/vrgdg/music_builder/audio_mask/separate", {
            "project_folder": str(project), "scene_id": scene, "audio_path": audio, "start_seconds": segment["start"],
            "duration_seconds": segment["end"] - segment["start"], "model_name": "htdemucs", "device": "auto"})
        print("split:", state.get("exists"), state.get("stem_names"), state.get("duration"), "seconds taken", state.get("separation", {}).get("seconds_taken"))
        settings = {"vocals": {"mask": True, "regions": [{"start": 0.5, "end": 2.5, "fade_ms": 40}], "db": -3}, "drums": {"mute": True}, "bass": {"db": 2}}
        request = {"project_folder": str(project), "scene_id": scene, "stems": settings}
        mix_app = post(args.app, "/vrgdg/music_builder/audio_mask/render", request)
        app_wav = folder / "masked_mix.wav"
        app_bytes = samples(app_wav)
        shutil.copy(app_wav, folder / "app_mix.wav")
        mix_pack = post(args.pack, "/vrgdg/music_builder/audio_mask/render", request)
        pack_bytes = samples(app_wav)
        print("same settings in the reply:", mix_app["mix"]["stems"] == mix_pack["mix"]["stems"])
        print("wav format:", app_bytes[:3] == pack_bytes[:3], app_bytes[:3])
        import array
        a, b = array.array("h"), array.array("h")
        a.frombytes(app_bytes[3])
        b.frombytes(pack_bytes[3])
        worst = max((abs(x - y) for x, y in zip(a, b)), default=0)
        print("largest sample difference (16 bit steps):", worst, "of", len(a), "samples")
        state_app = post(args.app, "/vrgdg/music_builder/audio_mask/state", {"project_folder": str(project), "scene_id": scene})
        state_pack = post(args.pack, "/vrgdg/music_builder/audio_mask/state", {"project_folder": str(project), "scene_id": scene})
        same_keys = sorted(state_app) == sorted(state_pack)
        lanes_equal = all(max((abs(x - y) for x, y in zip(state_app["peaks"][k], state_pack["peaks"][k])), default=0) < 0.002 and len(state_app["peaks"][k]) == len(state_pack["peaks"][k]) for k in state_pack["peaks"])
        print("state keys equal:", same_keys, "| peak lanes equal:", lanes_equal, "| duration", state_app["duration"], state_pack["duration"])
        return 0 if worst <= 1 and same_keys and lanes_equal else 1
    finally:
        shutil.rmtree(project / "audio_masks", ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
