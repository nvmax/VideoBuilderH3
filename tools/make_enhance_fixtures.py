"""Dev tool: golden cases for MiniMax H3 Enhance Prompts (`minimax/prompt_enhance.py`), from the pack's own functions.

    python tools/make_enhance_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/enhance.json``: each call with its arguments and what the pack returned or the error it raised.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

HEAD = ("subject_definitions:\n<Subject 1> is the singer from the first character reference.\n\n"
        "<Audio 1> is the complete song.\n\ndetailed_description:\n"
        "The target video is in a photorealistic cinematic music-video music-video style.\n\n")
TAIL = "\n\noverall_soundscape:\n<Audio 1> remains the sole soundtrack."
SHOT1 = ("[Shot 1] A medium shot of <Subject 1> at the table as she sings in sync with the vocals in <Audio 1> "
         "from 0.3 s to 3.3 s, <d>[English, soft, singing] A silver <i>fork</i></d>, smiling.")
SHOT2 = ("[Shot 2] At 00:03.450, continuing directly from the previous shot's final moment, <Subject 1> still at "
         "the table sings in sync with the vocals in <Audio 1> from 3.6 s to 4.8 s, <d>[English, singing] A hundred "
         "little</d>, looking down.")
SHOT3 = "[Shot 3] At 00:05.000, <Subject 1> turns toward the window and says \"Not tonight\" in <Audio 1> from 5.0 s to 6.0 s."
ORIGINAL = HEAD + SHOT1 + "\n\n" + SHOT2 + TAIL
GOOD = ("[Shot 1] A wide-angle close-up of <Subject 1> at the table, her face large in the frame, wagging a "
        "fork as she sings in sync with the vocals in <Audio 1> from 0.3 s to 3.3 s, <d>[English, cheeky, "
        "playful, singing] A silver <i>fork</i></d>, one brow lifting.\n\n"
        "[Shot 2] At 00:03.450, continuing directly from the previous shot's final moment, <Subject 1> still "
        "at the table, the camera cuts to a low side angle as she sings in sync with the vocals in <Audio 1> "
        "from 3.6 s to 4.8 s, <d>[English, mocking, sly, singing] A hundred little</d>, then holds a sly grin.")
REFERENCES = [
    {"label": "<Subject 1>", "picture_label": "<Picture 1>", "name": "the singer", "kind": "subject", "purpose": "character identity and wardrobe reference", "describe": True, "path": "singer.png"},
    {"label": "<Subject 2>", "picture_label": "<Picture 2>", "name": "the environment", "kind": "location", "purpose": "environment reference", "describe": True, "path": "room.png"},
]
VISION = {"prompt": GOOD, "reference_descriptions": [
    {"label": "<Subject 1>", "description": "A woman with a red bob, an ivory ruffled top and a black pleated skirt."},
    {"label": "<Subject 2>", "description": "A wood-paneled dining room with a long table and warm overhead lights."},
]}


def enhance_cases():
    originals = [
        ORIGINAL, SHOT1 + "\n\n" + SHOT2, SHOT1, HEAD + SHOT1 + TAIL, HEAD + SHOT1 + "\n\n" + SHOT2 + "\n\n" + SHOT3 + TAIL, "no shots here", HEAD + "text [Shot 1] inline only",
        ORIGINAL.replace("\n", "\r\n"), HEAD + "  " + SHOT1 + "\n\n   " + SHOT2 + "\n\nretention:\nkeep it all.",
    ]
    replies = [
        GOOD, "```text\n" + GOOD + "\n```", GOOD.replace("[Shot 1]", "**[Shot 1]**").replace("[Shot 2]", "**[Shot 2]**"), "Here is the prompt:\n\n" + GOOD, "no shot blocks", "",
        GOOD.replace("[Shot 2]", "[Shot 3]"), GOOD + "\n\n[Shot 3] Another one that is long enough to count.", GOOD.replace("A hundred little", "A thousand little"),
        GOOD.replace("from 3.6 s", "from 3.5 s"), GOOD.replace("<Subject 1> still at", "the woman still at"), GOOD.replace("<Audio 1> from 3.6", "<Audio 2> from 3.6"),
        GOOD.replace("then holds a sly grin.", "then holds a sly grin and"), GOOD.replace("[Shot 2] At 00:03.450, continuing directly from the previous shot's final moment, ", "[Shot 2] "),
        "[Shot 1] Short.\n\n[Shot 2] Short.", GOOD.replace("At 00:03.450, ", ""), "[Shot 1] " + "x" * 8000 + ".\n\n[Shot 2] At 00:03.450, ok " + "y" * 30 + ".",
        GOOD.replace("then holds a sly grin.", 'then says "A silver fork" softly.'), "[Shot 1] A medium shot of <Subject 1> at the table as she sings in sync with the vocals in <Audio 1> from 0.3 s to 3.3 s, <d>[English, soft, singing] A silver <i>fork</i></d>, smiling widely.",
        GOOD.replace("then holds a sly grin.", "then holds a sly grin…"), GOOD.replace("then holds a sly grin.", 'then holds a sly grin”'),
    ]
    out = []
    for original in originals:
        for reply in replies:
            out.append((original, reply))
    third_reply = "\n\n".join([
        "[Shot 1] Close on <Subject 1> at the table, singing in <Audio 1> from 0.3 s to 3.3 s, <d>[English, soft, singing] A silver <i>fork</i></d>, smiling.",
        "[Shot 2] At 00:03.450, <Subject 1> sings in <Audio 1> from 3.6 s to 4.8 s, <d>[English, singing] A hundred little</d>, looking down at the table.",
        '[Shot 3] At 00:05.000, <Subject 1> turns to the window and says "Not tonight" in <Audio 1> from 5.0 s to 6.0 s, her jaw set.'])
    out.append((HEAD + SHOT1 + "\n\n" + SHOT2 + "\n\n" + SHOT3 + TAIL, third_reply))
    out.append((HEAD + SHOT1 + "\n\n" + SHOT2 + "\n\n" + SHOT3 + TAIL, third_reply.replace("Not tonight", "Not today")))
    return out


def reference_cases():
    out = []
    originals = [ORIGINAL, SHOT1 + "\n\n" + SHOT2, HEAD + SHOT1 + "\n\n" + SHOT2 + "\n\nretention:\nkept."]
    anchors = REFERENCES + [{"label": "<Subject 3>", "picture_label": "<Picture 3>", "kind": "start_frame", "purpose": "first frame", "describe": False},
                            {"label": "<Subject 4>", "picture_label": "<Picture 4>", "kind": "end_frame", "purpose": "last frame", "describe": False}]
    for original in originals:
        for references, reply in ((REFERENCES, VISION), (anchors, VISION)):
            out.append((original, reply, references))
    for change in ("missing", "duplicate", "unknown", "injection", "too long", "too short", "not a string", "not a list", "no prompt", "newline"):
        reply = copy.deepcopy(VISION)
        descriptions = reply["reference_descriptions"]
        if change == "missing":
            descriptions.pop()
        elif change == "duplicate":
            descriptions[1]["label"] = "<Subject 1>"
        elif change == "unknown":
            descriptions[1]["label"] = "<Subject 9>"
        elif change == "injection":
            descriptions[0]["description"] = "A woman <Subject 5> in a coat."
        elif change == "too long":
            descriptions[0]["description"] = "x" * 460
        elif change == "too short":
            descriptions[0]["description"] = "tiny"
        elif change == "not a string":
            descriptions[0]["description"] = 5
        elif change == "not a list":
            reply["reference_descriptions"] = "none"
        elif change == "no prompt":
            reply.pop("prompt")
        elif change == "newline":
            descriptions[0]["description"] = "A woman\nwith red hair and a long coat."
        out.append((ORIGINAL, reply, REFERENCES))
    out.append((ORIGINAL, "text", REFERENCES))
    out.append((ORIGINAL, {**VISION, "reference_descriptions": [VISION["reference_descriptions"][0], 4]}, REFERENCES))
    out.append((ORIGINAL, VISION, [{**REFERENCES[0], "name": "the <singer>\n", "purpose": ""}, REFERENCES[1]]))
    out.append((ORIGINAL, {**VISION, "prompt": GOOD.replace("A hundred little", "A thousand little")}, REFERENCES))
    out.append((HEAD.replace("<Subject 1> is the singer from the first character reference.", "<Subject 1> is old text.\n<Subject 2> is old location text.") + SHOT1 + "\n\n" + SHOT2 + TAIL, VISION, REFERENCES))
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    pe = importlib.import_module("vrgdg_pack.minimax.prompt_enhance")
    out = []
    for original, reply in enhance_cases():
        try:
            out.append({"fn": "merge", "args": [original, reply], "return": pe.merge_enhanced_prompt(original, reply)})
        except pe.PromptEnhanceError as exc:
            out.append({"fn": "merge", "args": [original, reply], "error": str(exc)})
    for original, reply, references in reference_cases():
        try:
            out.append({"fn": "reference", "args": [original, reply, references], "return": pe.merge_reference_enhanced_prompt(original, reply, references)})
        except pe.PromptEnhanceError as exc:
            out.append({"fn": "reference", "args": [original, reply, references], "error": str(exc)})
    for text in (ORIGINAL, SHOT1, "none", HEAD + SHOT1 + "\n\nretention:\nx\n\n" + "tail:\ny"):
        try:
            out.append({"fn": "split", "args": [text], "return": list(pe.split_prompt(text))})
        except pe.PromptEnhanceError as exc:
            out.append({"fn": "split", "args": [text], "error": str(exc)})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "enhance.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"enhance.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
