"""Dev tool: golden tables for the cut plan, video style, temporal effect and FX wording, from the pack.

    python tools/make_style_fixtures.py --pack <pack folder>

Writes ``fixtures/timeline/style.json``.
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
    out = Path(__file__).resolve().parents[1] / "fixtures" / "timeline"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    pa = importlib.import_module("vrgdg_pack.minimax.prompt_assembly")
    vs = importlib.import_module("vrgdg_pack.agent_api.video_style")
    te = importlib.import_module("vrgdg_pack.agent_api.temporal_effect")

    cases = []
    for duration in (0.05, 1.0, 2.5, 4.0, 6.2, 9.9, 15.0):
        for freq in (0, 1, 3, 5, 7, 10, 99, -2):
            cases.append({"op": "cut_plan", "duration": duration, "frequency": freq, "return": pa.storyboard_cut_plan_for_duration(duration, freq)})

    styles = list(vs.VIDEO_STYLES)[:6]
    fx_names = list(vs.FX_PRESETS)[:4]
    style_cards = [
        ({}, {}), ({"video_style": styles[0]}, {}), ({}, {"video_style": styles[1]}), ({"video_style": "custom", "video_style_custom": " Neon noir "}, {}),
        ({}, {"video_style": "custom", "video_style_custom": "Dusty film"}), ({"video_style": "unknown style"}, {}), ({"video_style": styles[2]}, {"video_style": styles[3]}),
    ]
    for card, defaults in style_cards:
        cases.append({"op": "style", "card": card, "defaults": defaults, "label": vs.video_style_label(card, defaults), "guidance": vs.video_style_guidance(card, defaults), "verbiage": vs.video_style_verbiage(card, defaults)})

    temporal_keys = list(te.PRESETS)[:3] + ["custom"]
    for key in temporal_keys + ["", "nonsense"]:
        for override in ("", "global", "off", key):
            for protected in ("", "lead_only", "custom"):
                card = {"temporal_world_effect_override": override, "temporal_world_effect_custom": "own card effect"}
                defaults = {"temporal_world_effect": key, "temporal_world_effect_custom": "default custom", "temporal_protected_characters": protected, "temporal_protected_custom": "Ava"}
                text = te.temporal_world_effect_verbiage(card, defaults)
                cases.append({"op": "temporal", "card": card, "defaults": defaults, "return": text})
    sample = te.temporal_world_effect_verbiage({}, {"temporal_world_effect": temporal_keys[0]})
    for prompt in ("", "[Shot 1] a\n\n[Shot 2] b", f"[Shot 1] a {sample} b"):
        cases.append({"op": "temporal_append", "prompt": prompt, "verbiage": sample, "return": te.append_temporal_world_effect(prompt, sample)})
        cases.append({"op": "temporal_append", "prompt": prompt, "verbiage": "", "return": te.append_temporal_world_effect(prompt, "")})

    fx_defaults = [{}, {"fx_preset": fx_names[0]}, {"fx_preset": fx_names[1]}, {"fx_preset": "custom", "fx_custom_json": json.dumps({"cues": ["spark", {"text": "flash"}], "primary": "burst", "timing": "on the drop"})},
                   {"fx_preset": "custom", "fx_custom_json": "not json"}, {"fx_preset": "ghost"}]
    core = "[Shot 1] At 00:00.000, one.\n\n[Shot 2] At 00:01.500, two.\n\n[Shot 3] At 00:03.000, three."
    for defaults in fx_defaults:
        cases.append({"op": "fx", "defaults": defaults, "accents": [vs.fx_accent(defaults, i) for i in range(4)], "reserve": vs.fx_reserve(defaults, 3), "apply": vs.apply_fx(core, defaults)})
    lfp = importlib.import_module("vrgdg_pack.minimax.lyric_free_performance")
    articulation = [
        "", "Ava sings with her mouth open wide, and the camera pushes in.",
        "<Subject 1> sings <d>[English] la la la.</d> with her lips parted, eyes bright, as the camera tilts up.",
        "He keeps his lips pressed together and watches. She belts the chorus with a trembling jaw.",
        "Her mouth forms the words; she walks forward, then turns. The jaw tightens, and she looks up!",
        "Rate 2.5 per second, with natural lips moving, the singer steps. \"Quoted lips stay.\" Final.",
        "A [Image 1] wide shot with natural mouth, then and while lips quiver; hair blows in wind...",
        "No articulation here at all. Just a wide shot, slow dolly, warm light.",
    ]
    for text_in in articulation:
        cases.append({"op": "articulation", "text": text_in, "return": lfp.strip_singing_articulation(text_in)})
    emo = importlib.import_module("vrgdg_pack.llm.prompts.emotion_expression")
    payloads = [
        {}, {"facial_performance": "off"}, {"facial_performance": "natural"}, {"facial_performance": "custom", "facial_performance_custom": "bitter"},
        {"facial_performance_custom": "wistful"}, {"emotion_expression_tags": "start happy, then end sad", "lyric_text": "hold me close"},
        {"emotion_expression_tags": "angry", "facial_performance": "off", "lyric_text": ""},
        {"performance_mode": "speaking", "audio_mode": "built_in_audio"},
        {"performance_mode": "speaking", "audio_mode": "built_in_audio", "emotion_expression_tags": "calm", "lyric_text": "Hello"},
        {"video_type": "speaking", "audio_mode": "input_audio", "facial_performance": "joyful"},
    ]
    for payload in payloads:
        cases.append({"op": "emotion", "payload": payload, "return": emo.emotion_expression_instruction(payload)})
    segments = [({}, {}), ({"emotion_expression_tags": " sad ", "facial_performance": "soft"}, {"default_facial_performance": "x"}), ({}, {"default_facial_performance": "calm", "default_facial_performance_custom": "tender"})]
    for seg_, sess_ in segments:
        cases.append({"op": "emotion_input", "segment": seg_, "session": sess_, "return": emo.emotion_expression_input(seg_, sess_)})
    data = Path(__file__).resolve().parents[1] / "crates" / "h3-core" / "data"
    (data / "temporal_presets.json").write_text(json.dumps({"presets": te.PRESETS, "audio_direction": te.AUDIO_DIRECTION}, indent=1, ensure_ascii=False), encoding="utf-8")
    (out / "style.json").write_text(json.dumps({"cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print("style cases:", len(cases))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
