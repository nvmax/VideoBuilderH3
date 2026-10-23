"""Dev tool: save the /object_info entries the MiniMax H3 graphs use, from a running ComfyUI.

The pack's graph builders ask ComfyUI which inputs each node class has. The fixture generator replays that
answer from this snapshot, so no ComfyUI or torch import is needed. Run it again when node packs change.

    python tools/capture_object_info.py --pack <pack folder> [--url http://127.0.0.1:8188]
"""

import argparse
import json
import urllib.request
from pathlib import Path

# Nodes the patchers add to the templates (runner/minimax_patches.py, minimax_inputs.py, minimax_refmod.py).
PATCHER_NODES = [
    "ComfyMathExpression", "H3FastVAEDecode", "LoraLoaderModelOnly", "LTXVAudioVAEDecode",
    "MiniMaxH3MemoryEfficientSageAttentionPatch", "MiniMaxH3RefModAudioExtract", "MiniMaxH3RefModsLoader",
    "MiniMaxH3RefModTextEncode", "MMH3SpatialSplitParams", "MMH3TemporalSplitParams", "MMH3UltimateUpscale",
    "ResolutionSelector", "TESpeedMiniMaxH3", "VAEDecode", "VRGDG_MiniMaxH3ApplyMaskedContinuation",
    "VRGDG_MiniMaxH3KeyframeTiming", "VRGDG_MiniMaxH3LoadDraft", "VRGDG_MiniMaxH3LoadLatent",
    "VRGDG_MiniMaxH3SaveDraft", "VRGDG_MiniMaxH3SaveLatent", "VRGDG_MiniMaxH3SpatialTilePlan",
    "VRGDG_MiniMaxH3UltimateUpscaleParams", "VRGDG_RefModCombine", "LoadImage", "ImageResizeKJv2",
    "MiniMaxChunkFeedForward", "BlockSparseAttention",
]


def template_classes(pack: Path) -> set:
    classes = set()
    for path in (pack / "Workflows" / "UsedForUIDoNotTouch").glob("minimax_*.json"):
        data = json.loads(path.read_text(encoding="utf-8"))
        nodes = data.get("nodes")
        if isinstance(nodes, list):  # UI-format workflow
            classes.update(str(n.get("type")) for n in nodes if n.get("type"))
        else:  # API format
            classes.update(str(n.get("class_type")) for n in data.values() if isinstance(n, dict) and n.get("class_type"))
    return classes


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--url", default="http://127.0.0.1:8188")
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "object_info_h3.json")
    args = parser.parse_args()

    wanted = template_classes(args.pack) | set(PATCHER_NODES)
    with urllib.request.urlopen(args.url.rstrip("/") + "/object_info", timeout=120) as resp:
        info = json.load(resp)
    kept = {name: info[name] for name in sorted(wanted) if name in info}
    missing = sorted(wanted - set(kept))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(kept, indent=1, sort_keys=True), encoding="utf-8")
    print(f"saved {len(kept)} node classes to {args.out}")
    if missing:
        print("not installed in this ComfyUI:", ", ".join(missing))


if __name__ == "__main__":
    main()
