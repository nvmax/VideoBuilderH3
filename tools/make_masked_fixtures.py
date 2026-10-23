"""Dev tool: golden cases for the masked continuation rules, from the pack.

    python tools/make_masked_fixtures.py --pack <pack folder>

Writes ``fixtures/timeline/masked_context.json``.
"""

import argparse
import importlib
import json
import sys
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
    pa = importlib.import_module("vrgdg_pack.minimax.prompt_assembly")

    cases = []
    starts = [None, "", 0, 1.5, "2", "x", 99, -3, 0.04, 2.25, True]
    directions = [None, "", "  pan   left \n slowly "]
    for duration in (0.0, 0.3, 4.0, 5.0, 8.37, 15.0, 30.0):
        for start in starts:
            for direction in directions:
                for shots in (1, 3):
                    for vocals in (False, True):
                        segment = {}
                        if start is not None or direction is None:
                            segment["minimax_h3_continuation_start_seconds"] = start
                        if direction is not None:
                            segment["minimax_h3_continuation_direction"] = direction
                        cases.append({"segment": segment, "duration": duration, "shots": shots, "vocals": vocals,
                                      "return": pa.masked_continuation_context(segment, duration, shots, vocals)})
    (out / "masked_context.json").write_text(json.dumps(cases, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"masked_context.json: {len(cases)} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
