"""Dev tool: copy the Builder's default LLM instructions into ``crates/h3-llm/data/instructions.json``.

    python tools/export_instructions.py --pack <pack folder>

Reads ``_BUILDER_INSTRUCTION_DEFAULTS``, the labels and the preset groups of ``llm/builder_instructions.py`` by running the
pack's own module, so the texts are the pack's texts byte for byte.
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
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    module = importlib.import_module("vrgdg_pack.llm.builder_instructions")
    keys = {}
    for key, text in module._BUILDER_INSTRUCTION_DEFAULTS.items():
        keys[key] = {
            "label": module._BUILDER_INSTRUCTION_LABELS.get(key, key),
            "default": text,
            "preset_group": module._BUILDER_INSTRUCTION_PRESET_GROUPS.get(key, key),
        }
    data = {"keys": keys, "preset_group_labels": module._BUILDER_INSTRUCTION_PRESET_GROUP_LABELS}
    out = Path(__file__).resolve().parents[1] / "crates" / "h3-llm" / "data" / "instructions.json"
    out.write_text(json.dumps(data, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"instructions.json: {len(keys)} instructions, {out.stat().st_size} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
