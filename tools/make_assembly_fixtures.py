"""Dev tool: golden prompt assembly, validation and agent briefs (``h3-core::prompt::assembly``), from the pack.

    python tools/make_assembly_fixtures.py --pack <pack folder>

Each case is a project session, a scene, a mode and the shot descriptions an agent sent. The pack's
``assemble_minimax_h3_prompt``, ``validate_minimax_h3_prompt`` and ``build_minimax_prompt_context`` answer them.
Writes ``fixtures/prompt/assembly.json``.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

ROOT = "Z:\\h3fixture\\proj"
MODES = [None, "text_to_video", "image_to_video", "reference_to_video", "image_reference_to_video", "video_to_video", "i2v", "r2v", "nonsense"]


def base_session(**extra):
    session = {
        "project_folder": ROOT,
        "video_engine": "minimax_h3",
        "minimax_h3_settings": {"audio_mode": "input_audio", "aspect_ratio": "16:9"},
        "segments": [
            {"id": "s1", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "hold me close tonight", "approved_image_path": ROOT + "\\images\\image_0001.png"},
            {"id": "s2", "start": 4.0, "end": 9.0, "label": "Scene 2", "lyric_text": "[instrumental]", "notes": "slow drift", "first_last_frame_end_image_path": ROOT + "\\images\\end.png",
             "approved_image_path": ROOT + "\\images\\image_0002.png"},
            {"id": "s3", "start": 9.0, "end": 15.5, "label": "Scene 3", "lyric_text": "we run through the night\nand never look back", "minimax_h3_video_style": "noir"},
            {"id": "s4", "start": 15.5, "end": 17.0, "label": "Scene 4"},
        ],
        "flux_reference_builder": {
            "subjects": [
                {"id": "a", "name": "Ava", "description": "short dark hair, denim jacket", "image": {"path": ROOT + "\\subject_location\\subject\\Ava.png"}},
                {"id": "b", "name": "", "auto_build_role": "drummer", "reference_type": "extra"},
            ],
            "locations": [{"id": "l1", "name": "Roof", "description": "a rooftop at dusk", "image": {"path": ROOT + "\\subject_location\\location\\Roof.png"}}],
            "subject_scene_map": {"s1": ["a"], "s2": ["a"], "s3": ["a"]},
            "scene_map": {"s1": "l1", "s2": "l1", "s3": "l1"},
            "use_subject_reference": True,
            "use_location_references": True,
        },
    }
    session.update(extra)
    return session


VARIANTS = {
    "plain": {},
    "structured": {"use_structured_outputs": True},
    "cuts_5": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 5}},
    "cuts_10_continuous": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 10, "minimax_h3_cut_style": "continuous", "video_style": "cinematic_realism"}},
    "cuts_3_structured": {"use_structured_outputs": True, "builder_storyboard_defaults": {"minimax_h3_cut_frequency": 3, "video_style": "dark_fantasy"}},
    "built_in_audio": {"minimax_h3_settings": {"audio_mode": "built_in_audio", "aspect_ratio": "16:9"}},
    "speaking": {"video_type": "speaking"},
    "omit_lyrics": {"minimax_h3_settings": {"audio_mode": "input_audio", "omit_lyrics_from_video_prompts": True}},
    "masked": {"minimax_h3_settings": {"audio_mode": "input_audio", "continuity_mode": "latent_continuation_masked"}, "builder_storyboard_defaults": {"minimax_h3_cut_frequency": 2}},
    "session_mode": {"video_mode": "reference_to_video"},
    # Theatrical presets chosen for the whole project, and scenes with their own Scene Defaults on top.
    "theatrical_project": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 2, "performance_style": "theatrical", "camera_flow": "pretty_little_malfunction"}, "default_facial_performance": "theatrical"},
    "theatrical_scoped": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 2, "performance_style": "theatrical", "camera_flow": "pretty_little_malfunction"}, "default_facial_performance": "theatrical"},
    "omit_lyrics_theatrical": {"omit_lyrics_from_video_prompts": True, "default_facial_performance": "theatrical"},
}


def scene_overrides(session):
    """Scene Defaults on three scenes: a cut frequency and style, other presets (and an empty facial override), and a broken value."""
    segments = session["segments"]
    segments[0]["scene_defaults"] = {"minimax_h3_cut_frequency": 5, "minimax_h3_cut_style": "hard"}
    segments[1]["scene_defaults"] = {"performance_style": "calm", "camera_flow": "close_ups", "facial_performance": "off"}
    segments[2]["scene_defaults"] = {"facial_performance": "", "facial_performance_custom": "wry brows"}
    segments[2]["motion_summary"] = "slow push in"
    segments[3]["scene_defaults"] = "broken"


def make_session(name, extra):
    session = base_session(**copy.deepcopy(extra))
    if name == "theatrical_scoped":
        scene_overrides(session)
    return session

SHOTS = {
    "none": [],
    "one": ["She sings on the roof."],
    "dirty": ["[Shot 1] At 00:00.000, Ava sings, photorealistic, 4k\nhigh quality lighting", "  Camera pushes in   slowly  ", "No blur and no distortion\nShe turns toward the light!", "A fourth description", "a fifth"],
    "many": ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"],
}

PROMPTS = [
    "",
    "   ",
    "just text",
    "integrated_multimodal_description:\nGenerate a 4-second video.\n\n[Shot 1] A shot.",
    "detailed_description:\nstyle\n\n[Shot 2] At 00:01.000, a",
    "subject_definitions:\nx\n\nsummary:\ny\n\nretention_analysis:\nz\n\ndetailed_description:\n[Shot 1] ok <Picture 1> <Picture 2>",
    "[Shot 1] " + " ".join(f"<Picture {n}>" for n in range(1, 12)),
    "x" * 7001,
    "integrated_multimodal_description: [shot 1] ok",
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "prompt"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    pa = importlib.import_module("vrgdg_pack.minimax.prompt_assembly")

    assemble, context, validate = [], [], []
    sessions = {name: make_session(name, extra) for name, extra in VARIANTS.items()}
    for vname, extra in VARIANTS.items():
        session = make_session(vname, extra)
        for index in range(len(session["segments"])):
            scene = copy.deepcopy(session["segments"][index])
            case = {"variant": vname, "index": index}
            try:
                context.append({**case, "return": pa.build_minimax_prompt_context(copy.deepcopy(scene), copy.deepcopy(session))})
            except Exception as exc:  # noqa: BLE001
                context.append({**case, "error": str(exc)})
            for mode in MODES:
                for sname, shots in SHOTS.items():
                    entry = {**case, "mode": mode, "shots": sname, "descriptions": shots}
                    try:
                        result = pa.assemble_minimax_h3_prompt(copy.deepcopy(scene), copy.deepcopy(session), list(shots), mode=mode)
                        entry["return"] = result
                    except Exception as exc:  # noqa: BLE001
                        entry["error"] = str(exc)
                    assemble.append(entry)
            for mode in MODES[:6]:
                for prompt_index, prompt in enumerate(PROMPTS):
                    for strict in (False, True):
                        validate.append({
                            **case, "mode": mode, "prompt": prompt_index, "strict": strict,
                            "return": pa.validate_minimax_h3_prompt(prompt, segment=scene, mode=mode, fail_on_invalid_prompt_formats=strict),
                        })
    # The validator also runs without a scene.
    for prompt_index, prompt in enumerate(PROMPTS):
        for strict in (False, True):
            validate.append({"variant": None, "index": None, "mode": None, "prompt": prompt_index, "strict": strict,
                             "return": pa.validate_minimax_h3_prompt(prompt, segment=None, mode=None, fail_on_invalid_prompt_formats=strict)})
    (out / "assembly.json").write_text(json.dumps({"sessions": sessions, "prompts": PROMPTS, "assemble": assemble, "context": context, "validate": validate}, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    errors = sum(1 for c in assemble + context if "error" in c)
    print(f"assembly.json: {len(assemble)} assemble, {len(context)} context, {len(validate)} validate, {errors} errors")
    return 0


if __name__ == "__main__":
    sys.exit(main())
