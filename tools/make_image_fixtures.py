"""Dev tool: golden graphs for the built-in image presets (``h3-image``), made by the pack's own patch functions.

    python tools/make_image_fixtures.py --pack <pack folder>

For each built-in preset the pack's ``_patch_*_api_prompt`` fills its template from a payload. Each case here records the
payload, the matching values for the Rust preset bindings, and the graph the pack produced. Writes
``fixtures/image/*.json``. Image to image with a file and random seeds are left to the Rust tests, because they depend on
files and the clock.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

FAKE_REFS = pack_env.FAKE_ROOT + "\\refs\\"
LORAS = [{"name": "style\\film_look.safetensors", "strength": 0.7}, {"name": "style\\soft_light.safetensors", "strength": 1.2}]


def lora_payload(loras):
    payload = {"use_custom_loras": bool(loras), "lora_count": len(loras)}
    for i, lora in enumerate(loras, start=1):
        payload[f"lora_{i}"] = lora["name"]
        payload[f"strength_{i}"] = lora["strength"]
    return payload


def write(out: Path, name: str, cases: list) -> None:
    (out / f"{name}.json").write_text(json.dumps({"preset": name, "cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print(f"{name}: {len(cases)} cases")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "image")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    workflows = importlib.import_module("vrgdg_pack.runner.image_workflows")
    api_graph = importlib.import_module("vrgdg_pack.runner.api_graph")
    # The placeholder image is copied into ComfyUI's input folder, which the fake setup does not have.
    workflows._ensure_placeholder_load_image = lambda: "vrgdg_placeholder_i2i.png"

    def template(path_fn):
        return api_graph._load_api_template(path_fn())[1]

    def zimage_case(payload, loras=()):
        merged = {"prompt": "a cat", "seed": 11, "unet_name": "u.safetensors", "clip_name": "c.safetensors", "vae_name": "v.safetensors", **payload, **lora_payload(list(loras))}
        graph, _seed = workflows._patch_zimage_api_prompt(copy.deepcopy(template(workflows._zimage_api_template_path)), merged)
        values = {k: v for k, v in merged.items() if not k.startswith(("use_custom", "lora_", "strength_"))}
        values["loras"] = list(loras)
        return {"values": values, "payload": merged, "graph": graph}

    def ernie_case(payload, loras=()):
        merged = {"prompt": "a dog", "seed": 5, "unet_name": "u.safetensors", "clip_name": "c.safetensors", "vae_name": "v.safetensors", **payload, **lora_payload(list(loras))}
        graph, _seed = workflows._patch_ernie_image_api_prompt(copy.deepcopy(template(workflows._ernie_image_api_template_path)), merged)
        values = {k: v for k, v in merged.items() if not k.startswith(("use_custom", "lora_", "strength_"))}
        values["loras"] = list(loras)
        return {"values": values, "payload": merged, "graph": graph}

    def flux_case(payload, refs=(), loras=()):
        merged = {"prompt": "a fox", "seed": 21, "unet_name": "u.safetensors", "clip_name": "c.safetensors", "vae_name": "v.safetensors", **payload, **lora_payload(list(loras))}
        merged["image_ingredients"] = [{"path": FAKE_REFS + r} for r in refs]
        graph = workflows._patch_flux_klein_api_prompt(copy.deepcopy(template(workflows._flux_klein_api_template_path)), merged)
        values = {k: v for k, v in merged.items() if not k.startswith(("use_custom", "lora_", "strength_", "image_ingredients"))}
        values["loras"] = list(loras)
        values["reference_images"] = [FAKE_REFS + r for r in refs]
        return {"values": values, "payload": merged, "graph": graph}

    zimage = [
        zimage_case({}),
        zimage_case({"first_pass_width": 1024, "first_pass_height": 576, "second_pass_width": 1600, "second_pass_height": 900, "batch_size": 2}),
        zimage_case({"seed": 123456789012}, LORAS[:1]),
        zimage_case({"prompt": "wide shot", "image_to_image_start_at_step": 3}, LORAS),
        zimage_case({"first_pass_width": 99999, "batch_size": 99}),
    ]
    ernie = [
        ernie_case({}),
        ernie_case({"width": 1920, "height": 1080, "batch_size": 3}),
        ernie_case({"seed": 777}, LORAS),
        ernie_case({"image_to_image_start_at_step": 7, "width": 512, "height": 512}),
    ]
    flux = [
        flux_case({}),
        flux_case({"width": 1280, "height": 720}),
        flux_case({"seed": 99}, refs=["a.png"]),
        flux_case({"width": 768, "height": 768}, refs=["a.png", "b.png", "c.png"], loras=LORAS[:1]),
        flux_case({}, loras=LORAS),
    ]
    write(args.out, "zimage", zimage)
    write(args.out, "ernie", ernie)
    write(args.out, "flux_klein", flux)
    write_builder_models(args.out, workflows, template)
    return 0


def write_builder_models(out: Path, workflows, template) -> None:
    """Krea 2, Krea 2 two pass, MOSS-Image, NanoBanana and Z upscale: the Builder's own request in, the pack's graph out.

    These engines have no preset in ``h3-image``. The Rust builders in ``h3-graph/src/image.rs`` are compared with these.
    The model lists and the picture copy are replaced, because the fake setup has neither."""
    workflows._require_model_choice = lambda *args, **kwargs: None
    workflows._prepare_load_image_name = lambda path="", data="", name="image.png": "vrgdg_i2i_fixed.png" if (path or data) else ""
    lora = {"lora_1": "style\\film_look.safetensors", "strength_1": 0.6, "lora_2": "style\\soft_light.safetensors", "strength_2": 1.1, "use_custom_loras": True, "lora_count": 2}

    def case(builder, payload, **extra):
        merged = {**payload, **extra}
        result = builder(copy.deepcopy(merged))
        return {"payload": merged, "graph": result["prompt"], "used_seed": result.get("used_seed")}

    krea2 = [
        case(workflows._build_krea2_api_prompt, {"prompt": "a lake", "seed": 7}),
        case(workflows._build_krea2_api_prompt, {"prompt": "a lake", "seed": 7, "width": 1280, "height": 720, "first_pass_width": 800, "first_pass_height": 450, "batch_size": 2}),
        case(workflows._build_krea2_api_prompt, {"prompt": "a lake", "seed": 7, "use_zimage_enhance": False}),
        case(workflows._build_krea2_api_prompt, {"prompt": "a lake", "seed": 7, "zimage_enhance_strength": 0.9, "width": 1024, "height": 1024}),
        case(workflows._build_krea2_api_prompt, {"prompt": "a lake", "seed": 7, "width": 720, "height": 1280, "krea_unet_name": "k.safetensors", "z_clip_name": "z.safetensors"}),
    ]
    krea2_2pass = [
        case(workflows._build_krea2_2pass_api_prompt, {"prompt": "a hill", "seed": 3}),
        case(workflows._build_krea2_2pass_api_prompt, {"prompt": "a hill", "seed": 3, "cfg": 9, "sampler_name": "euler", "aspect_ratio": "9:16 (Portrait Widescreen)", "batch_size": 3}),
        case(workflows._build_krea2_2pass_api_prompt, {"prompt": "a hill", "seed": 3, "use_image_to_image": True, "image_to_image_path": FAKE_REFS + "a.png", "image_to_image_creativity": 8}),
        case(workflows._build_krea2_2pass_api_prompt, {"prompt": "a hill", "seed": 3, **lora}),
        case(workflows._build_krea2_2pass_api_prompt, {"prompt": "a hill", "seed": 3, "use_loras": True, "lora_count": 1, "lora_1": "style\\film_look.safetensors", "first_pass_strength_1": 0.4, "second_pass_strength_1": 0.9}),
    ]
    moss = [
        case(workflows._build_moss_image_api_prompt, {"prompt": "a robot", "seed": 5}),
        case(workflows._build_moss_image_api_prompt, {"prompt": "a robot", "seed": 5, "lora_name": "style\\film_look.safetensors", "lora_strength": 0.5, "negative_prompt": "blurry"}),
        case(workflows._build_moss_image_api_prompt, {"prompt": "a robot", "seed": 5, "width": 99999, "height": 8, "steps": 12, "cfg": 3.5, "batch_size": 2}),
    ]
    key = "AIzaSyD-example-key-0123456789abcdef"
    nb = [
        case(workflows._build_nb_image_api_prompt, {"prompt": "a cat in a hat", "api_key": key}),
        case(workflows._build_nb_image_api_prompt, {"prompt": key, "api_key": "a cat in a hat sitting"}),
        case(workflows._build_nb_image_api_prompt, {"prompt": "a cat in a hat", "api_key": key, "model": "gemini-x", "image_ingredients": [{"path": FAKE_REFS + "a.png"}, FAKE_REFS + "b.png"]}),
    ]
    z_upscale = [
        case(workflows._build_z_upscale_enhance_prompt, {"prompt": "sharpen", "seed": 9, "source_image_path": FAKE_REFS + "a.png"}),
        case(workflows._build_z_upscale_enhance_prompt, {"prompt": "sharpen", "seed": 9, "source_image_path": FAKE_REFS + "a.png", "width": 2560, "height": 1440, "enhance_amount": 12, **lora}),
    ]
    for name, cases in (("krea2", krea2), ("krea2_2pass", krea2_2pass), ("moss_image", moss), ("nb_image", nb), ("z_upscale_enhance", z_upscale)):
        write(out, name, cases)


if __name__ == "__main__":
    raise SystemExit(main())
