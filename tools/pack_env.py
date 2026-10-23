"""Dev tool: load the pack's pure and graph-building modules without ComfyUI, read-only.

Used only to generate test fixtures. Nothing here runs inside the app and nothing in the pack is modified.

What it does:
* Registers the pack folder as the package ``vrgdg_pack`` without running its ``__init__.py`` (that file imports
  ComfyUI's server). Relative imports inside the pack then resolve normally.
* Stubs ``folder_paths`` (ComfyUI) with fixed fake folders and a fixed model list.
* Stubs ``nodes`` with ``NODE_CLASS_MAPPINGS`` rebuilt from ``fixtures/object_info_h3.json``, so the graph
  builders see the same node inputs as a real ComfyUI.
* Makes paths under ``FAKE_ROOT`` look like they exist, without touching disk.
"""

import importlib
import json
import os
import sys
import types
from pathlib import Path

FAKE_ROOT = "Z:\\h3fixture"
FAKE_OUTPUT = FAKE_ROOT + "\\comfy\\output"
FAKE_INPUT = FAKE_ROOT + "\\comfy\\input"
FAKE_TEMP = FAKE_ROOT + "\\comfy\\temp"
FAKE_MODELS = FAKE_ROOT + "\\comfy\\models"

TOOLS_DIR = Path(__file__).resolve().parent
OBJECT_INFO_PATH = TOOLS_DIR.parent / "fixtures" / "object_info_h3.json"

# Model files the fake ComfyUI reports as installed. Cases may only use these names.
MODEL_FILES = {
    "diffusion_models": ["minimax_h3_ref2va_pruned_int8_convrot.safetensors", "minimax_h3_fl2va_pruned_int8_convrot.safetensors"],
    "unet": [],
    "text_encoders": ["qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors"],
    "clip": [],
    "vae": ["minimax_h3_video_vae_fp16.safetensors", "minimax_h3_audio_vae_fp32.safetensors"],
    "loras": [
        "minimax_h3_ref2v_turbo_4step_v0.1_comfyui_bf16.safetensors",
        "minimax_h3_turbo_4step_ema_ckpt850.safetensors",
        "minimax_h3_fl2v_turbo_4step_v1.1_768p_comfyui_bf16.safetensors",
        "minimax_h3_fl2v_lightx2v_turbo_4step_v0.1_comfy_resized_avg_rank_21_bf16.safetensors",
        "style\\film_look.safetensors",
        "style\\soft_light.safetensors",
    ],
    "latent_upscale_models": ["minimax_h3_latent_upscaler_3d_bf16.safetensors"],
    "upscale_models": [],
}

_installed = False


def _is_fake(path) -> bool:
    return isinstance(path, (str, os.PathLike)) and os.fspath(path).lower().startswith(FAKE_ROOT.lower())


def _build_folder_paths() -> types.ModuleType:
    module = types.ModuleType("folder_paths")
    module.get_output_directory = lambda: FAKE_OUTPUT
    module.get_input_directory = lambda: FAKE_INPUT
    module.get_temp_directory = lambda: FAKE_TEMP
    module.get_filename_list = lambda category: list(MODEL_FILES.get(category, []))
    module.get_folder_paths = lambda category: []
    module.get_full_path = lambda category, name: FAKE_MODELS + "\\" + category + "\\" + str(name)
    module.folder_names_and_paths = {}
    module.models_dir = FAKE_MODELS
    module.add_model_folder_path = lambda *a, **k: None
    return module


def _build_nodes() -> types.ModuleType:
    info = json.loads(OBJECT_INFO_PATH.read_text(encoding="utf-8"))
    module = types.ModuleType("nodes")
    mappings = {}
    for name, entry in info.items():
        spec = entry.get("input", {})
        mappings[name] = type(name, (), {"INPUT_TYPES": classmethod(lambda cls, spec=spec: spec)})
    module.NODE_CLASS_MAPPINGS = mappings
    return module


def _patch_os() -> None:
    real_isfile, real_isdir, real_makedirs = os.path.isfile, os.path.isdir, os.makedirs

    def isfile(path):
        return True if _is_fake(path) and Path(os.fspath(path)).suffix else real_isfile(path)

    def isdir(path):
        return True if _is_fake(path) and not Path(os.fspath(path)).suffix else real_isdir(path)

    def makedirs(name, mode=0o777, exist_ok=False):
        if _is_fake(name):
            return None
        return real_makedirs(name, mode, exist_ok)

    os.path.isfile, os.path.isdir, os.makedirs = isfile, isdir, makedirs


def install(pack_dir: Path):
    """Install the stubs and return the ``vrgdg_pack`` package for ``pack_dir``."""
    global _installed
    pack_dir = Path(pack_dir).resolve()
    if not _installed:
        sys.modules["folder_paths"] = _build_folder_paths()
        sys.modules["nodes"] = _build_nodes()
        _patch_os()
        package = types.ModuleType("vrgdg_pack")
        package.__path__ = [str(pack_dir)]
        sys.modules["vrgdg_pack"] = package
        _installed = True
    return importlib.import_module("vrgdg_pack")
