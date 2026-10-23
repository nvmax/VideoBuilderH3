"""Dev tool: scene input resolution cases (which reference images, frames and continuity a scene renders with).

    python tools/make_scene_fixtures.py --pack <pack folder>

Writes ``fixtures/scene/<topic>.json`` from ``minimax/scene_inputs.py``. The synthetic session uses the same
``flux_reference_builder`` shape the Video Builder saves. Paths under the fake root count as existing files.
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

ROOT = pack_env.FAKE_ROOT + "\\Projects\\Demo Song"
MISSING = "C:\\h3-fixture-does-not-exist\\missing.png"


def img(name):
    return {"path": f"{ROOT}\\refs\\{name}", "name": name}


def session(**overrides):
    """A Reference Builder with two characters, a band-mate extra, two locations and an ingredients sheet."""
    refs = {
        "use_subject_reference": True,
        "subject_count": 2,
        "subjects": [
            {"id": "s1", "name": "Mara", "description": "lead singer, red jacket", "image": img("mara.png"), "reference_image_type": "single"},
            {"id": "s2", "name": "Joe", "description": "drummer", "image": img("joe.png"), "reference_image_type": "single"},
            {"id": "s1b", "name": "Mara side", "image": img("mara_side.png"), "extra_reference_for": "s1"},
        ],
        "subject": {"description": "lead singer, red jacket", "image": img("mara.png")},
        "extras_enabled": True,
        "extra_subjects": [
            {"id": "e1", "name": "Dancer", "description": "backup dancer", "send_to_minimax": True, "image": img("dancer.png")},
            {"id": "e2", "name": "Fan", "description": "crowd member", "send_to_minimax": False, "image": img("fan.png")},
        ],
        "locations": [
            {"id": "l1", "name": "Neon street", "description": "wet street at night", "image": img("street.png")},
            {"id": "l2", "name": "Rooftop", "description": "rooftop lounge", "image": img("roof.png")},
            {"id": "l3", "name": "No image yet", "description": "placeholder", "image": {}},
        ],
        "ingredients_sheets": [{"id": "g1", "name": "Props", "description": "guitar and mic", "image": img("props.png")}],
        "subject_scene_map": {"sc1": "s1", "sc2": "s1,s2", "sc3": "s2", "sc4": ["s1", "s1b"]},
        "scene_map": {"sc1": "l1", "sc2": "l1", "sc3": "l2", "sc4": "l3", "sc5": "l2"},
        "extra_scene_map": {"sc2": [{"extra_id": "e1"}], "sc3": [{"extra_id": "e2"}]},
        "ingredients_scene_map": {"sc5": "g1"},
    }
    value = {"flux_reference_builder": refs}
    value.update(overrides)
    return value


SEGMENTS = {
    "sc1": {"id": "sc1"},
    "sc2": {"id": "sc2"},
    "sc3": {"id": "sc3"},
    "sc4": {"id": "sc4"},
    "sc5": {"id": "sc5"},
    "sc6_unmapped": {"id": "sc6"},
    "no_character": {"id": "sc1", "no_character_present": True},
    "explicit_keys": {"id": "sc1", "minimax_h3_reference_keys": ["location:l2", "subject:s2", "bogus:key", "subject:s2"]},
    "empty_explicit_keys": {"id": "sc1", "minimax_h3_reference_keys": []},
    "extra_paths": {"id": "sc1", "minimax_h3_image_paths": [f"{ROOT}\\refs\\extra1.png", {"path": f"{ROOT}\\refs\\mara.png"}]},
    "with_image": {"id": "sc1", "approved_image_path": f"{ROOT}\\zimage_approved\\image_0001.png"},
    "with_history": {"id": "sc1", "image_history": [f"{ROOT}\\img\\a.png", f"{ROOT}\\img\\b.png"], "image_history_index": 1,
                     "approved_image_path": f"{ROOT}\\zimage_approved\\image_0001.png"},
    "start_frame": {"id": "sc1", "approved_image_path": f"{ROOT}\\zimage_approved\\image_0001.png", "minimax_h3_use_scene_image_as_start_frame": True},
    "start_frame_choice": {"id": "sc1", "approved_image_path": f"{ROOT}\\zimage_approved\\image_0001.png", "minimax_h3_scene_image_use": "exact-start-frame"},
    "flf": {"id": "sc1", "approved_image_path": f"{ROOT}\\zimage_approved\\image_0001.png", "minimax_h3_i2v_frame_mode": "flf",
            "first_last_frame_end_image_path": f"{ROOT}\\zimage_approved\\image_0002.png"},
    "flf_no_end": {"id": "sc1", "approved_image_path": f"{ROOT}\\zimage_approved\\image_0001.png", "minimax_h3_i2v_frame_mode": "flf"},
    "normal_ignores_end": {"id": "sc1", "approved_image_path": f"{ROOT}\\zimage_approved\\image_0001.png", "minimax_h3_i2v_frame_mode": "normal",
                           "first_last_frame_end_image_path": f"{ROOT}\\zimage_approved\\image_0002.png"},
    "missing_file": {"id": "sc1", "approved_image_path": MISSING},
    "video_refs": {"id": "sc1", "minimax_h3_video_references": [{"path": f"{ROOT}\\clips\\a.mp4", "use_audio": True}]},
    "nine_extra": {"id": "sc1", "minimax_h3_image_paths": [f"{ROOT}\\refs\\x{i}.png" for i in range(10)]},
}


def call(fn, *args, **kwargs):
    try:
        return {"output": fn(*args, **kwargs)}
    except Exception as exc:
        return {"error": {"type": type(exc).__name__, "message": str(exc)}}


def write(out_dir: Path, name: str, function: str, cases: list) -> None:
    path = out_dir / f"{name}.json"
    path.write_text(json.dumps({"function": function, "cases": cases}, indent=1, sort_keys=True, ensure_ascii=False), encoding="utf-8")
    print(f"{name}: {len(cases)} cases ({sum(1 for c in cases if 'error' in c)} errors)")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "scene")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    inputs = importlib.import_module("vrgdg_pack.minimax.scene_inputs")

    base_session = session()
    (args.out / "session.json").write_text(json.dumps(base_session, indent=1, sort_keys=True), encoding="utf-8")
    (args.out / "segments.json").write_text(json.dumps(SEGMENTS, indent=1, sort_keys=True), encoding="utf-8")

    modes = ("reference_to_video", "image_to_video", "image_reference_to_video", "video_to_video", "text_to_video")

    cases = []
    for (seg_name, seg), mode in itertools.product(SEGMENTS.items(), modes):
        index = int(seg["id"][2]) - 1 if seg["id"].startswith("sc") and seg["id"][2:].isdigit() else 0
        cases.append({"input": {"segment": seg_name, "mode": mode, "index": index},
                      **call(inputs.reference_choices, copy.deepcopy(base_session), copy.deepcopy(seg), index, mode)})
    write(args.out, "reference_choices", "reference_choices", cases)

    cases = []
    for (seg_name, seg), mode in itertools.product(SEGMENTS.items(), modes):
        index = int(seg["id"][2]) - 1 if seg["id"][2:].isdigit() else 0
        cases.append({"input": {"segment": seg_name, "mode": mode, "index": index},
                      **call(inputs.ordered_reference_items, copy.deepcopy(base_session), copy.deepcopy(seg), mode, index)})
    write(args.out, "ordered_reference_items", "ordered_reference_items", cases)

    cases = []
    for (seg_name, seg), mode in itertools.product(SEGMENTS.items(), modes):
        index = int(seg["id"][2]) - 1 if seg["id"][2:].isdigit() else 0
        for configured in (None, [f"{ROOT}\\cfg\\one.png"]):
            cases.append({"input": {"segment": seg_name, "mode": mode, "index": index, "configured": configured},
                          **call(inputs.render_reference_image_paths, copy.deepcopy(base_session), copy.deepcopy(seg), mode, index, configured)})
    write(args.out, "render_reference_image_paths", "render_reference_image_paths", cases)

    frames_taken = []

    def fake_extract(project_folder, video_path, scene_number):
        frames_taken.append([project_folder, video_path, scene_number])
        return f"{project_folder}\\continuity_frames\\frame_{scene_number:04d}.png"

    previous = {"id": "sc1", "video_path": f"{ROOT}\\rendered_scene_videos\\video_0001-audio.mp4"}
    cases = []
    for (seg_name, seg), (mode, rp), cont, scene_number in itertools.product(
            SEGMENTS.items(),
            (("reference_to_video", "single"), ("reference_to_video", "two_pass"), ("reference_to_video", "three_pass"),
             ("image_to_video", "single"), ("image_reference_to_video", "single"), ("video_to_video", "single")),
            ("off", "latent_continuation_masked", "bogus"), (1, 3)):
        if seg_name not in ("sc1", "sc2", "no_character", "explicit_keys", "with_image", "start_frame", "flf", "flf_no_end", "missing_file", "video_refs"):
            continue
        index = scene_number - 1
        frames_taken.clear()

        def run(seg=seg, mode=mode, rp=rp, cont=cont, scene_number=scene_number, index=index):
            return inputs.resolve_scene_inputs(
                copy.deepcopy(base_session), copy.deepcopy(seg), mode, index, continuity_mode=cont,
                previous_segment=previous if scene_number > 1 else None, project_folder=ROOT, scene_number=scene_number,
                extract_final_frame=fake_extract, render_pass=rp)

        record = call(run)
        record["final_frames_requested"] = list(frames_taken)
        cases.append({"input": {"segment": seg_name, "mode": mode, "render_pass": rp, "continuity": cont, "scene_number": scene_number}, **record})
    write(args.out, "resolve_scene_inputs", "resolve_scene_inputs", cases)

    cases = [{"input": {"value": v}, "canonical": inputs.canonical_continuity_mode(v), "normalized": inputs.normalize_continuity_mode(v)}
             for v in (None, "", "off", "OFF", "latent", "Latent Continuation", "latent-continuation-masked", "latent_exact_frame",
                       "latent_masked_av", "continuation", "spatial", "exact_start_frame", "bogus", 5)]
    write(args.out, "continuity_modes", "canonical_continuity_mode / normalize_continuity_mode", cases)

    cases = [{"input": {"path": p}, "output": inputs.media_path_key(p)} for p in (None, "", "C:\\A\\B.PNG", "c://a//b.png", " /x/y ", "a\\\\b")]
    write(args.out, "media_path_key", "media_path_key", cases)

    cases = []
    for mapping, seg, index in (
            ({"sc1": "a"}, {"id": "sc1"}, 0), ({"sc1": "a"}, {"id": "sc2"}, 1), ({"1": "n1", "2": "n2"}, {"id": "zz"}, 1),
            ({"1": "n1", "x": "bad"}, {"id": "zz"}, 0), ({}, {"id": "sc1"}, 0), (None, {}, 0), ({"3": "n3"}, {}, 2)):
        cases.append({"input": {"mapping": mapping, "segment": seg, "index": index}, "output": inputs.scene_map_value(mapping, seg, index)})
    write(args.out, "scene_map_value", "scene_map_value", cases)

    cases = [{"input": {"segment": name}, "output": inputs.segment_image_path(seg)} for name, seg in SEGMENTS.items()]
    cases += [{"input": {"segment": {"image_history": ["a.png"], "image_history_index": 9}}, "output": inputs.segment_image_path({"image_history": ["a.png"], "image_history_index": 9})},
              {"input": {"segment": {"image_history": ["", "b.png"], "image_history_index": 0, "custom_image_path": "c.png"}},
               "output": inputs.segment_image_path({"image_history": ["", "b.png"], "image_history_index": 0, "custom_image_path": "c.png"})}]
    write(args.out, "segment_image_path", "segment_image_path", cases)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
