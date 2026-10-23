"""Dev tool: golden tables for scenes from timed lyric lines, from the pack.

    python tools/make_timed_lines_fixtures.py --pack <pack folder>

Writes ``fixtures/timeline/timed_lines.json``. Scene ids are random in the pack, so they are recorded as "ID".
"""

import argparse
import importlib
import json
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402


def words_for(text: str, start: float, end: float) -> list:
    parts = text.split()
    step = (end - start) / max(1, len(parts))
    return [{"text": w, "start": round(start + i * step, 3), "end": round(start + (i + 1) * step - 0.02, 3)} for i, w in enumerate(parts)]


def line(text: str, start: float, end: float, kind: str = "vocal", with_words: bool = True) -> dict:
    item = {"text": text, "start": start, "end": end, "type": kind}
    if with_words:
        item["words"] = words_for(text, start, end)
    return item


REFERENCE = "[Verse 1]\nWalking down the empty street\nLights are fading under my feet\n\n[Chorus]\nSing it loud tonight\nHold the morning light\n\n[Bridge]\nQuiet now\n[End]"

PAYLOADS = {
    "plain": {
        "duration": 40.0,
        "segments": [
            line("Walking down the empty street", 2.0, 6.0),
            line("Lights are fading under my feet", 6.2, 10.0),
            line("Sing it loud tonight", 14.0, 18.5),
            line("Hold the morning light", 18.5, 23.0),
            line("Quiet now", 30.0, 32.0),
        ],
    },
    "long_chunk": {
        "duration": 30.0,
        "segments": [line("one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen", 1.0, 25.0)],
    },
    "long_no_words": {"duration": 26.0, "segments": [line("a very long line without word timing at all", 0.0, 23.0, with_words=False)]},
    "tiny_lines": {
        "duration": 12.0,
        "segments": [line("Hey", 0.5, 1.0), line("Oh", 1.2, 1.5), line("Walking down the empty street", 2.0, 6.0), line("Yeah", 6.1, 6.4), line("Lights are fading under my feet", 8.0, 12.0)],
    },
    "unsorted_overlap": {
        "duration": 20.0,
        "segments": [line("Sing it loud tonight", 8.0, 12.0), line("Walking down the empty street", 1.0, 5.0), line("Hold the morning light", 11.0, 16.0), {"text": "bad", "start": 3.0, "end": 3.0}, "junk"],
    },
    "instrumental_and_brackets": {
        "duration": 25.0,
        "segments": [line("[Verse 1] Walking down the empty street", 0.0, 5.0), line("", 5.0, 9.0, "instrumental", False), line("[Instrumental]", 9.0, 12.0, with_words=False), line("Sing it loud tonight", 12.0, 17.0)],
    },
    "bundled": {"duration": 12.0, "segments": [line("Walking down the empty street Lights are fading under my feet", 0.0, 6.0), line("Sing it loud tonight", 6.0, 12.0)]},
    "empty": {"duration": 10.0, "segments": []},
    "only_duration": {"segments": [line("Walking down the empty street", 0.0, 4.0)]},
}


def mask(scenes):
    return [{**s, "id": "ID"} for s in scenes]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "timeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    lo = importlib.import_module("vrgdg_pack.agent_api.orchestrator.lyrics_orchestrator")
    bl = importlib.import_module("vrgdg_pack.builder.lyric_scenes")

    cases = []
    modes = ("whisper_chunks", "reference_lines", "exact_reference_lines", "reference_stanzas")
    limits = ((1.0, 8.0), (3.5, 10.0), (2.0, 5.0))
    for name, payload in PAYLOADS.items():
        for mode in modes:
            for enforce in (True, False):
                for min_s, max_s in limits:
                    for gaps in (True, False):
                        params = {"segment_mode": mode, "instrumental_text": "[instrumental]", "min_scene_seconds": min_s, "max_scene_seconds": max_s, "min_gap_seconds": 2.0, "include_instrumental_gaps": gaps, "enforce_lengths": enforce}
                        try:
                            result = {"scenes": mask(lo.build_scenes_from_payload(json.loads(json.dumps(payload)), params, REFERENCE))}
                        except Exception as exc:  # noqa: BLE001
                            result = {"error": str(exc)}
                        cases.append({"op": "build", "payload": name, "params": params, "return": result})
    # a custom instrumental marker and no reference lyrics
    params = {"segment_mode": "whisper_chunks", "instrumental_text": "[no vocals]", "min_scene_seconds": 3.0, "max_scene_seconds": 8.0, "min_gap_seconds": 1.0, "include_instrumental_gaps": True, "enforce_lengths": True}
    cases.append({"op": "build", "payload": "plain", "params": params, "reference": "", "return": {"scenes": mask(lo.build_scenes_from_payload(json.loads(json.dumps(PAYLOADS["plain"])), params, ""))}})

    texts = ["", "  hello   world ", "[Verse 1] Walking , down", "[Instrumental] la la", "[Chorus]", "a ( b ) c [ d ]", "[x] short tag", "Word [Break] more", "tab\tand\nnewline"]
    for value in texts:
        cases.append({"op": "clean", "text": value, "return": bl.clean_timestamped_lyric_text(value)})

    rng = random.Random(3)
    outputs = ['{"segments": []}', 'noise {"segments": [{"start": 0}], "duration": 3} trailing', "no json here", '{"other": 1}', '[1, 2]', '  {"segments": [], "x": 1}  ']
    for value in outputs:
        try:
            result = {"value": lo.parse_timestamped_lyrics_output(value)}
        except Exception as exc:  # noqa: BLE001
            result = {"error": str(exc)}
        cases.append({"op": "parse", "text": value, "return": result})
    _ = rng

    (out / "timed_lines.json").write_text(json.dumps(cases, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"timed_lines.json: {len(cases)} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
