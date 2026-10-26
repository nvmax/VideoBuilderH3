"""Dev tool: input and output tables for the pack's MiniMax H3 prompt text builders (``minimax/shot_prompt.py``).

    python tools/make_prompt_fixtures.py --pack <pack folder>

Writes ``fixtures/prompt/<topic>.json``. The LLM is not involved: these are the deterministic parts the Rust port must
reproduce character for character (shot labels, cut times, wrappers, reference definitions, validation, cue handling).
"""

import argparse
import importlib
import itertools
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

PLANS = {
    "one_shot": {"exact_duration_seconds": 5.0, "cut_times_seconds": [], "frequency": 0},
    "two_cuts": {"exact_duration_seconds": 8.25, "cut_times_seconds": [2.5, 5.125], "frequency": 5},
    "one_cut": {"exact_duration_seconds": 4.0, "cut_times_seconds": [2.0], "frequency": 3},
    "many_cuts": {"exact_duration_seconds": 12.0, "cut_times_seconds": [1.5, 3.0, 4.5, 7.25, 9.0, 10.5], "frequency": 9},
    "continuous_two": {"exact_duration_seconds": 8.25, "cut_times_seconds": [2.5, 5.125], "frequency": 5, "continuous": True},
    "long_minute": {"exact_duration_seconds": 75.0, "cut_times_seconds": [61.234], "frequency": 2},
}

ITEMS = {
    "singer_only": [{"kind": "subject", "label": "Mara", "description": "lead singer, red jacket, short dark hair"}],
    "full_cast": [
        {"kind": "subject", "label": "Mara", "description": "lead singer, red jacket, short dark hair"},
        {"kind": "subject", "label": "the Joe", "description": "drummer with a beard, grey hoodie and a wide silver ring on each hand, always sweating through the set"},
        {"kind": "extra", "label": "Dancer", "description": "backup dancer"},
        {"kind": "location", "label": "Neon street", "description": "wet street at night, signs reflecting in puddles"},
        {"kind": "ingredients", "label": "Props", "description": "guitar and mic"},
    ],
    "start_frame": [
        {"kind": "start_frame", "label": "Scene start frame"},
        {"kind": "subject", "label": "Mara", "description": "lead singer"},
        {"kind": "location", "label": "Rooftop", "description": ""},
    ],
    "start_and_end": [
        {"kind": "start_frame"}, {"kind": "subject", "label": "Mara"}, {"kind": "end_frame"},
    ],
    "location_only": [{"kind": "location", "label": "Empty hall", "description": "a long empty hall"}],
    "custom_purpose": [{"kind": "subject", "label": "Mara", "purpose": "voice and face reference; keep the scar"}],
    "none": [],
}

STYLES = ("", "cinematic realism", "anime", "  claymation  ")

TEXTS = [
    "", "  A steady shot.  ", "Camera pans left. No tears or crying. The singer smiles.", "no text overlays, the subject runs.",
    "The camera cuts to a close-up of <Subject 1>. She sings <d>[English] Hold on tight.</d> and smiles.",
    "[Shot 3] At 00:04.250, the camera cuts to a wide shot.", "At 00:04.250, the camera cuts to a wide shot of the room.",
    "Cut to a low angle. The drummer hits hard.", "Without any sweat, she dances. Never blinks.", "A  double  spaced   sentence .",
    "Hard cut to black. Then <Subject 2> stands.", "the camera cuts to: a wide shot.", "Avoid blur; keep sharp. The band plays on.",
]

LYRICS = [
    "", "Hold on tight\nDon't let go", "[Verse 1]\nHold on tight\n\nDon't let go\n[Chorus]\nWe are the night", "One line only",
    "a\nb\nc\nd\ne\nf\ng", "  spaced   out   line  \r\nsecond line\r\n",
]

DESCRIPTIONS = {
    "ok_one": ["<Subject 1> sings <d>[English] Hold on tight.</d> in sync with <Audio 1> and the camera drifts closer in a slow push."],
    "ok_three": [
        "A wide shot of <Subject 1> on the curb, singing in sync with <Audio 1> as neon light flickers over the wet street.",
        "Continuing directly from the previous shot's final moment, <Subject 1> keeps singing as the camera cuts to a low angle close-up.",
        "<Subject 1> finishes the line and holds a quiet, steady gaze at the camera, ending on a clear held pose.",
    ],
    "short_final": ["A wide shot of the street.", "Short."],
    "no_punctuation": ["A wide shot of the street at night with rain", "A close-up of her face as she sings the final line with feeling"],
    "picture_leak": ["Starting from <Picture 1>, the camera follows her down the street and she sings the first line.", "A close-up."],
}


