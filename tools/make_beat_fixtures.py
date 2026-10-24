"""Dev tool: golden cases for the beat tracker (`beats.rs`), from the pack's own `_estimate_beats_from_audio` (librosa).

    python tools/make_beat_fixtures.py --pack <pack folder>

Writes ``fixtures/audio/beats.json`` and the synthetic songs it names, ``fixtures/audio/beat_*.wav`` (22,050 Hz, mono, 16 bit,
so the pack's `librosa.load` and the Rust test read exactly the same samples). The songs are made here from a seeded noise
generator: drums with kicks and hi-hats, a swung groove over a held chord, a silent intro, and a tempo change.
"""

import argparse
import importlib
import json
import sys
import wave
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

SR = 22050


def decaying(length, tone, decay, noise=0.0, rng=None):
    t = np.arange(length) / SR
    wave_ = np.sin(2 * np.pi * tone * t) * np.exp(-t * decay)
    if noise and rng is not None:
        wave_ += noise * rng.standard_normal(length) * np.exp(-t * decay)
    return wave_


def place(track, start_seconds, sound, gain=1.0):
    start = int(start_seconds * SR)
    end = min(len(track), start + len(sound))
    if start < len(track):
        track[start:end] += gain * sound[: end - start]


def groove(seconds, bpm, seed, swing=0.0, offset=0.0, hats=True, pad=False, accent=0.0):
    rng = np.random.RandomState(seed)
    track = np.zeros(int(seconds * SR))
    beat = 60.0 / bpm
    kick = decaying(int(0.25 * SR), 55, 18, 0.05, rng)
    snare = decaying(int(0.18 * SR), 190, 22, 0.6, rng)
    hat = decaying(int(0.05 * SR), 7000, 90, 0.8, rng)
    at, n = offset, 0
    while at < seconds - 0.3:
        strong = 1.0 + accent * (1.0 if n % 4 == 0 else 0.0)
        place(track, at, kick, 0.9 * strong)
        if n % 2 == 1:
            place(track, at, snare, 0.6)
        if hats:
            place(track, at, hat, 0.25)
            place(track, at + beat * (0.5 + swing), hat, 0.18)
        at += beat
        n += 1
    if pad:
        t = np.arange(len(track)) / SR
        for freq in (220.0, 277.18, 329.63):
            track += 0.06 * np.sin(2 * np.pi * freq * t)
    return track


def tempo_change(seconds, first_bpm, second_bpm, switch, seed):
    rng = np.random.RandomState(seed)
    track = np.zeros(int(seconds * SR))
    kick = decaying(int(0.25 * SR), 58, 18, 0.05, rng)
    hat = decaying(int(0.05 * SR), 6500, 90, 0.8, rng)
    at = 0.2
    while at < seconds - 0.3:
        place(track, at, kick, 0.9)
        place(track, at + 0.25, hat, 0.2)
        at += 60.0 / (first_bpm if at < switch else second_bpm)
    return track


def write_wav(path, samples):
    peak = max(1e-9, float(np.max(np.abs(samples))))
    data = np.clip(samples / peak * 0.8, -1, 1)
    pcm = (data * 32767).astype("<i2")
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(SR)
        handle.writeframes(pcm.tobytes())


def songs():
    silent_intro = groove(16, 140, 3, hats=True, offset=2.4)
    return {
        "beat_drums_124.wav": groove(14, 124, 1),
        "beat_swing_96_pad.wav": groove(16, 96, 2, swing=0.12, pad=True, accent=0.5),
        "beat_late_start_140.wav": silent_intro,
        "beat_tempo_change.wav": tempo_change(16, 100, 112, 8.0, 4),
        "beat_sparse_72.wav": groove(18, 72, 5, hats=False, accent=0.8),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    audio = importlib.import_module("vrgdg_pack.builder.audio")
    import librosa

    target = Path(__file__).resolve().parents[1] / "fixtures" / "audio"
    target.mkdir(parents=True, exist_ok=True)
    cases = []
    for name, samples in songs().items():
        path = target / name
        write_wav(path, samples)
        duration = len(samples) / SR
        beats, bpm = audio._estimate_beats_from_audio(str(path), [0.0] * 64, duration, include_tempo=True)
        waveform, _ = librosa.load(str(path), sr=SR, mono=True)
        onset = librosa.onset.onset_strength(y=waveform, sr=SR)
        cases.append({
            "file": name, "duration": duration, "beats": beats, "tempo_bpm": bpm,
            "onset_length": int(len(onset)), "onset_sum": float(np.sum(onset)), "onset_head": [float(x) for x in onset[:40]],
            "onset_sample": [float(onset[i]) for i in range(0, len(onset), 37)],
        })
        print(f"{name}: {len(beats)} beats, {bpm} BPM, onset frames {len(onset)}")
    (target / "beats.json").write_text(json.dumps({"cases": cases}, separators=(",", ":")), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
