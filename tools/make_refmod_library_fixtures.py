"""Dev tool: golden RefMod library listings (``h3-pipeline::refmod_library``), from the pack.

    python tools/make_refmod_library_fixtures.py --pack <pack folder>

Builds a small RefMod folder, lists it with the pack's ``list_refmods`` and ``find_refmod``. Writes
``fixtures/timeline/refmod_library.json`` with the files (so the Rust test can build the same folder) and the answers.
"""

import argparse
import importlib
import json
import struct
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402


def meta(**fields):
    return {"__metadata__": {"refmod_meta": json.dumps(fields)}}


FILES = {
    "identity/darrel": meta(kind="video", latent_t=4, latent_h=32, latent_w=24, concept_type="character", description="a man", tags=["man", 7], mode="video"),
    "identity/brad": meta(kind="image", latent_t=1, latent_h=40, latent_w=40),
    "background/meadow": meta(kind="image", latent_h=20, latent_w=30, concept_type="", description=None),
    "Style/Film": meta(kind="video", latent_t="3", latent_h="16", latent_w="16"),
    "loose": meta(kind="image", latent_h=8, latent_w=8),
    "audio/voice": {"__metadata__": {"audio_refmod_meta": json.dumps({"kind": "audio", "latent_t": 5})}},
    "identity/broken_meta": {"__metadata__": {"refmod_meta": "not json"}},
    "identity/no_meta": {"x": 1},
    "identity/odd_numbers": meta(kind="video", latent_t=3, latent_h=15, latent_w=9),
    "identity/bad_numbers": meta(kind="video", latent_t="many", latent_h=2, latent_w=2),
    "graph_presets/skipped": meta(kind="image", latent_h=8, latent_w=8),
}
PREVIEWS = ["identity/darrel"]
FIND = ["identity/darrel", "identity\\brad", "loose", "audio/voice", "../outside", "/abs", "C:x", "", "identity/missing", " background/meadow "]


def write(root: Path, name: str, header: dict) -> None:
    path = root / (name + ".safetensors")
    path.parent.mkdir(parents=True, exist_ok=True)
    blob = json.dumps(header).encode("utf-8")
    path.write_bytes(struct.pack("<Q", len(blob)) + blob)


def public(entry):
    return {k: v for k, v in entry.items() if k not in ("path", "directory")}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "timeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    lib = importlib.import_module("vrgdg_pack.minimax.refmod_library")
    picker = importlib.import_module("vrgdg_pack.minimax.refmod_picker")

    with tempfile.TemporaryDirectory() as tmp:
        first, second = Path(tmp) / "one", Path(tmp) / "two"
        for name, header in FILES.items():
            write(first, name, header)
        write(second, "identity/darrel", meta(kind="image", latent_h=2, latent_w=2))  # a repeat: the first folder wins
        write(second, "extra/more", meta(kind="image", latent_h=4, latent_w=4))
        for name in PREVIEWS:
            (first / (name + ".preview.png")).write_bytes(b"png")
        lib._refmod_dirs = lambda: [str(first), str(second), str(Path(tmp) / "missing")]
        picker._refmod_dirs = lib._refmod_dirs
        answers = {
            "list": [public(e) for e in lib.list_refmods("")],
            "folders": {f: [public(e) for e in lib.list_refmods(f)] for f in ("identity", "extra", "nope", "Style")},
            "find": {name: (public(e) if (e := lib.find_refmod(name)) else None) for name in FIND},
        }
    data = {"files": FILES, "previews": PREVIEWS, "find": FIND, "answers": answers}
    (out / "refmod_library.json").write_text(json.dumps(data, indent=1, ensure_ascii=False), encoding="utf-8")
    print("listed:", len(answers["list"]), [e["name"] for e in answers["list"]])
    return 0


if __name__ == "__main__":
    sys.exit(main())
