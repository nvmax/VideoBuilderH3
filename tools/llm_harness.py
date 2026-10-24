"""Shared by the ``make_*_llm_fixtures.py`` tools: run the pack's prompt writers with the LLM replaced by a recorder.

The pack's writers build an instruction, call ``_run_builder_text_llm`` or ``_try_run_remote_vision``, then clean and check
the reply. The recorder keeps what they asked (the instruction text, the sampling values, the picture sizes) and answers with
a canned reply, so the instruction text and the handling of replies can be compared with the Rust port without a model.
"""

import base64
import importlib
import io
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

INFO_TEXT = {"runner": "lm_studio", "used_model": "test-model", "unloaded": False}
INFO_VISION = {"runner": "lm_studio_vision", "used_model": "test-model", "unloaded": False}


def png_data_url(width=40, height=30, color=(200, 40, 40)):
    from PIL import Image

    buffer = io.BytesIO()
    Image.new("RGB", (width, height), color).save(buffer, format="PNG")
    return "data:image/png;base64," + base64.b64encode(buffer.getvalue()).decode("ascii")


class Recorder:
    """Stands in for the pack's LLM runner functions. ``replies`` is consumed in order."""

    def __init__(self):
        self.calls = []
        self.replies = []

    def _next(self):
        if not self.replies:
            raise RuntimeError("The recorder ran out of canned replies.")
        return self.replies.pop(0)

    def text(self, payload, instruction, temperature=0.6, top_p=0.95, max_new_tokens=1200, label="Gemma", preserve_paragraphs=False, json_schema=None):
        self.calls.append({"kind": "text", "instruction": instruction, "temperature": temperature, "top_p": top_p, "max_new_tokens": max_new_tokens, "label": label, "preserve_paragraphs": bool(preserve_paragraphs), "json_schema": bool(json_schema)})
        return self._next(), dict(INFO_TEXT)

    def vision(self, payload, instruction, pil_images, temperature=0.25, top_p=0.95, max_new_tokens=1200):
        self.calls.append({"kind": "vision", "instruction": instruction, "temperature": temperature, "top_p": top_p, "max_new_tokens": max_new_tokens, "images": [list(image.size) for image in pil_images]})
        return self._next(), dict(INFO_VISION)


def install(pack: Path, modules):
    """Load the pack and replace the LLM runner functions inside ``modules`` (names under ``vrgdg_pack``)."""
    pack_env.install(pack)
    pack_env.install_server_stub()
    recorder = Recorder()
    loaded = {}
    for name in ["llm.builder_runner"] + list(modules):
        module = importlib.import_module(f"vrgdg_pack.{name}")
        loaded[name] = module
        if hasattr(module, "_run_builder_text_llm"):
            module._run_builder_text_llm = recorder.text
        if hasattr(module, "_try_run_remote_vision"):
            module._try_run_remote_vision = recorder.vision
        if hasattr(module, "_clear_comfy_model_memory"):
            module._clear_comfy_model_memory = lambda: {}
    return recorder, loaded


def run_case(recorder, function, payload, replies):
    """Call a writer with canned replies. Returns ``{"calls": [...], "return": ...}`` or ``{"calls": [...], "error": ...}``."""
    recorder.calls = []
    recorder.replies = list(replies)
    try:
        result = function(json.loads(json.dumps(payload)))
        outcome = {"return": result}
    except Exception as exc:  # noqa: BLE001
        outcome = {"error": str(exc)}
    outcome["calls"] = recorder.calls
    return outcome


def temp_files(files):
    """Create the named text files in a temp folder. Returns the folder object and ``{name: path}``."""
    folder = tempfile.TemporaryDirectory()
    paths = {}
    for name, content in files.items():
        path = Path(folder.name) / name
        path.write_text(content, encoding="utf-8")
        paths[name] = str(path.resolve())
    return folder, paths


def scrub(value, folder):
    """Replace a temp folder's path by ``<TMP>`` in any JSON value."""
    text = json.dumps(value, ensure_ascii=False)
    real = str(Path(folder).resolve())
    text = text.replace(json.dumps(real)[1:-1], "<TMP>").replace(real, "<TMP>")
    return json.loads(text)
