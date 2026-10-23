"""Dev tool: tables of inputs and outputs for the pack's pure MiniMax H3 functions.

    python tools/make_core_fixtures.py --pack <pack folder>

Writes ``fixtures/core/<topic>.json``. Each file is ``{"function": ..., "cases": [{"input": ..., "output" | "error": ...}]}``.
The Rust ports in ``h3-core`` must return the same values (floats within 1e-9) or the same error text.
"""

import argparse
import importlib
import itertools
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

ASPECTS = [
    "1:1 (Square)", "2:3 (Portrait Photo)", "3:2 (Photo)", "3:4 (Portrait Standard)", "4:3 (Standard)",
    "9:16 (Portrait Widescreen)", "16:9 (Widescreen)", "21:9 (Ultrawide)",
]


def call(fn, *args, **kwargs):
    """Run ``fn`` and return ``{"output": value}`` or ``{"error": {...}}``."""
    try:
        return {"output": fn(*args, **kwargs)}
    except Exception as exc:  # recorded as the expected result
        return {"error": {"type": type(exc).__name__, "message": str(exc)}}


def plain(value):
    """Make dataclasses, tuples and other values JSON friendly."""
    if hasattr(value, "to_dict"):
        return value.to_dict()
    if isinstance(value, tuple):
        return [plain(v) for v in value]
    if isinstance(value, dict):
        return {k: plain(v) for k, v in value.items()}
    if isinstance(value, list):
        return [plain(v) for v in value]
    return value


def write(out_dir: Path, name: str, function: str, cases: list) -> None:
    for case in cases:
        if "output" in case:
            case["output"] = plain(case["output"])
    path = out_dir / f"{name}.json"
    path.write_text(json.dumps({"function": function, "cases": cases}, indent=1, sort_keys=True, ensure_ascii=False), encoding="utf-8")
    errors = sum(1 for c in cases if "error" in c)
    print(f"{name}: {len(cases)} cases ({errors} errors)")


