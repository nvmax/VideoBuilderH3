"""Dev tool: golden peaks, beats and tempo for ``h3-media::peaks``, made by the pack's own functions.

    python tools/make_peaks_fixtures.py --pack <pack folder>

Writes ``fixtures/media/pulses.wav`` (16 kHz mono, so ffmpeg does not resample it) and ``fixtures/media/peaks_cases.json``.
"""

import argparse
import importlib
import json
import math
import struct
import sys
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "media")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    audio = importlib.import_module("vrgdg_pack.builder.audio")

    rate, seconds = 16000, 6
    wav_path = args.out / "pulses.wav"
    frames = bytearray()
    for n in range(rate * seconds):
        t = n / rate
        beat_phase = (t % 0.5)
        envelope = math.exp(-beat_phase * 14.0)
        value = int(20000 * envelope * math.sin(2 * math.pi * 220 * t) * (1.0 if int(t / 0.5) % 4 else 1.4)) // 2
        frames += struct.pack("<h", max(-32767, min(32767, value)))
    with wave.open(str(wav_path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(rate)
        handle.writeframes(bytes(frames))

    cases = []
    for target in (400, 100, 7, 100000):
        info = audio._read_audio_peaks_with_ffmpeg(str(wav_path), target)
        beats = audio._estimate_beats_from_peaks(info["peaks"], info["duration"])
        cases.append({"target": target, "duration": info["duration"], "peaks": info["peaks"], "beats": beats, "tempo": audio._tempo_from_beat_times(beats)})
    (args.out / "peaks_cases.json").write_text(json.dumps({"cases": cases}), encoding="utf-8")
    print("peaks cases:", [(c["target"], len(c["peaks"]), len(c["beats"]), c["tempo"]) for c in cases])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
