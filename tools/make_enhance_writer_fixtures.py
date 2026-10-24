"""Dev tool: golden cases for the MiniMax H3 prompt enhancer (`_enhance_minimax_h3_prompt`), from the pack's own code with the LLM replaced by a recorder.

    python tools/make_enhance_writer_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/enhance_writer.json``. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402
import make_enhance_fixtures as e  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}


def refs(count, picture, **extra):
    items = []
    for number in range(1, count + 1):
        items.append({"label": f"<Subject {number}>", "picture_label": f"<Picture {number}>", "name": f"person {number}", "kind": "subject" if number % 2 else "location",
                      "purpose": "identity reference", "describe": True, "data": picture, **extra})
    return items


def vision_reply(count, prompt=e.GOOD):
    return json.dumps({"prompt": prompt, "reference_descriptions": [{"label": f"<Subject {n}>", "description": f"A person number {n} with distinctive clothing and hair."} for n in range(1, count + 1)]})


def cases(picture):
    original = e.ORIGINAL
    two = e.REFERENCES
    out = [
        ({"current_prompt": original}, [e.GOOD]),
        ({"current_prompt": original, "temperature": 0.5, "top_p": 0.7, "max_new_tokens": 3000}, [e.GOOD]),
        ({"current_prompt": original, "unload_after": False}, ["```\n" + e.GOOD + "\n```"]),
        ({"current_prompt": e.SHOT1}, ["[Shot 1] " + "A close shot of <Subject 1> singing in <Audio 1> from 0.3 s to 3.3 s, <d>[English, soft, singing] A silver <i>fork</i></d>, smiling widely."]),
        ({"current_prompt": original}, ["no shots here"]),
        ({"current_prompt": original}, [e.GOOD.replace("A hundred little", "A thousand little")]),
        ({"current_prompt": "   "}, []),
        ({}, []),
        ({"current_prompt": original, "use_reference_images": False}, [e.GOOD]),
    ]
    out += [
        ({"current_prompt": original, "use_reference_images": True, "image_references": refs(2, picture)}, [vision_reply(2)]),
        ({"current_prompt": original, "use_reference_images": True, "image_references": refs(1, picture), "temperature": 0.2}, [json.dumps({"prompt": e.GOOD, "reference_descriptions": [{"label": "<Subject 1>", "description": "A woman with a red bob and an ivory top."}]})]),
        ({"current_prompt": original, "use_reference_images": True, "image_references": refs(5, picture)}, [vision_reply(5)]),
        ({"current_prompt": original, "use_reference_images": True, "image_references": refs(9, picture)}, [vision_reply(9)]),
        ({"current_prompt": original, "use_reference_images": True, "image_references": refs(2, picture)}, [vision_reply(2).replace("distinctive", "<bad>")]),
        ({"current_prompt": original, "use_reference_images": True, "image_references": refs(2, picture)}, [vision_reply(1)]),
        ({"current_prompt": original, "use_reference_images": True, "image_references": []}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": refs(10, picture)}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": "x"}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": ["x"]}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": [{**two[0], "label": "<Audio 1>"}]}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": [{**r, "data": picture} for r in two] + [{**two[0], "data": picture}]}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": [{**two[0], "path": "refmod://identity/x"}]}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": [{"label": "<Subject 1>", "picture_label": "<Picture 1>", "describe": True}]}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": [{**r, "data": picture, "describe": False} for r in two]}, []),
        ({"current_prompt": original, "use_reference_images": True, "image_references": [{**two[0], "data": "", "path": "Z:/missing.png"}]}, []),
        ({"current_prompt": "", "use_reference_images": True, "image_references": refs(2, picture)}, []),
    ]
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.video_prompt_generation", "llm.minimax_reference_enhance"])
    function = modules["llm.video_prompt_generation"]._enhance_minimax_h3_prompt
    picture = h.png_data_url(400, 300)
    out = []
    for request, replies in cases(picture):
        outcome = h.run_case(recorder, function, {**RUNNER, **request}, replies)
        out.append({"fn": "enhance", "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "enhance_writer.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"enhance_writer.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
