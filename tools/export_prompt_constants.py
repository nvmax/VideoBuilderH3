"""Dev tool: copy the constants of the pack's LLM prompt modules into ``crates/h3-llm/data/prompt_constants.json``.

    python tools/export_prompt_constants.py --pack <pack folder>

Every module level text, number, list and dict of ``llm/prompts/*.py`` is written under the module's name, so the Rust code
uses the pack's texts byte for byte. Functions are not copied: they are ported.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

MODULES = ["concepts", "emotion_expression", "gemma4", "image", "location_scout", "lyrics", "minimax", "speech", "story_mode", "storyboard", "video"]


def jsonable(value):
    try:
        json.dumps(value)
        return True
    except TypeError:
        return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    data = {}
    for name in MODULES:
        module = importlib.import_module(f"vrgdg_pack.llm.prompts.{name}")
        constants = {}
        for key, value in vars(module).items():
            if key.startswith("__") or callable(value) or isinstance(value, type(sys)):
                continue
            if isinstance(value, (str, int, float, bool, list, dict, tuple)) and jsonable(value):
                constants[key] = value
        data[name] = constants
    out = Path(__file__).resolve().parents[1] / "crates" / "h3-llm" / "data" / "prompt_constants.json"
    out.write_text(json.dumps(data, ensure_ascii=False, indent=1), encoding="utf-8")
    print("prompt_constants.json:", {k: len(v) for k, v in data.items()}, out.stat().st_size, "bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
