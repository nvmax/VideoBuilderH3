"""Dev tool: copy the LLM API provider and model lists of the pack into ``crates/h3-llm/data/llm_api_models.json``.

    python tools/export_llm_data.py --pack <pack folder>

The lists are data in ``llm/api.py`` (``VRGDG_LLM_Multi.PROVIDER_MODELS`` and ``DEFAULT_MODEL``). They are read by running the
pack's own module, so what the Builder offers is what the pack offers.
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
    api = importlib.import_module("vrgdg_pack.llm.api").VRGDG_LLM_Multi
    out = Path(__file__).resolve().parents[1] / "crates" / "h3-llm" / "data" / "llm_api_models.json"
    data = {"providers": api.PROVIDER_MODELS, "default_model": api.DEFAULT_MODEL}
    out.write_text(json.dumps(data, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"llm_api_models.json: {', '.join(f'{k} {len(v)}' for k, v in api.PROVIDER_MODELS.items())}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
