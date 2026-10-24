"""Dev tool: golden cases for the MiniMax H3 prompt writer (`_generate_builder_t2v_prompt`), from the pack's own code with the LLM replaced
by a recorder.

    python tools/make_video_prompt_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/video_prompt.json``. Only what the app serves is covered: MiniMax H3 instruction keys and shot-description tasks (the Builder always sends one), not
the First Last Frame mode, not the built-in runner with pictures (the pack loads a GGUF for it). See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
TASK = (
    "MiniMax H3 shot-description task.\n"
    'Return exactly 2 JSON shot descriptions: {"shots":[{"description":"..."}]}\n'
    "Write only creative shot text. Do not include [Shot N] labels, cut times, markdown, or analysis.\n"
    "Mode: Reference to Video\nDuration: 6s, 16:9\nAudio mode: Input Audio 1 preserved by Builder\nShot count: 2\n"
    "Subject: Ann, a singer in a red coat.\nLocation: a rainy street."
)
REPLY = json.dumps({"shots": [{"description": "Ann sings under the neon sign, rain on her coat."}, {"description": "A slow push in on her face as she closes her eyes."}]})
PIC = h.png_data_url(40, 30)
TALL = h.png_data_url(600, 900, (20, 120, 200))
WIDE = h.png_data_url(1000, 400, (60, 60, 60))
KEY = "minimax_h3_reference_to_video"
PF = {"project_folder": "<TMP>", "scene_id": "scene_1"}


def cases():
    base = {"t2i_prompt": TASK, "builder_instruction_key": KEY}
    out = [
        (base, [REPLY]),
        ({**base, **PF}, [REPLY]),
        ({**base, "project_folder": "<TMP>", "scene_id": "scene 2/x"}, [REPLY]),
        ({**base, "project_folder": "<TMP>"}, [REPLY]),
        ({**base, "project_folder": "<TMP>", "scene_id": "  "}, [REPLY]),
        ({**base, "project_folder": "<TMP>nope", "scene_id": "scene_1"}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "minimax_h3_text_to_video", **PF}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "minimax_h3_text_to_video", "project_folder": "<TMP>", "scene_id": "scene_2"}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "minimax_h3_text_to_video"}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "minimax_h3_image_to_video"}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "minimax_h3_image_reference_to_video"}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "minimax_h3_video_to_video"}, [REPLY]),
        ({"t2i_prompt": TASK, "instruction_key": "minimax_h3_frame_continuity"}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": " MiniMax-H3 Reference To Video "}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "nope"}, []),
        ({"t2i_prompt": TASK, "builder_instruction_key": "Not A Key!"}, []),
        # the notes and the context that goes with them
        ({**base, "user_notes": "Slow camera, handheld."}, [REPLY]),
        ({**base, "subject_context": "Ann: red coat, dark hair."}, [REPLY]),
        ({**base, "subject_context": "Ann: red coat.", "user_notes": "Handheld."}, [REPLY]),
        ({**base, "location_context": "A rainy street at night."}, [REPLY]),
        ({**base, "no_character_present": True, "subject_context": "Ann", "user_notes": "Empty street."}, [REPLY]),
        ({**base, "no_subject": 1}, [REPLY]),
        ({**base, "no_visible_subject": "yes", "location_context": "Alley."}, [REPLY]),
        ({**base, "theme_style_path": "<TMP>/theme.txt", "story_idea_path": "<TMP>/story.txt", "subject_scene_path": "<TMP>/subject.txt"}, [REPLY]),
        ({**base, "theme_style_path": "<TMP>/theme.txt", "user_notes": "Handheld.", "subject_context": "Ann.", "location_context": "Street."}, [REPLY]),
        ({**base, "no_character_present": True, "subject_scene_path": "<TMP>/subject.txt"}, [REPLY]),
        ({**base, "theme_style_path": "<TMP>/missing.txt"}, []),
        ({**base, "story_idea_path": "  "}, [REPLY]),
        # the scene concept
        ({"builder_instruction_key": KEY, "scene_prompt": TASK}, [REPLY]),
        ({"builder_instruction_key": KEY, "t2i_prompt": "   ", "scene_prompt": TASK}, [REPLY]),
        ({"builder_instruction_key": KEY}, []),
        ({"builder_instruction_key": KEY, "t2i_prompt": ""}, []),
        # the lyric cue map and lyric-free prompts
        ({**base, "lyric_cue_map": [{"type": "vocal", "singer_name": "Ann", "text": "She said \"stay\"", "start": 0, "end": 3.5}, {"type": "instrumental", "start": 3.5, "end": 6}]}, [REPLY]),
        ({**base, "omit_lyrics_from_video_prompts": True, "lyric_cue_map": [{"singer_name": "Ann", "text": "x", "start": 0, "end": 3}, {"type": "Instrumental ", "start": 3, "end": 6}], "lyric_text": "She said stay"}, [REPLY]),
        ({**base, "lyric_cue_map": [{"speaker_name": "Bo", "text": "it's fine"}, "junk", 5, {"text": "no singer", "start": None, "end": 2}]}, [REPLY]),
        ({**base, "lyric_cue_map": [{"singer_name": "Ann", "text": "héllo", "start": "1", "end": "2"}], "user_notes": "Existing notes."}, [REPLY]),
        ({**base, "lyric_cue_map": ["junk"]}, [REPLY]),
        ({**base, "lyric_cue_map": []}, [REPLY]),
        ({**base, "omit_lyrics_from_video_prompts": True, "lyric_text": "Words to leave out"}, [REPLY]),
        ({**base, "omit_lyrics_from_video_prompts": True}, [REPLY]),
        ({**base, "omit_lyrics_from_video_prompts": True, "audio_mode": "built_in_audio", "lyric_text": "kept out of it"}, [REPLY]),
        ({**base, "omit_lyrics_from_video_prompts": True, "performance_mode": "no_lip_sync", "lyric_text": "x"}, [REPLY]),
        ({**base, "omit_lyrics_from_video_prompts": False, "lyric_text": "not used"}, [REPLY]),
        ({**base, "performance_mode": "speaking", "audio_mode": "built_in_audio", "lyric_text": "Hello there"}, [REPLY]),
        ({**base, "facial_performance": "soft_smile", "emotion_expression_tags": "wistful, tender", "lyric_text": "la la"}, [REPLY]),
        ({**base, "facial_performance": "off"}, [REPLY]),
        ({**base, "facial_performance": "custom", "facial_performance_custom": "A slow blink."}, [REPLY]),
        # pictures
        ({**base, "image_references": [{"data": PIC}]}, [REPLY]),
        ({**base, "image_references": [{"data": PIC}, {"data": WIDE}, {"data": TALL}]}, [REPLY]),
        ({**base, "image_references": [{"data": TALL}], "temperature": 0.3, "top_p": 0.6, "max_new_tokens": 900}, [REPLY]),
        ({**base, "image_references": [{"data": PIC}] * 12}, [REPLY]),
        ({**base, "image_references": [{"data": PIC}] * 12, "prompt_only_scene_inspiration": True}, [REPLY]),
        ({**base, "image_references": [{"data": PIC}] * 12, "frame_continuity_prompt": True}, [REPLY]),
        ({"t2i_prompt": TASK, "builder_instruction_key": "minimax_h3_frame_continuity", "image_references": [{"data": PIC}] * 11}, [REPLY]),
        ({**base, "image_references": json.dumps([{"data": PIC}, {"data": WIDE}])}, [REPLY]),
        ({**base, "image_references": "<TMP>/ref.png\n\n  <TMP>/ref2.png  \n"}, [REPLY]),
        ({**base, "image_references": ["<TMP>/ref.png", {"path": "<TMP>/ref2.png"}, 5, None, {"data": PIC}]}, [REPLY]),
        ({**base, "image_references": [{"path": "<TMP>/ref.png", "data": PIC}]}, [REPLY]),
        ({**base, "image_references": [{"data": "not a data url"}]}, []),
        ({**base, "image_references": [{"path": "<TMP>/nope.png"}]}, []),
        ({**base, "image_references": [{}]}, []),
        ({**base, "image_references": "{}"}, [REPLY]),
        ({**base, "image_references": {}}, [REPLY]),
        ({**base, "image_reference_data": PIC}, [REPLY]),
        ({**base, "image_reference_path": "<TMP>/ref.png"}, [REPLY]),
        ({**base, "image_reference_path": "<TMP>/nope.png"}, []),
        ({**base, "image_reference_data": PIC, "image_reference_path": "<TMP>/ref.png"}, [REPLY]),
        ({**base, "image_references": [{"data": PIC}], "image_reference_data": TALL}, [REPLY]),
        ({**base, "image_references": [{"data": PIC}], "omit_lyrics_from_video_prompts": True, "lyric_text": "x", **PF}, [REPLY]),
        # the runner and sampling
        ({**base, "temperature": 0.2, "top_p": 0.5, "max_new_tokens": 1500}, [REPLY]),
        ({**base, "lmstudio_output_token_limit": 2048}, [REPLY]),
        ({**base, "text_runner": "own_server", "own_server_output_token_limit": 900, "max_new_tokens": 3000}, [REPLY]),
        ({**base, "text_runner": "llm_api"}, [REPLY]),
        ({**base, "unload_after": False}, [REPLY]),
        ({**base, "n_ctx": "x"}, []),
        ({**base, "n_gpu_layers": "many"}, []),
        ({**base, "n_threads": "x"}, []),
        ({**base, "temperature": "hot"}, []),
        ({**base, "top_p": [1]}, []),
        ({**base, "max_new_tokens": "lots"}, []),
        ({**base, "text_runner": "builtin"}, []),
        ({**base, "text_runner": "builtin", "model_file": "model.bin"}, []),
        ({**base, "text_runner": "qwen_local", "model_file": ""}, []),
        # the reply
        (base, ["```json\n" + REPLY + "\n```"]),
        (base, ["Answer: " + REPLY]),
        (base, ["<think>planning the shots</think>" + REPLY]),
        (base, ["  \n" + REPLY + "\n  "]),
        (base, [""]),
        (base, ["```\n```"]),
        (base, []),
    ]
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.video_prompt_generation"])
    function = modules["llm.video_prompt_generation"]._generate_builder_t2v_prompt
    folder, _ = h.temp_files({"theme.txt": "﻿Noir, neon, rain.\n", "story.txt": "A woman walks home.\n", "subject.txt": "The singer: Ann.\n"})
    root = Path(folder.name)
    custom = root / "project_context" / "custom_builder_instructions"
    (custom / "scenes" / "scene_2").mkdir(parents=True)
    (custom / "minimax_h3_text_to_video.txt").write_bytes(b"Project H3 text to video instructions.\n")
    (custom / "scenes" / "scene_2" / "minimax_h3_text_to_video.txt").write_bytes(b"Scene 2 H3 text to video instructions.\n")
    from PIL import Image

    Image.new("RGB", (64, 48), (10, 200, 10)).save(root / "ref.png")
    Image.new("RGB", (800, 700), (200, 10, 10)).save(root / "ref2.png")
    real = json.dumps(str(root.resolve()))[1:-1]
    out = []
    with folder:
        for request, replies in cases():
            payload = {**RUNNER, **json.loads(json.dumps(request).replace("<TMP>", real))}
            outcome = h.run_case(recorder, function, payload, replies)
            out.append({"fn": "t2v", "request": request, "replies": replies, **h.scrub(outcome, folder.name)})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "video_prompt.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"video_prompt.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
