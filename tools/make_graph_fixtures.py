"""Dev tool: build golden MiniMax H3 graphs with the pack's Python code and save them for the Rust tests.

    python tools/make_graph_fixtures.py --pack <pack folder>

For each case in ``cases()``: settings overrides -> normalized settings -> render payload -> graph, the same
path ``agent_api/orchestrator/video_orchestrator.py`` takes. Files are written to ``fixtures/graphs/<name>.json``.
Paths are fixed fake paths (see pack_env.py) so output is identical on every run. ffprobe and ffmpeg are replaced
by recorders: the probed duration is a constant and the audio trim returns the planned clip.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

PROJECT = pack_env.FAKE_ROOT + "\\Projects\\Demo Song"
SONG = pack_env.FAKE_ROOT + "\\Projects\\Demo Song\\song.wav"
SONG_SECONDS = 215.0
IMAGES = [pack_env.FAKE_ROOT + "\\Projects\\Demo Song\\refs\\" + n for n in ("singer.png", "street.png", "band.png")]
START_FRAME = pack_env.FAKE_ROOT + "\\Projects\\Demo Song\\zimage_approved\\image_0003.png"
LAST_FRAME = pack_env.FAKE_ROOT + "\\Projects\\Demo Song\\zimage_approved\\image_0004.png"
PROMPT = "A singer walks down a neon street at night, <Subject 1> sings <d>[English] Hold on tight.</d> in sync with <Audio 1>."
# A six-section prompt with shot times and lyric cues, for the chunked second pass. Shots 2 and 3 read the same once the
# cues are removed, so the pass 2 prompt merges them.
LONG_PROMPT = (
    "subject_definitions:\n"
    "<Subject 1> is Gabriel from the <Picture 1> character reference.\n\n"
    "<Audio 1> is the complete synchronized song and vocal track.\n\n"
    "detailed_description:\n"
    "The target video is a gritty, photorealistic cinematic music video.\n\n"
    "[Shot 1] A continuous medium close-up of <Subject 1>'s face: <Subject 1> sings in sync with the vocals in "
    "<Audio 1> from 1.0 s to 4.44 s, <d>[English, somber, soft singing] My <i>brother's</i> gone quiet <breath> the "
    "<i>throne</i> stands bare</d>, delivered softly with small natural lip and jaw movements.\n\n"
    "[Shot 2] At 00:04.600, continuing directly from the previous shot's final moment, <Subject 1> sings in sync "
    "with the vocals in <Audio 1> from 4.76 s to 5.76 s, <d>[English, quiet] I breathe the <i>air</i>.</d>, "
    "delivered softly.\n\n"
    "[Shot 3] At 00:06.000, continuing directly from the previous shot's final moment, <Subject 1> sings in sync "
    "with the vocals in <Audio 1> from 6.0 s to 7.5 s, <d>[English, quiet] Hold on tight.</d>, delivered softly.\n\n"
    "[Shot 4] At 00:08.000, the camera slowly pulls back to a wide shot of the empty hall while <Subject 1> hums.\n\n"
    "overall_soundscape:\n"
    "<Audio 1> remains the sole complete soundtrack.\n"
)


def scene(number=1, start=0.0, end=6.0, **extra):
    """Scene-level inputs the orchestrator adds to the settings payload."""
    value = {
        "scene_number": number,
        "start": start,
        "end": end,
        "duration": end - start,
        "audio_path": SONG,
        "prompt": PROMPT,
        "image_paths": list(IMAGES[:2]),
        "video_references": [],
        "continuity_mode": "off",
    }
    value.update(extra)
    return value


def cases():
    """Return ``(name, settings_overrides, scene_inputs)`` for every fixture."""
    out = []

    def add(name, settings=None, scene_inputs=None, mode_override=None):
        out.append((name, dict(settings or {}), scene_inputs if scene_inputs is not None else scene(), mode_override))

    # Every pass gets a fixed seed. A seed of -1 means "random" in the pack and cannot be a golden value.
    seeds = {"seed": 1234, "two_pass_pass1_seed": 1234, "two_pass_pass2_seed": 5678,
             "advanced_two_pass_pass1_seed": 1234, "advanced_two_pass_pass2_seed": 5678}
    base = {"video_mode": "reference_to_video", **seeds}
    # Render passes, reference to video, input audio.
    # In the pack the saved render_pass "three_pass" is the UI's "2 Pass Advanced" (tile plan graph).
    for render_pass in ("single", "two_pass", "three_pass"):
        add(f"r2v_{render_pass}_scene1", {**base, "render_pass": render_pass})
    for preset in ("8gb", "12gb", "16gb", "24gb"):
        add(f"r2v_advanced_{preset}", {**base, "render_pass": "three_pass", "advanced_two_pass_vram_preset": preset})
    # The older fixed 3 pass graph is only reachable by naming the mode directly.
    add("legacy_3pass_builder", {**base, "render_pass": "three_pass"}, scene(), mode_override="minimax_h3_3pass")
    # Resolution and aspect ratio.
    for ratio in ("9:16 (Portrait Widescreen)", "1:1 (Square)", "21:9 (Ultrawide)", "4:3 (Standard)"):
        key = ratio.split(" ")[0].replace(":", "x")
        add(f"r2v_two_pass_ratio_{key}", {**base, "render_pass": "two_pass", "aspect_ratio": ratio})
    for preset in ("custom", "1k", "2k", "1440p", "4k"):
        add(f"r2v_two_pass_res_{preset}", {**base, "render_pass": "two_pass", "resolution_preset": preset, "megapixels": 1.2})
    # Reference image counts.
    for count in (1, 3, 9):
        add(f"r2v_two_pass_images_{count}", {**base, "render_pass": "two_pass"},
            scene(image_paths=[IMAGES[i % 3] for i in range(count)]))
    # Timing: scene position, warm-up and cool-down frames.
    for start, end in ((0.0, 3.5), (12.0, 21.5), (200.0, 215.0)):
        add(f"r2v_two_pass_time_{int(start)}_{int(end * 10)}", {**base, "render_pass": "two_pass"},
            scene(number=4, start=start, end=end))
    for warm, cool in ((0, 0), (9, 0), (0, 9), (13, 13), (25, 5)):
        add(f"r2v_two_pass_warm{warm}_cool{cool}", {**base, "render_pass": "two_pass", "warmup_frames": warm,
                                                       "cooldown_frames": cool}, scene(number=2, start=6.0, end=12.0))
    # Latent Continuation Masked.
    for passes in ("single", "two_pass"):
        for frames in (39, 90, 141, 192):
            settings = {**base, "render_pass": passes, "continuity_mode": "latent_continuation_masked",
                        "latent_context_frames": frames}
            add(f"r2v_{passes}_masked_ctx{frames}_scene1", settings, scene(continuity_mode="latent_continuation_masked"))
            add(f"r2v_{passes}_masked_ctx{frames}_scene3", settings,
                scene(number=3, start=12.0, end=18.0, continuity_mode="latent_continuation_masked",
                      predecessor_latent_frames=337 if frames == 192 else 169))
    for frames, padding in ((97, 0), (241, 9), (337, 13)):
        add(f"r2v_two_pass_masked_pred{frames}_pad{padding}",
            {**base, "render_pass": "two_pass", "continuity_mode": "latent_continuation_masked", "latent_context_frames": 90},
            scene(number=3, start=12.0, end=18.0, continuity_mode="latent_continuation_masked",
                  predecessor_latent_frames=frames, predecessor_tail_padding=padding))
    add("error_masked_ctx192_short_predecessor",
        {**base, "render_pass": "two_pass", "continuity_mode": "latent_continuation_masked", "latent_context_frames": 192},
        scene(number=3, start=12.0, end=18.0, continuity_mode="latent_continuation_masked"))
    add("error_advanced_masked_scene3",
        {**base, "render_pass": "three_pass", "continuity_mode": "latent_continuation_masked"},
        scene(number=3, start=12.0, end=18.0, continuity_mode="latent_continuation_masked"))
    # Samplers, steps and denoise.
    for sampler, scheduler in (("euler", "simple"), ("euler_ancestral", "beta"), ("dpmpp_2m", "karras")):
        add(f"r2v_single_sampler_{sampler}", {**base, "render_pass": "single", "sampler_name": sampler,
                                               "scheduler": scheduler, "steps": 24})
    add("r2v_two_pass_custom_pass_settings", {
        **base, "render_pass": "two_pass", "two_pass_pass1_steps": 12, "two_pass_pass2_steps": 6,
        "two_pass_pass1_denoise": 1.0, "two_pass_pass2_denoise": 0.5, "two_pass_pass1_seed": 11, "two_pass_pass2_seed": 22,
        "two_pass_pass1_megapixels": 0.35, "two_pass_pass2_megapixels": 1.1,
    })
    # LoRAs, turbo, speed options.
    lora = {"name": "style\\film_look.safetensors", "strength": 0.7}
    add("r2v_single_lora_one", {**base, "render_pass": "single", "use_loras": True, "lora_count": 1, "loras": [lora]})
    add("r2v_single_lora_two", {**base, "render_pass": "single", "use_loras": True, "lora_count": 2,
                                "loras": [lora, {"name": "style\\soft_light.safetensors", "strength": 0.4}]})
    add("r2v_single_turbo", {**base, "render_pass": "single", "use_turbo_lora": True, "steps": 4})
    add("r2v_two_pass_lora", {**base, "render_pass": "two_pass", "use_loras": True, "lora_count": 1, "loras": [lora]})
    add("r2v_two_pass_lora_strength", {**base, "render_pass": "two_pass", "two_pass_lora_strength": 0.6})
    add("legacy_3pass_lightx_strength", {**base, "render_pass": "three_pass", "three_pass_lightx_lora_strength": 0.8}, scene(), mode_override="minimax_h3_3pass")
    for key in ("use_te_speed", "use_fast_vae_decode", "use_feedforward", "use_block_sparse_attention"):
        add(f"r2v_single_{key}", {**base, "render_pass": "single", key: True})
        two_key = "two_pass_" + key.replace("use_", "use_", 1)
        add(f"r2v_two_pass_{key}", {**base, "render_pass": "two_pass", two_key: True, key: True})
    # Sigma shift (ModelSamplingMiniMaxH3): Single, each pass of 2 Pass, and 2 Pass Advanced, which ignores it.
    sigma = {"sigma_pass1_enabled": True, "sigma_pass1_video": 8.5, "sigma_pass1_audio": 1.25}
    add("r2v_single_sigma", {**base, "render_pass": "single", **sigma})
    add("r2v_two_pass_sigma_pass1", {**base, "render_pass": "two_pass", **sigma})
    add("r2v_two_pass_sigma_pass2", {**base, "render_pass": "two_pass", "sigma_pass2_enabled": True,
                                     "sigma_pass2_video": 5, "sigma_pass2_audio": 2})
    add("r2v_two_pass_sigma_both", {**base, "render_pass": "two_pass", **sigma, "sigma_pass2_enabled": True,
                                    "sigma_pass2_video": 5, "sigma_pass2_audio": 2})
    add("r2v_advanced_sigma_ignored", {**base, "render_pass": "three_pass", **sigma})
    add("r2v_single_sigma_with_feedforward_and_lora", {
        **base, "render_pass": "single", **sigma, "use_feedforward": True, "use_loras": True, "lora_count": 1, "loras": [lora]})
    # DMAD: the settings the Builder's DMAD button fills in (set_minimax_h3_dmad), compiled like any other settings.
    # The compiler knows nothing about DMAD, only about the LoRAs, sampler and sigma shift that button sets.
    import importlib

    set_dmad = importlib.import_module("vrgdg_pack.minimax.settings_payload").set_minimax_h3_dmad
    for render_pass in ("single", "two_pass"):
        add(f"r2v_{render_pass}_dmad", set_dmad({**base, "render_pass": render_pass}, True))
    add("r2v_two_pass_dmad_with_extra_lora", set_dmad({**base, "render_pass": "two_pass", "use_loras": True, "lora_count": 1,
                                                       "loras": [{**lora, "apply_to": "both"}]}, True))
    add("r2v_single_sage_auto", {**base, "render_pass": "single", "sage_attention": "auto"})
    add("r2v_single_sage_memory_efficient", {**base, "render_pass": "single", "use_memory_efficient_sage_attention": True})
    add("r2v_single_easy_cache", {**base, "render_pass": "single", "easy_cache_bypass": False, "easy_cache_reuse_threshold": 0.3})
    # Built-in audio.
    for render_pass in ("single", "two_pass"):
        add(f"r2v_{render_pass}_built_in_audio", {**base, "render_pass": render_pass, "audio_mode": "built_in_audio"})
    # Image to video, first/last frame and image + reference.
    one_image = scene(image_paths=[START_FRAME])
    flf = scene(image_paths=[START_FRAME], last_frame_path=LAST_FRAME)
    for render_pass in ("single", "two_pass"):
        i2v = {"video_mode": "image_to_video", **seeds, "render_pass": render_pass, "i2v_pass_settings_version": 1}
        add(f"i2v_{render_pass}_normal", i2v, one_image)
        add(f"i2v_{render_pass}_flf", i2v, flf)
        add(f"i2v_{render_pass}_scene3_warm", {**i2v, "warmup_frames": 9}, {**one_image, "scene_number": 3, "start": 12.0, "end": 18.0})
    # Older I2V saves (no i2v_pass_settings_version) are migrated to Single by normalize_minimax_h3_settings.
    add("i2v_legacy_save_migrates_to_single", {"video_mode": "image_to_video", **seeds, "render_pass": "two_pass"}, one_image)
    add("ir2v_single_with_references", {"video_mode": "image_reference_to_video", **seeds, "render_pass": "single"},
        scene(image_paths=[START_FRAME, IMAGES[0], IMAGES[1]]))
    # Text to video: Single and 2 Pass, with supplied or built-in audio. Saves from version 1 on opt in to the pass.
    no_references = scene(image_paths=[])
    t2v = {"video_mode": "text_to_video", **seeds, "t2v_pass_settings_version": 1}
    for render_pass in ("single", "two_pass"):
        for audio_mode in ("input_audio", "built_in_audio"):
            add(f"t2v_{render_pass}_{audio_mode}", {**t2v, "render_pass": render_pass, "audio_mode": audio_mode}, no_references)
    add("t2v_legacy_save_migrates_to_single", {"video_mode": "text_to_video", **seeds, "render_pass": "two_pass"}, no_references)
    # Built-in audio in the multi-pass modes that used to require supplied audio.
    add("i2v_two_pass_built_in_audio", {"video_mode": "image_to_video", **seeds, "render_pass": "two_pass", "i2v_pass_settings_version": 1,
                                        "audio_mode": "built_in_audio"}, one_image)
    add("r2v_advanced_built_in_audio", {**base, "render_pass": "three_pass", "audio_mode": "built_in_audio"})
    # Pass 2 in windows (pass2_chunking): a rendered clip longer than the limit swaps its second pass for the chunk node.
    long_scene = scene(number=1, start=0.0, end=12.0, prompt=LONG_PROMPT)
    two = {**base, "render_pass": "two_pass"}
    add("r2v_two_pass_chunked_long", two, long_scene)
    add("r2v_two_pass_chunked_limit3", {**two, "pass2_chunking_min_seconds": 3}, scene(number=1, start=0.0, end=5.0, prompt=LONG_PROMPT))
    add("r2v_two_pass_chunked_limit_not_met", {**two, "pass2_chunking_min_seconds": 6.5}, scene(number=1, start=0.0, end=5.0, prompt=LONG_PROMPT))
    add("r2v_two_pass_chunking_off_long", {**two, "pass2_chunking": "off"}, long_scene)
    add("r2v_two_pass_chunked_masked_scene3", {**two, "continuity_mode": "latent_continuation_masked", "latent_context_frames": 90},
        scene(number=3, start=12.0, end=21.0, continuity_mode="latent_continuation_masked", prompt=LONG_PROMPT))
    add("r2v_two_pass_chunked_built_in_audio", {**two, "audio_mode": "built_in_audio"}, long_scene)
    add("r2v_two_pass_chunked_dmad", set_dmad({**two, "use_loras": True, "lora_count": 1, "loras": [{**lora, "apply_to": "both"}]}, True), long_scene)
    add("r2v_two_pass_chunked_sigma_both", {**two, **sigma, "sigma_pass2_enabled": True, "sigma_pass2_video": 5, "sigma_pass2_audio": 2}, long_scene)
    add("r2v_advanced_long_not_chunked", {**base, "render_pass": "three_pass"}, long_scene)
    # The nvidia-vfx library is not installed ("_no_rtx_vsr" cases run with nvidia_vsr_available() answering False): the final resize
    # that would be nvidia_rtx_vsr is lanczos instead (2 Pass Advanced has no such resize). Every other case runs as if the library were installed.
    add("r2v_two_pass_no_rtx_vsr", two)
    add("r2v_two_pass_chunked_no_rtx_vsr", two, long_scene)
    add("i2v_two_pass_no_rtx_vsr", {"video_mode": "image_to_video", **seeds, "render_pass": "two_pass", "i2v_pass_settings_version": 1}, one_image)
    add("t2v_two_pass_chunked_long", {**t2v, "render_pass": "two_pass"}, scene(number=1, start=0.0, end=12.0, prompt=LONG_PROMPT, image_paths=[]))
    add("i2v_two_pass_chunked_unsupported", {"video_mode": "image_to_video", **seeds, "render_pass": "two_pass", "i2v_pass_settings_version": 1},
        scene(image_paths=[START_FRAME], start=0.0, end=12.0))
    # Scene prompt, timing and input edge cases that must error.
    add("error_empty_prompt", {**base, "render_pass": "single"}, scene(prompt=""))
    add("error_unknown_model", {**base, "render_pass": "single", "diffusion_model_name": "missing_model.safetensors"})
    add("error_bad_aspect", {**base, "render_pass": "single", "aspect_ratio": "7:5 (Odd)"})
    add("error_i2v_without_image", {"video_mode": "image_to_video", "render_pass": "single"}, scene(image_paths=[]))
    return out


def walk_strings(value, fn):
    """Return a copy of ``value`` with ``fn`` applied to every string."""
    if isinstance(value, dict):
        return {k: walk_strings(v, fn) for k, v in value.items()}
    if isinstance(value, list):
        return [walk_strings(v, fn) for v in value]
    return fn(value) if isinstance(value, str) else value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "graphs")
    parser.add_argument("--only", default="", help="Only cases whose name contains this text.")
    args = parser.parse_args()

    pack = pack_env.install(args.pack)
    import importlib

    payload_mod = importlib.import_module("vrgdg_pack.minimax.settings_payload")
    workflows = importlib.import_module("vrgdg_pack.runner.minimax_workflows")
    api_graph = importlib.import_module("vrgdg_pack.runner.api_graph")

    recorded = {}
    # Whether the pack finds nvidia-vfx depends on the machine, so each case says it (see "_no_rtx_vsr" in cases()).
    vsr = {"installed": True}
    api_graph.nvidia_vsr_available = lambda: vsr["installed"]

    def fake_probe(path):
        recorded.setdefault("probe", []).append(path)
        return SONG_SECONDS

    def fake_trim(source_path, project_folder, scene_number, timing):
        target = f"{project_folder}\\minimax_h3_scene_audio\\scene_audio_{scene_number:04d}.wav"
        info = {"audio_path": target, "start": timing.audio_trim_start_seconds, "duration": timing.audio_trim_duration_seconds,
                "requested_duration": timing.audio_trim_duration_seconds, "format": "pcm_s16le_wav"}
        recorded["trim"] = {"source": source_path, "project_folder": project_folder, "scene_number": scene_number,
                            "start": timing.audio_trim_start_seconds, "duration": timing.audio_trim_duration_seconds,
                            "leading_padding": timing.audio_leading_padding_seconds}
        return info

    latent_mod = importlib.import_module("vrgdg_pack.minimax.latent_manager")
    pred = {"frames": 169, "tail_padding": 9}

    def fake_latent_info(cls, project_folder, scene_number):
        """Predecessor latent header as the sidecar JSON would report it, without a file on disk."""
        tokens = latent_mod._frames_to_tokens(pred["frames"])
        return {"ok": True, "exists": True, "path": cls.get_path(project_folder, scene_number), "scene_number": scene_number,
                "size_bytes": 1, "frame_count": pred["frames"], "token_count": tokens, "fps": 24.0, "timestamp": 0.0,
                "tail_padding_frames": pred["tail_padding"], "dirty": False}

    latent_mod.SceneLatentManager.get_latent_info = classmethod(fake_latent_info)
    latent_mod.SceneLatentManager.is_dirty = classmethod(lambda cls, project_folder, scene_number: False)
    workflows._probe_media_duration_seconds = fake_probe
    workflows._trim_minimax_h3_audio_context = fake_trim

    args.out.mkdir(parents=True, exist_ok=True)
    for stale in args.out.glob("*.json"):
        stale.unlink()

    ok = failed = 0
    for name, overrides, scene_inputs, mode_override in cases():
        if args.only and args.only not in name:
            continue
        recorded.clear()
        pred["frames"] = int(scene_inputs.get("predecessor_latent_frames", 169))
        pred["tail_padding"] = int(scene_inputs.get("predecessor_tail_padding", 9))
        record = {"name": name, "settings_overrides": overrides, "scene": scene_inputs, "project_folder": PROJECT}
        vsr["installed"] = not name.endswith("_no_rtx_vsr")
        if not vsr["installed"]:
            record["nvidia_vsr_available"] = False
        try:
            settings = payload_mod.normalize_minimax_h3_settings(overrides)
            record["settings"] = settings
            mode = mode_override or payload_mod.minimax_workflow_key(settings)
            payload = payload_mod.build_minimax_render_payload(settings)
            payload.update(scene_inputs)
            payload["project_folder"] = PROJECT
            payload["i2v_prompt"] = payload["t2v_prompt"] = scene_inputs["prompt"]
            record["mode"] = mode
            record["payload"] = payload
            if mode == "minimax_h3_3pass":
                result = workflows._build_minimax_h3_3pass_api_prompt(payload)
            elif mode == "minimax_h3_advanced_2pass":
                result = workflows._build_minimax_h3_advanced_2pass_api_prompt(payload)
            elif mode == "minimax_h3_2pass":
                result = workflows._build_minimax_h3_2pass_api_prompt(payload)
            else:
                result = workflows._build_minimax_h3_api_prompt(payload)
            result["workflow_path"] = Path(result["workflow_path"]).name
            record["result"] = result
            record["audio_trim_call"] = recorded.get("trim")
            ok += 1
        except Exception as exc:  # expected for error_* cases, recorded as the fixture
            record["error"] = {"type": type(exc).__name__, "message": str(exc)}
            failed += 1
            if not name.startswith("error_"):
                print(f"UNEXPECTED ERROR {name}: {type(exc).__name__}: {exc}")
        (args.out / f"{name}.json").write_text(
            json.dumps(record, indent=1, sort_keys=True, default=str, ensure_ascii=False), encoding="utf-8")
    # What the fake ComfyUI reports, so the Rust tests answer the same questions the same way.
    env_path = args.out.parent / "graph_env.json"
    env_path.write_text(json.dumps({
        "fake_root": pack_env.FAKE_ROOT, "output_dir": pack_env.FAKE_OUTPUT, "song_seconds": SONG_SECONDS,
        "models": pack_env.MODEL_FILES,
    }, indent=1, sort_keys=True, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {ok + failed} fixtures ({ok} graphs, {failed} errors) to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