def call(fn, *args, **kwargs):
    try:
        return {"output": fn(*args, **kwargs)}
    except Exception as exc:  # the expected error is part of the fixture
        error = {"type": type(exc).__name__, "message": str(exc)}
        if hasattr(exc, "code"):
            error["code"] = exc.code
        return {"error": error}


def write(out_dir: Path, name: str, function: str, cases: list) -> None:
    path = out_dir / f"{name}.json"
    path.write_text(json.dumps({"function": function, "cases": cases}, indent=1, sort_keys=True, ensure_ascii=False), encoding="utf-8")
    print(f"{name}: {len(cases)} cases ({sum(1 for c in cases if 'error' in c)} errors)")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "prompt")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    sp = importlib.import_module("vrgdg_pack.minimax.shot_prompt")
    out = args.out

    write(out, "timecode", "timecode", [{"input": {"seconds": v}, **call(sp.timecode, v)} for v in
                                          (0, 0.5, 1.0005, 2.5, 59.9996, 60, 61.234, 125.5, None, -3, "7.25", "abc")])
    write(out, "plans", "shot_plan / apply_cut_style / cut_plan_instruction", [
        {"input": {"plan": name, "cut_style": style},
         "shot_plan": call(sp.shot_plan, plan)["output"],
         "applied": call(sp.apply_cut_style, plan, style)["output"],
         "instruction": call(sp.cut_plan_instruction, call(sp.apply_cut_style, plan, style)["output"])["output"]}
        for (name, plan), style in itertools.product(PLANS.items(), (None, "free", "continuous", "CONTINUOUS"))])
    write(out, "cut_style_value", "cut_style_value", [{"input": {"value": v}, "output": sp.cut_style_value(v)} for v in (None, "", "free", "continuous", " Continuous ", "x")])

    write(out, "performance_craft", "performance_craft_instruction", [
        {"input": {"style": s, "has_cast": c, "has_vocals": v, "continuation": k}, **call(sp.performance_craft_instruction, s, c, v, k)}
        for s, c, v, k in itertools.product(("", "cinematic realism", "anime"), (True, False), (True, False), (True, False))])
    write(out, "static_texts", "literal_scene_instruction / scene_timing_instruction / opening_wrapper / motion_energy_text", [
        {"input": {"fn": "literal_scene_instruction"}, "output": sp.literal_scene_instruction()},
    ] + [
        {"input": {"fn": "scene_timing_instruction", "duration": d, "plan": name}, **call(sp.scene_timing_instruction, d, plan)}
        for d in (3.5, 5.0, 8.25, 12.0) for name, plan in PLANS.items()
    ] + [
        {"input": {"fn": "opening_wrapper", "style": s, "continuous": c, "guidance": g}, **call(sp.opening_wrapper, s, c, g)}
        for s, c, g in itertools.product(STYLES, (False, True), ("", "Soft film grain and warm light."))
    ] + [
        {"input": {"fn": "motion_energy_text", "camera": a, "character": b}, **call(sp.motion_energy_text, a, b)}
        for a in (0, 2, 4.5, 5, 7, 10) for b in (0, 3, 5, 8, 10)
    ])
    write(out, "life_movements", "life_movements", [
        {"input": {"character_speed": speed, "seed": seed}, **call(sp.life_movements, speed, seed)}
        for speed in (0, 2, 4, 5, 6, 8, 10) for seed in ("", "scene-1", "scene-2", "Mara 17")])

    write(out, "description_cleanup", "strip_negative_sentences / normalize_description / continuous_cut_text / post_cut_text", [
        {"input": {"text": t}, "strip_negative_sentences": call(sp.strip_negative_sentences, t).get("output"),
         "normalize_description": call(sp.normalize_description, t).get("output"),
         "post_cut_text": call(sp.post_cut_text, t).get("output"),
         "continuous_cut_text_2": call(sp.continuous_cut_text, t, 2).get("output"),
         "continuous_cut_text_5": call(sp.continuous_cut_text, t, 5).get("output")} for t in TEXTS])

    raws = [
        '["a", "b"]', '{"shots": ["a", "b"]}', '{"descriptions": ["a", "b", "c"]}', '```json\n["a", "b"]\n```', 'Here you go: ["a","b"] hope that helps',
        '["only one"]', '[]', '', 'not json at all', '{"shot_1": "a", "shot_2": "b"}', '[{"description": "a"}, {"description": "b"}]',
        '["a", "b",]', '["[Shot 1] a", "[Shot 2] At 00:02.500, b"]', '["a"\n, "b"]', '"just a string"',
    ]
    write(out, "parse_shot_descriptions", "parse_shot_descriptions", [
        {"input": {"raw": r, "expected": n}, **call(sp.parse_shot_descriptions, r, n)} for r in raws for n in (1, 2, 3)])

    cases = []
    for (dname, descs), (pname, plan), style, guidance in itertools.product(
            DESCRIPTIONS.items(), PLANS.items(), ("cinematic realism", ""), ("", "Warm grain.")):
        record = {"input": {"descriptions": dname, "plan": pname, "style": style, "guidance": guidance}}
        record["shot_body"] = call(sp.shot_body, descs, plan)
        record["assemble_prompt"] = call(sp.assemble_prompt, descs, plan, style, guidance)
        record["character_budget"] = call(sp.character_budget, plan, style)
        if "output" in record["assemble_prompt"]:
            record["validate_prompt"] = call(sp.validate_prompt, record["assemble_prompt"]["output"], plan)
        cases.append(record)
    write(out, "assemble_and_validate", "shot_body / assemble_prompt / character_budget / validate_prompt", cases)

    prompts = ["", "   ", "no sections here", "detailed_description:\n[Shot 1] A short line.", "detailed_description:\n[Shot 1] " + "x" * 7100 + ".",
               "detailed_description: [Shot 1] A wide shot of the street at night with a long take. [Shot 1] Again.",
               "detailed_description:\n[Shot 1] A wide shot of the street at night. [Shot 2] At 00:02.500, close up of a singer.",
               "detailed_description:\n[Shot 1] A wide shot of the street at night ending on a clear held pose with <Subject 1> sing",
               "detailed_description:\n[Shot 1] Begin exactly from <Picture 1> and sing the first line of the song today."]
    write(out, "validate_prompt_edges", "validate_prompt", [
        {"input": {"prompt": p, "plan": name}, **call(sp.validate_prompt, p, plan)} for p in prompts for name, plan in PLANS.items()])

    cases = []
    for (iname, items), (pname, plan), style, audio, direction in itertools.product(
            ITEMS.items(), PLANS.items(), ("cinematic realism", ""), ("input_audio", "built_in_audio"), ("", "Rain on tin and distant traffic.")):
        if pname not in ("one_shot", "two_cuts", "continuous_two"):
            continue
        frame = call(sp.reference_frame, items, plan, style, audio, direction)
        record = {"input": {"items": iname, "plan": pname, "style": style, "audio_mode": audio, "audio_direction": direction}, "frame": frame}
        if "output" in frame:
            record["wrapped"] = sp.wrap_reference_prompt("detailed_description:\n[Shot 1] A shot.", frame["output"])
            record["wrapped_again"] = sp.wrap_reference_prompt(record["wrapped"], frame["output"])
        cases.append(record)
    write(out, "reference_frame", "reference_frame / wrap_reference_prompt", cases)

    creative_samples = ["<Subject 1> sings. <Subject 2> plays drums.", "<Subject 1> (S1) walks while <Subject 2> (the drummer) waits.", "Nobody is named here.",
                        "[Shot 1] <Subject 1> (Mara) stands.\n\n[Shot 2] At 00:02.500, <Subject 2> (Joe) cuts in.\n\n[Shot 3] At 00:05.000, wide."]
    cases = []
    for (iname, items), text in itertools.product(ITEMS.items(), creative_samples):
        cases.append({"input": {"items": iname, "text": text},
                      "labels": call(sp.reference_labels, items), "bindings": call(sp.character_reference_bindings, text, items),
                      "compact": call(sp.compact_reference_prompt, text, items)})
    write(out, "reference_labels", "reference_labels / character_reference_bindings / compact_reference_prompt", cases)

    write(out, "compact_description", "_compact_description", [
        {"input": {"text": t, "limit": n}, "output": sp._compact_description(t, n)} for t in
        ("", "short text", "A long description, with clauses; and more words follow here to pass the limit easily today.",
         "NoSpacesAtAllJustOneVeryLongTokenThatNeverBreaksAnywhereForALongTimeAndKeepsGoingAndGoing", "ends with period.  ")
        for n in (20, 60, 140, 240)])

    write(out, "lyrics", "lyric_lines / lyric_chunks / ensure_quoted_lyrics", [
        {"input": {"lyrics": ly, "shots": n}, "lines": sp.lyric_lines(ly), "chunks": call(sp.lyric_chunks, ly, n),
         "quoted": call(sp.ensure_quoted_lyrics, ["A wide shot.", "A close-up.", "A last shot."][:n], ly)}
        for ly in LYRICS for n in (1, 2, 3)] + [
        {"input": {"lyrics": "Hold on tight", "shots": 1, "descriptions": d, "performer": p},
         "quoted": call(sp.ensure_quoted_lyrics, d, "Hold on tight", p)}
        for d in (['<Subject 1> sings <d>[English] Hold on tight.</d> now.'], ['She sings "Hold on tight" loudly.'], ['She sings hold on tight loudly.'], ["A wide shot."])
        for p in ("the singer", "<Subject 1> (Mara)")])

    saved = ["", "no shots", "[Shot 1] only one shot here.", "[Shot 1] first.\n\n[Shot 2] At 00:02.500, second shot, long text " + "y" * 800 + "\n\ndetailed more",
             "[Shot 1] a\n\n[Shot 2] At 00:01.000, b\n\noverall_soundscape: x"]
    write(out, "last_shot_text", "last_shot_text", [{"input": {"saved": s, "limit": n}, "output": sp.last_shot_text(s, n)} for s in saved for n in (700, 40)])

    # Timed cue maps.
    cue_segments = {
        "together": {"lyric_performance_mode": "together", "lyric_cue_map": [{"text": "a", "start": 0}]},
        "cue_map": {"lyric_performance_mode": "cue_map", "lyric_cue_map": [
            {"type": "vocal", "text": "hold on tight", "start": 0, "vocal_start": 0.5, "vocal_end": 2.25, "singer_id": "s1", "singer_name": "Mara"},
            {"type": "instrumental", "start": 2.5, "action_note": "camera cranes up over the crowd"},
            {"type": "vocal", "text": "don't let go", "start": 4.0, "vocal_start": 4.1, "vocal_end": 6.0, "singer_id": "s2", "singer_name": "Joe"}]},
        "cue_map_missing_start": {"lyric_performance_mode": "cue_map", "lyric_cue_map": [{"type": "vocal", "text": "a"}, {"type": "vocal", "text": "b", "start": 2}]},
        "cue_map_blank_vocal": {"lyric_performance_mode": "cue_map", "lyric_cue_map": [{"type": "vocal", "text": "  ", "start": 0}]},
        "empty": {},
    }
    cases = []
    for name, seg in cue_segments.items():
        rows = sp.timed_cue_rows(seg)
        record = {"input": {"segment": name}, "rows": rows}
        if rows:
            labeled = [{**r, "label": "<Subject 1> (Mara)" if r["type"] == "vocal" else ""} for r in rows]
            record["contract"] = sp.cue_shot_contract(labeled)
            record["cut_plan"] = sp.cue_cut_plan({"exact_duration_seconds": 6.5, "cut_times_seconds": [], "frequency": 0}, rows)
            record["enforced"] = [call(sp.enforce_cue_shot, text, row) for row in labeled for text in
                                  ("A wide shot of the singer on stage, singing the line.", "<Subject 1> sings <d>[English, tender, wistful] <i>hold</i> on tight</d> softly.")]
        cases.append(record)
    write(out, "cues", "timed_cue_rows / cue_shot_contract / cue_cut_plan / enforce_cue_shot", cases)

    headers = [None, "", "English", "english, tender, wistful", "Spanish, angry, bold, singing", "tender, wistful", "[English] x", "French , joy"]
    write(out, "cue_header", "cue_header", [{"input": {"header": h}, **call(sp.cue_header, h)} for h in headers])
    deliveries = [("hold on tight", "hold on tight"), ("hold on tight", "<i>hold</i> on tight"), ("hold on tight", "hold <breath> on tight"),
                  ("Hold on tight, don't let go.", "Hold on <i>tight</i>, <breath> don't let <i>go</i>."), ("hold on tight", "something else entirely"),
                  ("a b c d e f", "<i>a</i> <i>b</i> <i>c</i> <i>d</i> <i>e</i> f"), ("it's  fine", "<i>it's</i> fine"), ("", ""), ("don’t stop", "don't <i>stop</i>")]
    write(out, "cue_delivery", "cue_delivery", [{"input": {"cue_text": a, "llm_body": b}, **call(sp.cue_delivery, a, b)} for a, b in deliveries])

    # A tagged cue is normalized in the sentence it is in (merge_timed_cue). The pack's own regression cases, plus one case for every
    # other way the merge reads its sentence: the verb forms, a wrong or missing actor, an action before the verb, no merge possible.
    lfp = importlib.import_module("vrgdg_pack.minimax.lyric_free_performance")
    sentence_cases = json.loads((args.pack / "tests" / "minimax_cue_sentence_cases.json").read_text(encoding="utf-8"))
    vocal = {"type": "vocal", "text": "Find the spark", "label": "<Subject 1>", "vocal_start": 0.21, "vocal_end": 2.67}
    sentence_cases += [
        {"name": "singing after the actor keeps the participle", "cue": vocal,
         "description": "<Subject 1> advances along the path, singing in sync with <Audio 1>: <d>[English, bright, singing] Find the <i>spark</i>.</d> The camera follows."},
        {"name": "continues singing keeps the verb for the same actor", "cue": vocal,
         "description": "<Subject 1> continues singing <d>[English, bright, singing] Find the <i>spark</i>.</d> with a grin."},
        {"name": "lip-syncing for the same actor", "cue": vocal,
         "description": "<Subject 1> is lip-syncing <d>[English, bright, singing] Find the spark.</d> from 0 s to 9 s, shoulders swaying."},
        {"name": "a pronoun is the performer", "cue": vocal,
         "description": "<Subject 1> stands tall. She sings in sync with Audio 1: <d>[English, bright, singing] Find the spark.</d> The light fades."},
        {"name": "an action before a bare verb becomes its own sentence", "cue": vocal,
         "description": "The camera pans across the stage, and sings in sync with <Audio 1>: <d>[English, bright, singing] Find the spark.</d>"},
        {"name": "a cue with no label uses the singer", "cue": {**vocal, "label": None},
         "description": "The camera pans left. Someone sings <d>[English, bright, singing] Find the spark.</d> in sync with <Audio 1>."},
        {"name": "no cue window", "cue": {"type": "vocal", "text": "Find the spark", "label": "<Subject 1>"},
         "description": "<Subject 1> sings <d>[English, bright, singing] Find the spark.</d> and smiles."},
        {"name": "two tags cannot be merged and get the closing contract", "cue": vocal,
         "description": "<Subject 1> sings <d>[English, bright, singing] Find the spark.</d> then sings <d>[English, bright, singing] Find the spark.</d> again."},
        {"name": "a tag with no singing verb before it gets the closing contract", "cue": vocal,
         "description": "A wide shot of <Subject 1> at the mic, <d>[English, bright, singing] Find the spark.</d>, the crowd cheering."},
        {"name": "no tag at all gets the closing contract", "cue": vocal,
         "description": "A wide shot of <Subject 1> at the mic. The crowd cheers."},
        {"name": "an old canonical contract is replaced, not repeated", "cue": vocal,
         "description": "A wide shot of the stage. <Subject 1> is the only visible performer singing, in sync with <Audio 1>, <d>[English, bright, singing] Find the spark.</d>; every other visible performer remains silent."},
    ]
    cases = []
    for item in sentence_cases:
        enforced = sp.enforce_cue_shot(item["description"], item["cue"])
        cases.append({"input": {"name": item["name"], "description": item["description"], "cue": item["cue"]}, "enforced": enforced,
                      "clean": lfp.strip_singing_articulation(enforced), "again": sp.enforce_cue_shot(enforced, item["cue"])})
    write(out, "cue_sentences", "enforce_cue_shot / strip_singing_articulation on sentences that hold a tagged cue", cases)

    # Location continuity and continuation text.
    places = {
        "street": {"id": "l1", "name": "Neon street", "description": "A wet street at night. Signs reflect in puddles."},
        "roof": {"id": "l2", "name": "Rooftop", "description": "rooftop lounge with string lights"},
        "long": {"id": "l3", "name": "Warehouse", "description": "x" * 400},
        "bare": {"name": "Hall"}, "none": None,
    }
    presets = ("normal", "surreal", "cinematic", "inner_world", "match", "motion", "creative_auto", "masked", "custom", "bogus")
    cases = []
    for (cname, cur), (pname, prev), preset, vocals in itertools.product(places.items(), places.items(), presets, (False, True)):
        if preset not in ("normal", "masked", "custom", "bogus", "surreal") and (cname, pname) != ("roof", "street"):
            continue
        for custom, hold in (("", 1.5), ("  fly  through   the sign ", 0)):
            if preset != "custom" and custom:
                continue
            if preset not in ("masked",) and hold == 0:
                continue
            cases.append({"input": {"current": cname, "previous": pname, "preset": preset, "custom": custom, "hold": hold, "has_vocals": vocals},
                          **call(sp.location_continuity_contract, cur, prev, preset, custom, hold, vocals)})
    write(out, "location_contract", "location_continuity_contract", cases)
    write(out, "performance_line", "performance_line", [{"input": {"has_vocals": v, "part": p}, "output": sp.performance_line(v, p)}
                                                         for v in (False, True) for p in ("opening", "transition", "other")])

    continuations = {
        "basic": {"hold_seconds": 1.5, "scene_seconds": 6.0},
        "direction": {"hold_seconds": 1.5, "scene_seconds": 6.0, "direction": "The singer walks toward the stage lights."},
        "zero_hold": {"hold_seconds": 0, "scene_seconds": 4.0},
        "left": {"hold_seconds": 2.0, "scene_seconds": 9.0, "seconds_left": 7.25},
        "empty": {},
    }
    cases = []
    for (cname, cont), vocals, picture, prev_shot, contract in itertools.product(
            continuations.items(), (False, True), (False, True), ("", "The singer holds a pose on the curb."), ("", "LOCATION PHASE — X")):
        cases.append({"input": {"continuation": cname, "has_vocals": vocals, "with_picture": picture, "previous_shot": prev_shot, "location_contract": contract},
                      **call(sp.continuation_task_text, cont, prev_shot, vocals, picture, contract)})
    write(out, "continuation_task", "continuation_task_text", cases)

    # Whole task text the LLM receives for a scene.
    labels = sp.reference_labels(ITEMS["full_cast"])
    base = dict(mode_label="Reference to Video", duration=8.25, aspect_ratio="16:9 (Widescreen)", audio_mode="input_audio",
                cut_plan=PLANS["two_cuts"], style="cinematic realism", labels=labels, camera_speed=6, character_speed=4, seed="scene-3")
    variants = {
        "plain": {}, "lyrics": {"lyric_text": "Hold on tight\nDon't let go"}, "visual_only": {"visual_only": True},
        "no_character": {"no_character": True}, "beat": {"story_beat": "She finds the old guitar.", "lyric_section": "Verse 1", "scene_notes": "Keep it moody."},
        "guidance": {"camera_guidance": "slow dolly", "character_guidance": "subtle acting", "subject_text": "Mara: lead singer", "location_text": "Neon street"},
        "continued": {"continuation": continuations["direction"], "previous_shot": "She holds a pose.", "with_picture": True, "location_contract": "LOCATION PHASE — X"},
        "built_in": {"audio_mode": "built_in_audio", "audio_direction": "Rain and traffic."},
        "motion_request": {"motion_request": "Orbit the singer.", "continuity": "Same wardrobe.", "storyboard_context": "Card context here."},
        "cues": {"cues": [{**r, "label": "<Subject 1> (Mara)"} for r in sp.timed_cue_rows(cue_segments["cue_map"])]},
        "small_limit": {"target_limit": 1500},
    }
    write(out, "build_shot_task", "build_shot_task", [
        {"input": {"variant": name, "kwargs_extra": extra}, **call(sp.build_shot_task, **{**base, **extra})} for name, extra in variants.items()])
    # The named inputs the cases above refer to, so the Rust tests build the same objects.
    (out / "named_inputs.json").write_text(json.dumps({
        "plans": PLANS, "items": ITEMS, "descriptions": DESCRIPTIONS, "cue_segments": cue_segments, "places": places,
        "continuations": continuations, "build_shot_task_base": base,
    }, indent=1, sort_keys=True, ensure_ascii=False), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
