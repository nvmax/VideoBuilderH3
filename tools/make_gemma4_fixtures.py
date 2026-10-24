"""Dev tool: golden cases for the Gemma 4 drafts (`_run_gemma4_prompt`), from the pack's own code with the LLM replaced by a recorder.

    python tools/make_gemma4_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/gemma4.json``. The built-in runner is left out: the pack loads a GGUF file for it, the app uses ComfyUI's Gemma.
See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
DRAFT = "A moody, rain soaked story about a singer who walks home through a neon city.\n\nShe finds a lit window and stops."
FILES = {
    "style_theme.txt": "﻿Custom style instructions.\n",
    "story_idea.txt": "   \n",
    "location_description_detail.txt": "Custom location detail instructions.",
}


def cases():
    pf = {"project_folder": "<TMP>"}
    out = [
        ({"target": "builder_style_theme", "notes": "neon noir"}, [DRAFT]),
        ({"target": "builder_style_theme", "lyrics": "la la", "style_theme": "old style"}, [DRAFT]),
        ({"target": "builder_style_theme", "story_idea": "a night walk"}, [DRAFT]),
        ({"target": "builder_style_theme", "notes": "neon noir", **pf}, [DRAFT]),
        ({"target": "builder_story_idea", "notes": "a night walk", "style_theme": "Noir."}, [DRAFT]),
        ({"target": "builder_story_idea", "story_idea": "rain", "lyrics": "x"}, [DRAFT]),
        ({"target": "builder_story_idea", "notes": "a night walk", **pf}, [DRAFT]),
        ({"target": "builder_subjects_and_scenes", "notes": "Ann and a dock", "style_theme": "Noir.", "story_idea": "ignored"}, [DRAFT]),
        ({"target": "builder_subjects_and_scenes", "story_idea": "A long night."}, [DRAFT]),
        ({"target": "style_theme", "lyrics": "She walks home alone"}, [DRAFT]),
        ({"target": "style_theme", "lyrics": "She walks home alone", "notes": "make it warm", **pf}, [DRAFT]),
        ({"target": "story_idea", "lyrics": "She walks home alone", "style_theme": "Noir.", "notes": "slow"}, [DRAFT]),
        ({"target": "story_idea", "lyrics": "She walks home alone", **pf}, [DRAFT]),
        ({"target": "subjects_and_scenes", "story_idea": "A long night.", "notes": "Ann is the singer"}, [DRAFT]),
        ({"target": "subjects_and_scenes", "story_idea": "A long night."}, [DRAFT]),
        ({"target": "song_lyrics", "notes": "a song about rain", "duration": "45"}, [DRAFT]),
        ({"target": "song_lyrics"}, [DRAFT]),
        ({"target": "text_to_image_from_concept", "concept_prompt": "A woman in an alley.", "story_idea": "Night.", "style_theme": "Noir.", "extra_user_input": "more rain"}, [DRAFT]),
        ({"target": "text_to_image_from_concept", "concept_prompt": "A woman in an alley."}, [DRAFT]),
        ({"target": "text_to_image_from_concept", "concept_prompt": "  "}, []),
        ({"target": "text_to_video_from_concept", "concept_prompt": "A woman walks.", "subjects_and_scenes": "Ann", "style_theme": "Noir.", "extra_user_input": "slow"}, [DRAFT]),
        ({"target": "text_to_video_from_concept", "concept_prompt": "A woman walks."}, [DRAFT]),
        ({"target": "text_to_video_from_concept"}, []),
        ({"target": "advanced_prompt_detail", "prompts": ["First scene.", "", "Third scene.", None, 7], "label": "Lighting", "notes": "warm"}, [DRAFT]),
        ({"target": "advanced_prompt_detail", "prompts": ["Only one."]}, [DRAFT]),
        ({"target": "advanced_prompt_detail", "prompts": ["", "  "]}, []),
        ({"target": "advanced_prompt_detail", "prompts": "x"}, []),
        ({"target": "location_description_detail", "location_name": "Neon alley", "location_description": "a narrow wet alley"}, [DRAFT]),
        ({"target": "location_description_detail", "location_name": "Neon alley", "notes": "from the notes"}, [DRAFT]),
        ({"target": "location_description_detail", "location_name": "Neon alley", "location_description": "   ", "notes": "ignored"}, []),
        ({"target": "location_description_detail", "location_name": "Neon alley", "location_description": "a narrow wet alley", **pf}, [DRAFT]),
        ({"target": "location_description_detail", "location_description": "a narrow wet alley"}, []),
        ({"target": "location_description_detail", "location_name": "Neon alley"}, []),
        ({"target": "nope", "notes": "x"}, []),
        ({"notes": "x"}, []),
        ({"target": "builder_story_idea", "notes": "a night walk", "temperature": 0.9, "top_p": 0.8, "max_new_tokens": 900}, [DRAFT]),
        ({"target": "builder_story_idea", "notes": "a night walk", "unload_after": True}, [DRAFT]),
        ({"target": "builder_story_idea", "notes": "a night walk"}, ["```text\n" + DRAFT + "\n```"]),
        ({"target": "builder_story_idea", "notes": "a night walk"}, ["Answer: " + DRAFT]),
        ({"target": "builder_story_idea", "notes": "a night walk"}, [""]),
        ({"target": "builder_story_idea", "notes": "a night walk"}, ["```text\n```"]),
        ({"target": "builder_story_idea", "notes": "a night walk", "n_ctx": "x"}, []),
        ({"target": "builder_story_idea", "notes": "a night walk", "n_threads": "many"}, []),
        ({"target": "builder_story_idea", "notes": "a night walk", "temperature": "hot"}, []),
        ({"target": "builder_story_idea", "notes": "a night walk", "text_runner": "builtin"}, []),
        ({"target": "builder_story_idea", "notes": "a night walk", "text_runner": "builtin", "model_file": ""}, []),
        ({"target": "builder_story_idea", "notes": "a night walk", "model_file": "g.gguf"}, [DRAFT]),
    ]
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.gemma4"])
    function = modules["llm.gemma4"]._run_gemma4_prompt
    folder, _ = h.temp_files({})
    custom = Path(folder.name) / "project_context" / "custom_llm_instructions"
    custom.mkdir(parents=True)
    for name, body in FILES.items():
        (custom / name).write_bytes(body.encode("utf-8"))
    out = []
    with folder:
        for request, replies in cases():
            payload = {**RUNNER, **json.loads(json.dumps(request).replace("<TMP>", json.dumps(str(Path(folder.name).resolve()))[1:-1]))}
            outcome = h.run_case(recorder, function, payload, replies)
            out.append({"fn": "gemma4", "request": request, "replies": replies, **h.scrub(outcome, folder.name)})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "gemma4.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"gemma4.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
