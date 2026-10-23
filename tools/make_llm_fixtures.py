"""Dev tool: golden reply cleaning for ``h3-llm`` and the MiniMax instruction text, from the pack.

    python tools/make_llm_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/clean.json`` and the instruction texts under ``crates/h3-llm/data/``.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

SAMPLES = [
    "plain answer",
    "  spaced \r\n answer \r ",
    "<think>hidden</think>The answer.",
    "<thought>one</thought> A <think>two</think> B",
    "reasoning without opener</think>Real answer",
    "Thinking: step one\nstep two\nFinal answer: Done.",
    "Thought process: stuff\nAnswer: 42",
    "Analysis: just a label then text",
    "<|channel|>thought: tail",
    "```json\n{\"a\": 1}\n```",
    "```text\nhello\n```",
    "Assistant: hi there",
    "Final answer: yes",
    "Answer:   spaced",
    "<think>unclosed",
    "",
    "Reasoning: x\n\nresponse: y\nsecond line",
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    rs = Path(__file__).resolve().parents[1]
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    runner = importlib.import_module("vrgdg_pack.llm.builder_runner")
    cases = [{"input": s, "strip": runner._strip_builder_thinking_text(s), "plain": runner._clean_lm_studio_plain_text(s)} for s in SAMPLES]
    out = rs / "fixtures" / "llm"
    out.mkdir(parents=True, exist_ok=True)
    (out / "clean.json").write_text(json.dumps({"cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print("clean cases:", len(cases))
    minimax = importlib.import_module("vrgdg_pack.llm.prompts.minimax")
    data = rs / "crates" / "h3-llm" / "data"
    data.mkdir(parents=True, exist_ok=True)
    (data / "minimax_h3_reference_to_video.txt").write_text(minimax.MINIMAX_H3_REFERENCE_TO_VIDEO_INSTRUCTIONS, encoding="utf-8", newline="")
    (data / "minimax_h3_frame_continuity.txt").write_text(minimax.MINIMAX_H3_FRAME_CONTINUITY_INSTRUCTIONS, encoding="utf-8", newline="")
    print("instruction chars:", len(minimax.MINIMAX_H3_REFERENCE_TO_VIDEO_INSTRUCTIONS), len(minimax.MINIMAX_H3_FRAME_CONTINUITY_INSTRUCTIONS))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
