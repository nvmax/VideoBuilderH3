"""Dev tool: golden tables for the timeline and lyric scene logic (``h3-core``), made by the pack's own code.

    python tools/make_timeline_fixtures.py --pack <pack folder>

Each case holds the input scenes, the arguments, and what the pack's function returned and left in the scenes.
Writes ``fixtures/timeline/*.json``.
"""

import argparse
import copy
import importlib
import itertools
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402


def seg(i, start, end, **extra):
    s = {"id": f"s{i}", "start": start, "end": end}
    s.update(extra)
    return s


def timelines():
    four = [seg(1, 0, 4), seg(2, 4, 8), seg(3, 8, 12), seg(4, 12, 16)]
    locked = [seg(1, 0, 4), seg(2, 4, 8, video_path="v2.mp4"), seg(3, 8, 12), seg(4, 12, 16, video_path="v4.mp4")]
    labelled = [seg(1, 0, 3, label="Scene 1"), seg(2, 3, 6, label="2. Rooftop"), seg(3, 6, 9, label="Chorus"), seg(4, 9, 12, label="")]
    gapped = [seg(1, 1, 4), seg(2, 5, 8), seg(3, 8, 12, custom_audio_timeline_start=8.5)]
    return {"four": four, "locked": locked, "labelled": labelled, "gapped": gapped}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "timeline")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    tl = importlib.import_module("vrgdg_pack.builder.timeline")
    ls = importlib.import_module("vrgdg_pack.builder.lyric_scenes")
    counter = itertools.count(1)
    ls.new_segment_id = lambda: f"seg_new{next(counter)}"

    def write(name, cases):
        (args.out / f"{name}.json").write_text(json.dumps({"cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
        print(f"{name}: {len(cases)} cases")

    def run_inplace(fn, base, *a, **k):
        segs = copy.deepcopy(base)
        try:
            ret = fn(segs, *a, **k)
            return {"segments": segs, "return": ret}
        except Exception as e:  # noqa: BLE001
            return {"error": str(e)}

    # normalize_segments
    cases = []
    for name, base in timelines().items():
        for idx in range(len(base)):
            for edit in ({"end": 1.0}, {"end": 99.0}, {"start": -3.0}, {"start": base[idx]["start"] + 1.5}, {"end": base[idx]["end"] + 2.0}):
                segs = copy.deepcopy(base)
                segs[idx].update(edit)
                inp = copy.deepcopy(segs)
                tl.normalize_segments(segs, active_index=idx)
                cases.append({"input": inp, "active_index": idx, "output": segs})
    write("normalize", cases)

    # move / resize
    cases = []
    for name, base in timelines().items():
        for sid in ("s1", "s2", "3", "zz"):
            for start in (0.0, 2.5, 20.0):
                for ripple in (False, True):
                    cases.append({"op": "move", "input": base, "scene": sid, "start": start, "ripple": ripple, **run_inplace(tl.move_scene_timing, base, sid, start, ripple=ripple)})
            for dur, end in ((2.0, None), (0.01, None), (None, 11.0), (None, None)):
                for ripple in (False, True):
                    cases.append({"op": "resize", "input": base, "scene": sid, "duration": dur, "end": end, "ripple": ripple,
                                  **run_inplace(tl.resize_scene_timing, base, sid, new_duration=dur, new_end=end, ripple=ripple)})
    write("move_resize", cases)

    # gaps
    cases = []
    for name, base in timelines().items():
        overlay = [seg(90, 5, 6)]
        segs, ov = copy.deepcopy(base), copy.deepcopy(overlay)
        removed = tl.close_all_base_timeline_gaps(segs, ov)
        cases.append({"op": "close_all", "input": base, "overlay": overlay, "output": segs, "overlay_output": ov, "return": removed})
        for start, end in ((4, 8), (0, 0), (8, 12.5)):
            segs, ov = copy.deepcopy(base), copy.deepcopy(overlay)
            ret = tl.close_base_timeline_gap(segs, ov, start, end)
            cases.append({"op": "close_one", "input": base, "overlay": overlay, "start": start, "end": end, "output": segs, "overlay_output": ov, "return": ret})
    write("gaps", cases)

    # labels, snap, validate
    cases = []
    for name, base in timelines().items():
        segs = copy.deepcopy(base)
        tl.renumber_generic_base_scene_labels(segs)
        cases.append({"op": "labels", "input": base, "output": segs})
        for dur in (None, 10.0, 16.0):
            cases.append({"op": "validate", "input": base, "audio_duration": dur, "return": tl.validate_timeline_consistency(copy.deepcopy(base), None, dur)})
    for t, beats in ((1.0, [0.9, 1.1, 2.0]), (1.0, [{"time": 1.13}]), (1.0, []), (5.0, [1.0]), (0.5, [0.64, 0.36])):
        cases.append({"op": "snap", "t": t, "beats": beats, "return": tl.snap_to_beat(t, beats)})
    write("labels_snap_validate", cases)

    # bulk parse
    cases = []
    texts = {
        "durations": ["4\n3.5\n2", "1:30\n0:45.5", "- 4 Hello darkness\n# note\n\n3.5 my old friend", "x", "0.01", ""],
        "ranges": ["0 --> 4\n4 - 8.5", "0:10 to 0:20 Verse one\n20 -> 25", "5 - 4", "abc", "1,5 - 3,5 hello"],
        "markers": ["0\n4\n8", "8 b\n0 a\n4 c", "1 only", "0\n0.01"],
    }
    for mode, items in texts.items():
        for text in items:
            try:
                out = {"return": tl.parse_bulk_scenes(text, mode=mode)}
            except Exception as e:  # noqa: BLE001
                out = {"error": str(e)}
            cases.append({"mode": mode, "text": text, **out})
    write("bulk_parse", cases)

    # lyric helpers
    cases = []
    texts = ["", "[instrumental]", "Hello world", "line one\nline two\nline three", "one two three four five", "single", "no vocals", "Instrumental break", "[Break]"]
    for t in texts:
        cases.append({"op": "instrumental", "text": t, "return": ls.is_instrumental_lyric_text(t)})
        for frac in (0.1, 0.5, 0.9):
            cases.append({"op": "split", "text": t, "fraction": frac, "return": list(ls.split_lyric_text(t, frac))})
        for u in ("", "tail", "[instrumental]", t):
            cases.append({"op": "merge", "a": t, "b": u, "return": ls.merge_lyric_text(t, u)})
    old = [seg(1, 0, 4, lyric_text="a b c d", lyric_singers=["Ava"]), seg(2, 4, 8, lyric_text="x\ny"), seg(3, 8, 12, lyric_text="[instrumental]")]
    for ranges in ([(0, 8), (8, 12)], [(0, 2), (2, 4), (4, 12)], [(20, 24)], [(0, 12)]):
        cases.append({"op": "carry", "old": old, "ranges": ranges, "return": ls.carry_lyrics_over(old, ranges)})
    write("lyrics", cases)

    # length fixes
    cases = []
    bases = {
        "mixed": [seg(1, 0, 14, lyric_text="a b c d e f"), seg(2, 14, 15, lyric_text="short"), seg(3, 15, 19), seg(4, 19, 19.5, lyric_text="[instrumental]")],
        "even": [seg(1, 0, 6), seg(2, 6, 12)],
        "locked": [seg(1, 0, 20, video_path="v.mp4"), seg(2, 20, 21), seg(3, 21, 30)],
        "single": [seg(1, 0, 1)],
    }
    for name, base in bases.items():
        for lo, hi in ((3.0, 8.0), (2.0, 5.0), (0.5, 30.0), (9.0, 4.0)):
            locked_ids = [s["id"] for s in base if s.get("video_path")]
            try:
                fix = ls.next_length_fix(copy.deepcopy(base), lo, hi, set(locked_ids))
            except ValueError as e:
                cases.append({"op": "fix", "input": base, "min": lo, "max": hi, "locked": locked_ids, "error": str(e)})
                continue
            entry = {"op": "fix", "input": base, "min": lo, "max": hi, "locked": locked_ids, "return": fix}
            if fix:
                segs = copy.deepcopy(base)
                ls.apply_length_fix(segs, fix)
                entry["applied"] = segs
            cases.append(entry)
    write("length_fix", cases)

    # scene card fields
    card = importlib.import_module("vrgdg_pack.storyboard.scene_card_fields")
    patches = [
        {"label": " Hello ", "notes": "plan\r\nmore", "director_note": "dir"},
        {"lyrics": "la la", "lyric_text": "la la"},
        {"lyrics": "la", "lyric_text": "different"},
        {"performance_mode": "No-Lip Sync", "include_microphone": True, "no_character_present": False},
        {"performance_mode": "dancing"},
        {"include_microphone": "yes"},
        {"label": 5, "story_beat": None},
        {"label": True},
        {"label": "x" * 200},
        {"subject_ids": ["a", "b", "a", " "], "location_id": "loc1"},
        {"subject_ids": "solo", "lyric_singers": "Ava"},
        {"subject_ids": [1.5]},
        {"lyric_cue_map": [{"a": 1}], "lyric_shot_word_timing_enabled": True},
        {"lyric_cue_map": ["x"]},
        {"image_prompt": "a still", "video_prompt": "a clip", "video_prompt_origin": "gemma"},
        {"video_prompt": "only clip"},
        {"video_prompt_origin": "manual"},
        {"video_style": "noir", "video_style_custom": "grain", "temporal_world_effect_override": ""},
        {"speaker_assignments": [{"speaker_id": "s1", "speaker_name": " Ava ", "text": " Hello ", "start": "1.5", "end": -2}, {"type": "Instrumental", "text": ""}]},
        {"speaker_assignments": "nope"},
        {"speaker_assignments": ["x"]},
        {"prompt_summary": "card only", "trigger_phrase": "tp"},
        {"unknown_field": 1, "i2v_notes": "motion", "t2i_prompt": "legacy"},
        {"minimax_h3_pass2_prompt": "second", "continuity_direction": "keep"},
    ]
    cases = []
    base_seg = {"id": "s1", "start": 0, "end": 4, "label": "Scene 1", "lyric_text": "old"}
    for patch in patches:
        for engine, mode in (("minimax_h3", "zimage"), ("ltx", "flux_klein")):
            entry = {"patch": patch, "engine": engine, "mode": mode}
            try:
                values, rest = card.parse_scene_card_patch(patch)
                segment = copy.deepcopy(base_seg)
                card.apply_to_segment(segment, values, video_engine=engine, image_mode=mode)
                entry.update({"values": values, "rest": rest, "segment": segment, "card_only": card.card_only_names(values)})
            except card.SceneCardFieldError as exc:
                entry["error"] = str(exc)
            cases.append(entry)
    # Scene Defaults: the scene's own overrides, merged, cleared, refused, and kept in step with the scene fields that share their names.
    scoped_patches = [
        ({}, {"scene_defaults": {"camera_flow": "medium_shots", "camera_motion_speed": 0, "unused": None}}),
        ({}, {"scene_defaults": {"camera_flow": "medium_shots", "camera_motion_speed": 0}}),
        ({}, {"scene_defaults": {}}),
        ({}, {"scene_defaults": {"nope": 1, "also": 2}}),
        ({}, {"scene_defaults": {"camera_motion_speed": 11}}),
        ({}, {"scene_defaults": {"temporal_allow_background_extras": 1}}),
        ({}, {"scene_defaults": "x"}),
        ({}, {"scene_defaults": {"performance_style": "theatrical", "video_style": "noir", "facial_performance_custom": "wry"}, "label": "Opening"}),
        ({"scene_defaults": {"camera_flow": "close_ups", "performance_style": "calm"}}, {"scene_defaults": {"camera_flow": "medium_shots"}}),
        ({"scene_defaults": {"camera_flow": "close_ups", "performance_style": "calm"}}, {"scene_defaults": {}}),
        ({"scene_defaults": {"camera_flow": "close_ups", "performance_style": "calm", "video_style": "noir"}}, {"performance_style": "theatrical", "video_style_custom": "grain"}),
        ({"scene_defaults": {"video_style": "noir"}}, {"video_style": "pastel", "label": "x"}),
        ({"scene_defaults": "broken"}, {"label": "x"}),
    ]
    for base, patch in scoped_patches:
        for engine, mode in (("minimax_h3", "zimage"),):
            entry = {"patch": patch, "engine": engine, "mode": mode, "base": base}
            try:
                values, rest = card.parse_scene_card_patch(patch)
                segment = {**copy.deepcopy(base_seg), **copy.deepcopy(base)}
                card.apply_to_segment(segment, values, video_engine=engine, image_mode=mode)
                entry.update({"values": values, "rest": rest, "segment": segment, "card_only": card.card_only_names(values)})
            except card.SceneCardFieldError as exc:
                entry["error"] = str(exc)
            cases.append(entry)
    cases.append({"names": card.field_names()})
    write("scene_card", cases)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
