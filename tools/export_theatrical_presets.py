"""Dev tool: copy the pack's Theatrical presets into ``crates/h3-core/data/theatrical_presets.json``.

    python tools/export_theatrical_presets.py --pack <pack folder>

The pack generates ``storyboard/theatrical_presets.json`` from the Builder's own preset files (``scripts/export_theatrical_presets.mjs``), so
the texts the Agent API writes into a prompt are the ones the browser shows. The file is copied byte for byte (line ends as the pack's tests read them).
"""

import argparse
import json
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    source = args.pack / "storyboard" / "theatrical_presets.json"
    text = source.read_text(encoding="utf-8").replace("\r\n", "\n")
    data = json.loads(text)
    out = Path(__file__).resolve().parents[1] / "crates" / "h3-core" / "data" / "theatrical_presets.json"
    out.write_text(text, encoding="utf-8", newline="\n")
    print(f"theatrical_presets.json: camera {data['camera_key']}, {len(data['camera']['sequence'])} shots, {len(text)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
