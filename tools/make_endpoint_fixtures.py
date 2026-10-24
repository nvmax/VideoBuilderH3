"""Dev tool: the pack's endpoint list, for the route coverage test (``h3-api/tests/route_coverage.rs``).

    python tools/make_endpoint_fixtures.py --pack <pack folder>

Reads the pack's ``agent_api/endpoints.json`` and writes ``fixtures/api/pack_endpoints.json`` with method and path only.
"""

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    source = json.loads((args.pack / "agent_api" / "endpoints.json").read_text(encoding="utf-8"))
    rows = sorted({(e["method"], e["path"]) for e in source["endpoints"]})
    out = Path(__file__).resolve().parents[1] / "fixtures" / "api"
    out.mkdir(exist_ok=True)
    (out / "pack_endpoints.json").write_text(json.dumps([{"method": m, "path": p} for m, p in rows], indent=1), encoding="utf-8")
    print(f"pack_endpoints.json: {len(rows)} endpoints")
    return 0


if __name__ == "__main__":
    sys.exit(main())
