"""Dev tool: golden graphs for the lyric timing workflow, from the pack.

    python tools/make_timestamp_graph_fixtures.py --pack <pack folder>

Writes ``fixtures/timeline/timestamp_graph.json``. The song path is a temp file, recorded as "<AUDIO>".
"""

import argparse
import importlib
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "timeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    uw = importlib.import_module("vrgdg_pack.runner.utility_workflows")

    with tempfile.TemporaryDirectory() as tmp:
        audio = Path(tmp) / "song.wav"
        audio.write_bytes(b"RIFF")
        real = str(audio.resolve())
        requests = [
            {},
            {"reference_lyrics": "line one\nline two", "language": "spanish", "segment_mode": "whisper_chunks"},
            {"segment_mode": "reference_scene_words", "include_instrumental_gaps": False, "instrumental_text": "[break]"},
            {"segment_mode": "nonsense", "min_gap_seconds": 99, "min_scene_seconds": 0.1, "max_scene_seconds": 500, "vocal_tail_padding_seconds": -4},
            {"model_name": "  medium ", "min_gap_seconds": "3.5", "language": "", "instrumental_text": ""},
            {"audio_path": str(Path(tmp) / "missing.wav")},
        ]
        cases = []
        for request in requests:
            payload = {"audio_path": real, **request}
            try:
                graph = uw._build_timestamped_transcribe_api_prompt(payload)["prompt"]
                text = json.dumps(graph).replace(json.dumps(real)[1:-1], "<AUDIO>")
                result = {"prompt": json.loads(text)}
            except Exception as exc:  # noqa: BLE001
                message = str(exc).replace(real, "<AUDIO>").replace(str(Path(tmp)), "<TMP>")
                result = {"error": message}
            shown = dict(request)
            if shown.get("audio_path", "").startswith(tmp):
                shown["audio_path"] = "<MISSING>"
            cases.append({"request": shown, "return": result})
    (out / "timestamp_graph.json").write_text(json.dumps(cases, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"timestamp_graph.json: {len(cases)} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
