"""Dev tool: golden cases for the omit-lyrics policy (``h3-core::prompt::lyric_free``), from the pack.

    python tools/make_lyric_free_fixtures.py --pack <pack folder>

Writes ``fixtures/prompt/lyric_free.json``.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

DESCRIPTIONS = [
    "<Subject 1> (Ava) stands on the roof, wind lifting the jacket, the camera drifting closer.",
    "<Subject 1> sings <d>[English] hold me close tonight</d> with her lips parted, camera pushes in.",
    "She sings with passion in sync with <Audio 1> as the camera glides, her mouth shaping each word.",
    "<Subject 1> sings with a soft smile and <Subject 2> sings gently while the camera orbits.",
    "At 2.5 seconds the lights flare. <Subject 1> sings during 0s–2.5s. Her jaw tightens. The wind rises.",
    "<Subject 1> says \"hold me close\" and sways, \"never let go\" echoes. The camera tilts down.",
    "He sings, they sing. <Subject 2> keeps his lips pressed together and watches.",
    "",
    "Just camera movement across the skyline.",
]

CONTRACTS = [
    "<Subject 1> sings with passion in sync with the vocals in <Audio 1> during 0.3s–2.8s. <Subject 1>'s engaged eyes and expressive brows convey the song's intensity.",
    "<Subject 1> sings with passion in sync with the vocals in <Audio 1>.",
    "The assigned performer sings with passion in sync with the vocals in <Audio 1>.",
    "<Subject 2> sings in sync with <Audio 1> during 3.0s–5.0s.",
    "<Subject 1> sings with passion in sync with the vocals in <Audio 1> during 0s–2.5s.",
    "no singing here",
]

LYRIC_SETS = [["hold me close tonight"], ["hold me close", "never let go"], ["", " "], ["Hold  Me"], []]

CUE_ROWS = [
    {"type": "vocal", "text": "we run through the night", "singer_name": "Ava", "singer_id": "a", "start": 0.0, "vocal_start": 0.3, "vocal_end": 2.8, "end": 3.0},
    {"type": "vocal", "text": "and never look back", "singer_name": "Ben", "singer_id": "b", "start": 3.4, "vocal_start": 3.6, "vocal_end": 6.0, "end": 6.5},
]

REFS = {
    "subjects": [{"id": "a", "name": "Ava", "description": "dark hair", "image": {"path": "Z:\\p\\a.png"}}, {"id": "b", "name": "Ben", "description": "beard", "image": {"path": "Z:\\p\\b.png"}}],
    "locations": [{"id": "l1", "name": "Roof", "description": "a roof", "image": {"path": "Z:\\p\\r.png"}}],
}


def session(**extra):
    s = {
        "video_engine": "minimax_h3",
        "minimax_h3_settings": {"audio_mode": "input_audio", "aspect_ratio": "16:9"},
        "omit_lyrics_from_video_prompts": True,
        "segments": [{"id": "s1", "start": 0.0, "end": 6.5, "label": "Scene 1", "lyric_text": "we run through the night\nand never look back"}],
        "flux_reference_builder": {**copy.deepcopy(REFS), "subject_scene_map": {"s1": ["a", "b"]}, "scene_map": {"s1": "l1"}, "use_subject_reference": True, "use_location_references": True},
    }
    s.update(extra)
    return s


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "prompt"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    lfp = importlib.import_module("vrgdg_pack.minimax.lyric_free_performance")
    pa = importlib.import_module("vrgdg_pack.minimax.prompt_assembly")
    sp = importlib.import_module("vrgdg_pack.minimax.shot_prompt")

    data = {"merge": [], "clean": [], "facial": [], "directions": [], "apply": [], "context": [], "cut_plan": [], "performers": [], "enabled": []}

    for description in DESCRIPTIONS:
        for contract in CONTRACTS:
            data["merge"].append({"description": description, "contract": contract, "return": list(lfp.merge_contract(description, contract))})

    for text in DESCRIPTIONS + ["He mouths \"hold me close tonight\" quietly, hold me close tonight again.", "A 3.5 second hold, lyrics drift by. Vocals swell."]:
        for lyrics in LYRIC_SETS:
            for preserve in (False, True):
                data["clean"].append({"text": text, "lyrics": lyrics, "preserve": preserve, "return": lfp.clean_description(text, lyrics, preserve, False)})

    segments = {
        "plain": {"id": "s1", "lyric_text": "hold me close tonight"},
        "no_character": {"id": "s1", "lyric_text": "hold me close", "no_character_present": True},
        "instrumental": {"id": "s1", "lyric_text": "[instrumental]"},
        "no_lip_sync": {"id": "s1", "lyric_text": "la la", "lyric_no_lip_sync": True},
        "cue_map": {"id": "s1", "lyric_text": "we run\nand back", "lyric_performance_mode": "cue_map", "lyric_cue_map": copy.deepcopy(CUE_ROWS)},
        "cue_missing_times": {"id": "s1", "lyric_text": "we run\nand back", "lyric_performance_mode": "cue_map",
                              "lyric_cue_map": [{"type": "vocal", "text": "one", "singer_name": "Ava"}, {"type": "instrumental", "start": 2.0}, {"type": "vocal", "text": "two", "singer_name": "Zed", "start": 4.0, "end": 5.0}]},
        "cue_int_times": {"id": "s1", "lyric_text": "x", "lyric_performance_mode": "cue_map", "lyric_cue_map": [{"type": "vocal", "text": "x", "start": 0, "end": 3, "singer_id": "b"}, {"type": "vocal", "text": "y", "start": 3, "vocal_start": None, "vocal_end": 6}]},
        "cue_together_mode": {"id": "s1", "lyric_text": "a b", "lyric_performance_mode": "together", "lyric_cue_map": copy.deepcopy(CUE_ROWS)},
        "singers": {"id": "s1", "lyric_text": "one two", "lyric_singers": ["Ben", "Ava", "Nobody"]},
    }
    plans = {
        "single": pa.storyboard_cut_plan_for_duration(6.5, 0),
        "multi": pa.storyboard_cut_plan_for_duration(6.5, 5),
        "cue": sp.cue_cut_plan(pa.storyboard_cut_plan_for_duration(6.5, 0), sp.timed_cue_rows(segments["cue_map"])),
    }
    labels_sets = {"none": {}, "two": {"Ava": "<Subject 1>", "Ben": "<Subject 2>", "a": "<Subject 1>", "b": "<Subject 2>"}}
    presets = ["", "off", "custom", "pop_polished", "pop_flirty", "love_tender", "sad_wounded", "happy_joyful", "rock_intense", "metal_rage", "rap_high_intensity", "theatrical", "unknown"]

    for seg_name, segment in segments.items():
        for plan_name, plan in plans.items():
            for label_name, labels in labels_sets.items():
                for performer in ("<Subject 1>", "<Subject 1> and <Subject 2>"):
                    for index in range(-1, plan["shot_count"]):
                        data["directions"].append({"segment": segment, "plan": plan, "performer": performer, "index": index, "labels": labels,
                                                   "return": lfp.directions(segment, plan, performer, index, labels)})

    for preset in presets:
        for custom in ("", "soft eyes, parted lips, and a tender glance. Warm cheeks; jaw relaxed, bright notes"):
            segment = {"id": "s1", "lyric_text": "x", "facial_performance": preset, "facial_performance_custom": custom}
            data["facial"].append({"segment": segment, "session": {}, "return": lfp.facial_text(segment, {})})
    data["facial"].append({"segment": {"id": "s1"}, "session": {"default_facial_performance": "love_tender", "default_facial_performance_custom": "extra eyes"}, "return": lfp.facial_text({"id": "s1"}, {"default_facial_performance": "love_tender", "default_facial_performance_custom": "extra eyes"})})
    data["facial"].append({"segment": {"id": "s1", "facial_performance": 5}, "session": {}, "return": lfp.facial_text({"id": "s1", "facial_performance": 5}, {})})
    # The scene's own facial settings come before the project's, and a present but empty one still counts.
    for segment, session_defaults in (
        ({"id": "s1", "scene_defaults": {"facial_performance": "theatrical"}}, {}),
        ({"id": "s1", "scene_defaults": {"facial_performance": "theatrical"}}, {"default_facial_performance": "love_tender"}),
        ({"id": "s1", "scene_defaults": {"facial_performance": ""}}, {"default_facial_performance": "love_tender"}),
        ({"id": "s1", "scene_defaults": {"facial_performance": "off"}}, {"default_facial_performance": "love_tender"}),
        ({"id": "s1", "scene_defaults": {"facial_performance_custom": "wry brows"}}, {"default_facial_performance": "love_tender", "default_facial_performance_custom": "extra eyes"}),
        ({"id": "s1", "facial_performance": "pop_flirty", "scene_defaults": {"facial_performance": "theatrical"}}, {}),
        ({"id": "s1", "scene_defaults": "broken"}, {"default_facial_performance": "happy_joyful"}),
        ({"id": "s1"}, {"default_facial_performance": "theatrical"}),
    ):
        data["facial"].append({"segment": segment, "session": session_defaults, "return": lfp.facial_text(segment, session_defaults)})

    for seg_name, segment in segments.items():
        for facial in ("", "pop_polished", "off"):
            seg = {**segment, "facial_performance": facial} if facial else dict(segment)
            for plan_name, plan in plans.items():
                for label_name, labels in labels_sets.items():
                    shots = plan["shot_count"]
                    descriptions = [DESCRIPTIONS[(i + len(seg_name)) % len(DESCRIPTIONS)] for i in range(shots)]
                    for performer in ("<Subject 1>", "<Subject 1> and <Subject 2>"):
                        sess = session()
                        data["apply"].append({"descriptions": descriptions, "segment": seg, "plan": plan, "performer": performer, "labels": labels, "session": sess,
                                              "return": lfp.apply_shots(descriptions, seg, plan, performer, labels, sess)})
    task = ("MiniMax H3 shot-description task.\n\nVocal performance: Ava sings the exact words.\n\nExact lyric: we run through the night\n\n"
            "The scene shows a rooftop. Ava's lips part as she sings we run through the night and never look back. The camera drifts.\n\n"
            "Timed singer / shot contract:\nShot 1: Ava\n\nSet the mood with warm light, her jaw set firmly.")
    for seg_name, segment in segments.items():
        for plan_name, plan in plans.items():
            for label_name, labels in labels_sets.items():
                data["context"].append({"text": task, "segment": segment, "plan": plan, "performer": "<Subject 1>", "labels": labels, "session": session(),
                                        "return": lfp.prompt_context(task, segment, plan, "<Subject 1>", labels, session())})

    sessions = {
        "omit": session(),
        "keep": session(omit_lyrics_from_video_prompts=False),
        "built_in": session(minimax_h3_settings={"audio_mode": "built_in_audio", "aspect_ratio": "16:9"}),
        "speaking": session(video_type="speaking"),
        "hard_cuts": session(builder_storyboard_defaults={"minimax_h3_cut_frequency": 5, "minimax_h3_cut_style": "hard"}),
        "freq_string": session(builder_storyboard_defaults={"minimax_h3_cut_frequency": "4"}),
    }
    cut_segments = {
        **segments,
        "continuous": {"id": "s1", "start": 0.0, "end": 6.5, "lyric_text": "x", "location_continuous_shot": True},
        "cue_map_timed": {"id": "s1", "start": 0.0, "end": 6.5, "lyric_text": "x", "lyric_performance_mode": "cue_map", "lyric_cue_map": copy.deepcopy(CUE_ROWS)},
        "cue_map_dupes": {"id": "s1", "start": 0.0, "end": 6.5, "lyric_text": "x", "lyric_performance_mode": "cue_map",
                          "lyric_cue_map": [{"start": 0.0}, {"start": 2.0}, {"start": 2.0}, {"start": 0.02}, {"start": 6.49}, {"start": None}, {"start": 4}]},
        "short": {"id": "s1", "start": 1.0, "end": 1.05, "lyric_text": "x"},
        "scoped_frequency": {"id": "s1", "start": 0.0, "end": 6.5, "lyric_text": "x", "scene_defaults": {"minimax_h3_cut_frequency": 4, "minimax_h3_cut_style": "hard"}},
        "scoped_single": {"id": "s1", "start": 0.0, "end": 6.5, "lyric_text": "x", "scene_defaults": {"minimax_h3_cut_frequency": 0}},
        "scoped_text": {"id": "s1", "start": 0.0, "end": 6.5, "lyric_text": "x", "scene_defaults": {"minimax_h3_cut_frequency": "3"}},
        "scoped_broken": {"id": "s1", "start": 0.0, "end": 6.5, "lyric_text": "x", "scene_defaults": "oops"},
    }
    for sess_name, sess in sessions.items():
        for seg_name, segment in cut_segments.items():
            seg = copy.deepcopy(segment)
            seg.setdefault("start", 0.0)
            seg.setdefault("end", 6.5)
            data["cut_plan"].append({"session": sess_name, "segment": seg, "return": pa.scene_cut_plan(seg, copy.deepcopy(sess))})
            data["enabled"].append({"session": sess_name, "segment": seg, "return": lfp.scene_enabled(sess, seg)})
    data["sessions"] = sessions

    for singers in ([], ["Ben"], ["Ben", "Ava"], ["Ava", "Ava"], ["Nobody"]):
        sess = session()
        sess["segments"][0]["lyric_singers"] = singers
        performer, labels = lfp.scene_performers(sess, sess["segments"][0], "reference_to_video")
        data["performers"].append({"session": sess, "return": {"performer": performer, "labels": labels}})
    empty = session()
    empty["flux_reference_builder"]["subject_scene_map"] = {}
    performer, labels = lfp.scene_performers(empty, empty["segments"][0], "reference_to_video")
    data["performers"].append({"session": empty, "return": {"performer": performer, "labels": labels}})

    (out / "lyric_free.json").write_text(json.dumps(data, indent=1, ensure_ascii=False), encoding="utf-8")
    print({k: len(v) for k, v in data.items() if isinstance(v, list)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