def timing_cases(latent):
    cases = []

    def add(**kw):
        args = {"timeline_start_seconds": kw.pop("start"), "timeline_end_seconds": kw.pop("end"),
                "warmup_frames": kw.pop("warm", 0), "cooldown_frames": kw.pop("cool", 0)}
        extra = {k: v for k, v in kw.items() if v is not None or k in ("source_duration_seconds",)}
        cases.append({"input": {**args, **extra}, **call(
            latent.calculate_minimax_h3_timing, args["timeline_start_seconds"], args["timeline_end_seconds"],
            args["warmup_frames"], args["cooldown_frames"], **extra)})

    for start, length, warm, cool, src, pad in itertools.product(
            (0.0, 12.04, 200.0), (3.5, 6.0, 12.5), (0, 9, 25), (0, 13), (None, 215.0), (False, True)):
        kw = {"start": start, "end": round(start + length, 6), "warm": warm, "cool": cool, "pad_warmup": pad}
        if src is not None:
            kw["source_duration_seconds"] = src
        add(**kw)
    # Edges: float boundaries, explicit source offsets, long scenes, clamped handles, errors.
    add(start=79.96000000000001, end=86.0, warm=9, cool=9, source_duration_seconds=86.0)
    add(start=0.0, end=0.04, source_duration_seconds=215.0)
    add(start=0.0, end=0.25, warm=5, cool=5)
    add(start=3.0, end=9.0, warm=9, source_start_seconds=0.0, source_duration_seconds=6.0)
    add(start=3.0, end=9.0, warm=9, cool=9, source_start_seconds=3.0, source_duration_seconds=9.2)
    add(start=0.0, end=14.999, warm=13, cool=13)
    add(start=0.0, end=15.1, warm=0, cool=0)
    add(start=0.0, end=30.0, warm=0, cool=0)
    add(start=0.0, end=15.0, warm=0, cool=13)
    add(start=0.0, end=14.5, warm=0, cool=0)
    add(start=100.0, end=100.0)
    add(start=100.0, end=99.0)
    add(start=-1.0, end=5.0)
    add(start=210.0, end=220.0, source_duration_seconds=215.0)
    add(start=0.0, end=5.0, warm=-1)
    add(start=0.0, end=5.0, warm=2.5)
    add(start="abc", end=5.0)
    return cases


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "core")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    pack_env.install(args.pack)
    latent = importlib.import_module("vrgdg_pack.minimax.latent_manager")
    resolution = importlib.import_module("vrgdg_pack.minimax.resolution")
    tile_plan = importlib.import_module("vrgdg_pack.minimax.tile_plan")
    payload_mod = importlib.import_module("vrgdg_pack.minimax.settings_payload")

    # --- timing -------------------------------------------------------------------------------------------------
    write(args.out, "timing", "calculate_minimax_h3_timing", timing_cases(latent))

    cases = [{"input": {"frame_count": n}, **call(latent.align_h3_frame_count, n)} for n in (0, 1, 4, 5, 6, 21, 22, 38, 39, 100, 144, 145, 361, 362, 363)]
    cases += [{"input": {"frame_count": -1}, **call(latent.align_h3_frame_count, -1)}]
    write(args.out, "align_frame_count", "align_h3_frame_count", cases)

    cases = [{"input": {"duration": d, "fps": 24}, **call(latent.frames_covering_duration, d, 24)}
             for d in (0, 0.01, 0.04, 0.0417, 1, 3.5, 6.0, 6.04, 14.999, 15.0, 15.1)]
    write(args.out, "frames_covering_duration", "frames_covering_duration", cases)

    cases = [{"input": {"tokens": n}, **call(latent._tokens_to_frames, n)} for n in list(range(0, 31)) + [49, 62, 97, 100, 200]]
    write(args.out, "tokens_to_frames", "_tokens_to_frames", cases)
    cases = [{"input": {"frames": n}, **call(latent._frames_to_tokens, n)} for n in list(range(0, 30)) + [97, 121, 145, 169, 193, 241, 289, 337, 361]]
    write(args.out, "frames_to_tokens", "_frames_to_tokens", cases)

    # --- masked continuation window ---------------------------------------------------------------------------------
    cases = []
    for frames, pad, ctx in itertools.product((97, 145, 169, 241, 337, 361), (None, 0, 9, 13, 16), (39, 50, 90, 141, 192)):
        tokens = latent._frames_to_tokens(frames)
        cases.append({"input": {"total_tokens": tokens, "tail_padding_frames": pad, "context_frames": ctx},
                      **call(latent.plan_masked_context, tokens, pad, ctx)})
    cases += [{"input": {"total_tokens": t, "tail_padding_frames": 9, "context_frames": 39},
               **call(latent.plan_masked_context, t, 9, 39)} for t in (0, 1, 5, 12, 13, 14)]
    write(args.out, "masked_context", "plan_masked_context", cases)

    # --- resolution -----------------------------------------------------------------------------------------------
    cases = [{"input": {"megapixels": mp, "aspect_ratio": a}, **call(resolution.frame_size, mp, a)}
             for a in ASPECTS for mp in (0.1, 0.25, 0.5625, 0.9, 1.2, 2.0, 4.0, 16.0)]
    cases += [{"input": {"megapixels": 1.0, "aspect_ratio": a}, **call(resolution.frame_size, 1.0, a)} for a in ("", "bad", "0:0", "16:9")]
    write(args.out, "frame_size", "frame_size", cases)

    cases = [{"input": {"preset": p, "aspect_ratio": a}, **call(resolution.preset_megapixels, p, a)}
             for a in ASPECTS for p in ("custom", "1k", "2k", "1440p", "4k", "bogus")]
    write(args.out, "preset_megapixels", "preset_megapixels", cases)

    raws = [
        {}, {"resolution_preset": "2k"}, {"resolution_preset": "custom", "megapixels": 1.5}, {"megapixels": 0.9},
        {"megapixels": 2.2}, {"megapixels": "0.5"}, {"megapixels": 99}, {"megapixels": 0.01},
        {"two_pass_final_width": 1920, "two_pass_final_height": 1088},
        {"two_pass_final_width": 1280, "two_pass_final_height": 720, "aspect_ratio": "16:9 (Widescreen)"},
        {"advanced_two_pass_pass2_megapixels": 2.07}, {"advanced_two_pass_pass2_resolution_preset": "2k"},
        {"advanced_two_pass_pass2_resolution_preset": "bogus", "advanced_two_pass_pass2_megapixels": 3.0},
        {"resolution_preset": "1k", "aspect_ratio": "9:16 (Portrait Widescreen)"},
        {"megapixels": 1.0, "aspect_ratio": "21:9 (Ultrawide)"},
    ]
    cases = [{"input": {"raw": raw, "render_pass": rp}, **call(resolution.migrate_resolution, raw, rp)}
             for raw in raws for rp in ("single", "two_pass", "three_pass")]
    write(args.out, "migrate_resolution", "migrate_resolution", cases)

    # --- tile plan -------------------------------------------------------------------------------------------------
    sizes = [(768, 448), (1024, 1024), (1280, 704), (1088, 1920), (1920, 1088), (2560, 1440), (2048, 864), (3840, 2176)]
    cases = [{"input": {"width": w, "height": h, "vram_preset": p}, **call(tile_plan.plan_spatial_tiles, w, h, p)}
             for (w, h) in sizes for p in ("8gb", "12gb", "16gb", "24gb")]
    # plan_spatial_tiles expects a normalized preset: retired and unknown names raise.
    cases += [{"input": {"width": 1920, "height": 1088, "vram_preset": p}, **call(tile_plan.plan_spatial_tiles, 1920, 1088, p)}
              for p in ("32gb", "custom", "", "bogus")]
    write(args.out, "tile_plan", "plan_spatial_tiles", cases)
    cases = [{"input": {"total_px": t, "count": c, "base_overlap_px": o}, **call(tile_plan.solve_equal_tiles, t, c, o)}
             for t in (704, 1088, 1280, 1920, 2560) for c in (1, 2, 3, 4) for o in (64, 128, 160)]
    write(args.out, "solve_equal_tiles", "solve_equal_tiles", cases)
    cases = [{"input": {"value": v}, **call(tile_plan.normalize_vram_preset, v)} for v in (None, "", "8GB", "16gb", "32gb", "custom", "99gb")]
    write(args.out, "normalize_vram_preset", "normalize_vram_preset", cases)

    # --- settings ---------------------------------------------------------------------------------------------------
    raw_settings = [
        {}, {"render_pass": "two_pass"}, {"render_pass": "single", "steps": "12"}, {"steps": 3.7}, {"steps": 9999}, {"steps": -4},
        {"denoise": "0.5"}, {"seed": "42"}, {"seed": -1}, {"seed": "abc"}, {"use_te_speed": "true"}, {"use_te_speed": "false"},
        {"use_te_speed": 1}, {"use_loras": True, "lora_count": 2, "loras": [{"name": "a.safetensors", "strength": 0.5}]},
        {"loras": [{"name": "a"}] * 5}, {"loras": [{"strength": 1}]}, {"loras": "bad"},
        {"video_mode": "image_to_video"}, {"video_mode": "image_to_video", "render_pass": "two_pass"},
        {"video_mode": "image_to_video", "render_pass": "two_pass", "i2v_pass_settings_version": 1},
        {"video_mode": "image_to_video", "render_pass": "three_pass", "i2v_pass_settings_version": 1},
        {"video_mode": "reference_to_video", "render_pass": "three_pass"},
        {"video_mode": "text_to_video", "render_pass": "two_pass"}, {"video_mode": "bogus"},
        {"continuity_mode": "latent_continuation_masked"}, {"continuity_mode": "latent_continuation"}, {"continuity_mode": "bogus"},
        {"continuity_mode": "latent_continuation_masked", "latent_context_frames": 90},
        {"continuity_mode": "latent_continuation_masked", "latent_context_frames": 50},
        {"latent_context_frames": 16}, {"latent_context_frames": 192},
        {"audio_mode": "built_in_audio"}, {"audio_mode": "bogus"},
        {"advanced_two_pass_vram_preset": "32gb"}, {"advanced_two_pass_vram_preset": "bogus"},
        {"aspect_ratio": "9:16 (Portrait Widescreen)", "resolution_preset": "2k"}, {"aspect_ratio": "bogus"},
        {"megapixels": 1.4}, {"two_pass_final_width": 1920, "two_pass_final_height": 1088},
        {"use_turbo_lora": True}, {"use_turbo_lora": True, "use_loras": True},
        {"sage_attention": "auto"}, {"sage_attention": "bogus"}, {"unknown_setting": 5}, {"two_pass_pass1_steps": 0},
        {"pipeline": "refmod"}, {"pipeline": "refmod", "render_pass": "three_pass"}, {"pipeline": "bogus"},
        {"warmup_frames": 9, "cooldown_frames": 9}, {"warmup_frames": -1}, {"easy_cache_reuse_threshold": 2},
        {"two_pass_use_te_speed": True, "two_pass_te_speed_cache_depth": 3},
    ]
    cases = [{"input": {"raw": raw}, **call(payload_mod.normalize_minimax_h3_settings, raw)} for raw in raw_settings]
    cases.append({"input": {"raw": None}, **call(payload_mod.normalize_minimax_h3_settings, None)})
    write(args.out, "settings_normalize", "normalize_minimax_h3_settings", cases)

    patches = [
        {}, {"steps": 12}, {"steps": "x"}, {"steps": 9999}, {"unknown": 1}, {"render_pass": "bogus"}, {"render_pass": "two_pass"},
        {"loras": [{"name": "a"}]}, {"loras": [{"name": ""}]}, {"loras": [{"name": "a", "apply_to": "bogus"}]},
        {"loras": [{"name": "a"}] * 5}, {"latent_context_frames": 77}, {"latent_context_frames": 90},
        {"continuity_mode": "latent_continuation_masked"}, {"seed": -1}, {"megapixels": 0}, {"aspect_ratio": "7:5 (Odd)"},
        {"advanced_two_pass_pass2_megapixels": 2.0}, {"advanced_two_pass_pass1_steps": 4},
    ]
    cases = [{"input": {"patch": p}, **call(payload_mod.validate_minimax_h3_patch, p)} for p in patches]
    write(args.out, "settings_validate_patch", "validate_minimax_h3_patch", cases)

    # --- settings to render payload -------------------------------------------------------------------------------------
    cases = []
    for mode, rp, audio, cont in itertools.product(
            ("reference_to_video", "image_to_video", "image_reference_to_video", "text_to_video"),
            ("single", "two_pass", "three_pass"), ("input_audio", "built_in_audio"), ("off", "latent_continuation_masked")):
        raw = {"video_mode": mode, "render_pass": rp, "audio_mode": audio, "continuity_mode": cont, "i2v_pass_settings_version": 1,
               "seed": 1234, "two_pass_pass1_seed": 1234, "two_pass_pass2_seed": 5678,
               "advanced_two_pass_pass1_seed": 1234, "advanced_two_pass_pass2_seed": 5678}

        def build(raw=raw):
            settings = payload_mod.normalize_minimax_h3_settings(raw)
            return {"workflow_key": payload_mod.minimax_workflow_key(settings), "render_pass": payload_mod.minimax_render_pass(settings),
                    "payload": payload_mod.build_minimax_render_payload(settings)}

        cases.append({"input": {"raw": raw}, **call(build)})
    write(args.out, "settings_to_payload", "build_minimax_render_payload", cases)

    # Scene lock: a segment with its own settings replaces the project's.
    session = {"minimax_h3_settings": {"steps": 20, "render_pass": "two_pass"}}
    segments = [
        None, {}, {"use_scene_minimax_h3_settings": False, "minimax_h3_settings": {"steps": 8}},
        {"use_scene_minimax_h3_settings": True, "minimax_h3_settings": {"steps": 8}},
        {"use_scene_minimax_h3_settings": True, "minimax_h3_settings": {"continuity_mode": "latent_continuation_masked"}},
        {"use_scene_minimax_h3_settings": True, "minimax_h3_settings": "bad"},
    ]
    cases = [{"input": {"session": session, "segment": seg}, **call(payload_mod.minimax_h3_settings_for_scene, session, seg)} for seg in segments]
    write(args.out, "settings_for_scene", "minimax_h3_settings_for_scene", cases)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
