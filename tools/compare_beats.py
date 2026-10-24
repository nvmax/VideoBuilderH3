"""Dev tool: compare the app's beat markers for a real song with what the pack (librosa) finds for the same file.

    python tools/compare_beats.py --pack <pack folder> --audio <song> [--api http://127.0.0.1:8190]

The app must be running (or `h3 serve`). Prints counts and agreement only, no song content. A beat of the app agrees with the
pack's when one of the pack's lies within `--within` seconds (default one frame, 23 ms).
"""

import argparse
import bisect
import importlib
import json
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402


def stats(name, beats, tempo):
    gaps = sorted(round(b - a, 3) for a, b in zip(beats, beats[1:]))
    print(f"{name:26} beats {len(beats):4} | tempo {tempo:8.3f} | gap shortest {gaps[0]:6} median {gaps[len(gaps) // 2]:6} longest {gaps[-1]:7} | first {beats[0]:.3f}s last {beats[-1]:.3f}s")


def agreement(mine, theirs, within):
    ordered = sorted(theirs)

    def near(t):
        i = bisect.bisect_left(ordered, t)
        return min(abs(t - ordered[j]) for j in (i - 1, i) if 0 <= j < len(ordered)) <= within

    return sum(1 for t in mine if near(t))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--audio", required=True, type=Path)
    parser.add_argument("--api", default="http://127.0.0.1:8190")
    parser.add_argument("--within", type=float, default=0.024)
    args = parser.parse_args()
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    audio = importlib.import_module("vrgdg_pack.builder.audio")
    path = str(args.audio.resolve())
    peaks = audio._read_audio_peaks(path, 1800)
    theirs, their_tempo = audio._estimate_beats_from_audio(path, peaks.get("peaks", []), peaks.get("duration", 0), include_tempo=True)
    request = urllib.request.Request(args.api.rstrip("/") + "/vrgdg/music_builder/analyze_audio", data=json.dumps({"audio_path": path, "target_peaks": 1800}).encode(), headers={"Content-Type": "application/json"})
    mine = json.load(urllib.request.urlopen(request, timeout=300))
    stats("the pack (librosa)", theirs, their_tempo)
    stats("the app", mine["beats"], mine["tempo_bpm"])
    same = agreement(mine["beats"], theirs, args.within)
    back = agreement(theirs, mine["beats"], args.within)
    print(f"app beats the pack also has within {args.within * 1000:.0f} ms: {same} of {len(mine['beats'])} | pack beats the app also has: {back} of {len(theirs)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
