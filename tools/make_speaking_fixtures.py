"""Dev tool: golden cases for the Speaking mode (ElevenLabs voices and speech, dialogue, scene audio settings), from the pack's own code.

    python tools/make_speaking_fixtures.py --pack <pack folder>

Writes three files in ``fixtures/speaking``:
  pure.json     the checks, the error texts and the scene audio timing (no network)
  http.json     the ElevenLabs calls with ``urlopen`` replaced by a script of replies, with every request the pack made
  writers.json  the voice description and dialogue helpers with the LLM replaced by a recorder (see ``llm_harness.py``)

No real key or network is used. The key in the cases is a made-up string.
"""

import argparse
import base64
import copy
import importlib
import io
import json
import sys
from pathlib import Path
from urllib.error import HTTPError, URLError

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
OUT = Path(__file__).resolve().parents[1] / "fixtures" / "speaking"
KEY = "test-key-123"
MP3 = base64.b64encode(b"ID3\x04\x00\x00fake mp3 bytes " * 20).decode("ascii")
VOICE = {"enabled": True, "voice_id": "voice_abc-123", "name": "Warm Narrator"}
REFERENCES = {"subjects": [
    {"id": "mara", "name": "Mara", "description": "a woman", "reference_type": "character", "elevenlabs_voice": VOICE},
    {"id": "joe", "name": "Joe", "reference_type": "character", "elevenlabs_voice": {"enabled": False, "voice_id": "", "name": ""}},
    {"id": "extra", "name": "Extra", "reference_type": "character", "extra_reference_for": "mara", "elevenlabs_voice": VOICE},
    {"id": "prop", "name": "Hat", "reference_type": "prop"},
    {"id": "novoice", "name": "Zed"},
]}


def http_error(code, body):
    return HTTPError("https://api.elevenlabs.io/x", code, "error", {}, io.BytesIO(body if isinstance(body, bytes) else str(body).encode("utf-8")))


# -- pure cases ----------------------------------------------------------------------------------------------------------------

def segment(n, start, end, **extra):
    return {"id": f"scene_{n}", "label": f"Scene {n}", "start": start, "end": end, **extra}


def clip(scene, start, duration, **extra):
    return {"id": f"audio_{scene}", "scene_id": f"scene_{scene}", "path": f"C:/audio/{scene}.mp3", "name": f"Line {scene}", "start": start, "source_start": 0, "duration": duration, "full_duration": duration,
            "peaks": [0.1, 0.2], "lane": 0, "role": "dialogue", "volume": 1, "muted": False, "include_in_generation": True, **extra}


def session(**extra):
    base = {"video_type": "speaking", "segments": [segment(1, 0, 5), segment(2, 5, 9), segment(3, 9, 12)], "audio_clips": [clip(1, 0, 3), clip(2, 5, 2.5), clip(3, 9, 1.25)], "flux_reference_builder": REFERENCES}
    return {**base, **extra}


def pure_cases():
    out = []

    def add(name, *args):
        out.append((name, list(args)))

    for value in (KEY, "  " + KEY + "  ", "", "   ", None, 5, ["x"], "k" * 513, "k" * 512, "has space", "tab\tkey", "caf\u00e9", "\u007fdel", "ok!~"):
        add("validate_api_key", value, False)
        add("validate_api_key", value, True)
    for value in ({}, None, "x", VOICE, {"enabled": True, "voice_id": " abc ", "name": " N "}, {"enabled": 1}, {"enabled": "yes", "voice_id": "a"}, {"voice_id": 5}, {"name": None}, {"enabled": True, "voice_id": ""},
                  {"voice_id": "a b"}, {"voice_id": "a" * 129}, {"voice_id": "a" * 128, "name": "n" * 257}, {"voice_id": "a" * 128, "name": "n" * 256}, {"extra": 1}, {"voice_id": "caf\u00e9"}, {"voice_id": "_-"}, []):
        add("normalize_voice", value)
        add("validate_voice", value)
    for value in (None, {}, {"speaker_id": "a" * 300, "text": "t" * 20000, "delivery": "d" * 5000, "allow_rewrite": True, "model_id": "eleven_v3"}, {"model_id": "bogus"}, {"model_id": 5}, {"text": "x", "allow_rewrite": "yes"},
                  {"speaker_id": "mara", "text": "[whispers] Hello", "delivery": "soft", "allow_rewrite": True, "model_id": "eleven_v4"}, "x", []):
        add("normalize_dialogue", value)
        for require in (False, True):
            add("validate_dialogue", value, require)
    add("validate_dialogue", {"text": "  hello  "}, True)
    add("validate_dialogue", {"text": "t" * 5000, "model_id": "eleven_v3"}, True)
    add("validate_dialogue", {"text": "t" * 5001, "model_id": "eleven_v3"}, True)
    add("validate_dialogue", {"text": "t" * 10001}, True)
    add("validate_dialogue", {"text": "   "}, True)
    add("validate_dialogue", {"speaker_id": 5}, False)
    add("validate_dialogue", {"delivery": ["x"]}, False)
    add("validate_dialogue", {"text": None}, False)
    for speaker in ("mara", "joe", "extra", "prop", "novoice", "ghost", ""):
        add("dialogue_speaker", REFERENCES, speaker)
    add("dialogue_speaker", None, "mara")
    add("dialogue_speaker", {}, "mara")
    add("dialogue_speaker", {"subjects": []}, "mara")
    add("dialogue_speaker", {"subjects": [{"id": "m", "reference_type": "character", "elevenlabs_voice": {"enabled": True, "voice_id": "bad id"}}]}, "m")
    add("dialogue_speaker", {"subjects": [{"id": "m", "elevenlabs_voice": None}]}, "m")
    for value, label, low, high in (("  ok  ", "Field", 1, 10), ("", "Field", 1, 10), ("   ", "Field", 1, 10), (None, "Field", 1, 10), (5, "Field", 1, 10), ("a" * 11, "Field", 1, 10), ("abc", "Voice name", 1, 256), ("short", "Voice description", 20, 1000)):
        add("required_text", value, label, low, high)
    for value in (None, {}, {"user_input": "u" * 5000, "voice_description": "d" * 1500, "preview_text": "p" * 1500, "voice_name": "n" * 300, "model_id": "eleven_multilingual_ttv_v2"}, {"model_id": "bogus"}, {"voice_name": None}, "x"):
        add("normalize_design_draft", value)
        add("validate_design_draft", value)
    add("validate_design_draft", {"user_input": 5})
    add("validate_design_draft", {"voice_description": "d" * 1001})
    add("validate_design_draft", {"model_id": "eleven_ttv_v3", "voice_name": "Ann"})
    add("validate_design_draft", {"model_id": ["x"]})
    for text in ("", "Hello there", "[whispers] Hello, there!", "It's don\u2019t stop [laughs] now", "caf\u00e9 \u00e9t\u00e9 na\u00efve", "[a][b]", "one_two 3 four-five", "  [x]  "):
        add("spoken_words", text)
    for code, body, design in ((401, "", False), (403, "", False), (429, "", False), (500, "", False), (401, "", True), (403, "", True), (409, "", True), (422, "", True), (429, "", True), (500, "", True), (404, "", True),
                               (401, '{"detail": {"status": "missing_permissions"}}', False), (401, '{"detail": {"status": "missing_permissions"}}', True), (401, '{"detail": {"status": "invalid_api_key"}}', False),
                               (403, '{"detail": {"status": "invalid_api_key"}}', True), (401, '{"detail": {"status": "other"}}', False), (401, '{"detail": "text"}', False), (401, '{"detail": {"status": 5}}', False),
                               (401, "not json", False), (401, "[1]", False), (401, '{"detail": {"status": "missing_permissions"}}' + " " * 9000, False), (418, '{"detail": {"status": "invalid_api_key"}}', False)):
        add("_voice_request_error", code, body, design)
    # Scene audio settings.
    for value, scene in (({}, False), ({"silence_before": 1}, False), ({"silence_before": 1.23456, "silence_after": 60, "fit_duration": False}, False), ({"silence_before": -1}, False), ({"silence_after": 61}, False),
                         ({"silence_before": "1"}, False), ({"silence_before": True}, False), ({"silence_before": None}, False), ({"fit_duration": 1}, False), ({"use_project_defaults": True}, False), ({"use_project_defaults": False}, True),
                         ({"use_project_defaults": 0}, True), ({"bogus": 1}, True), (None, False), ([], True), ({"silence_before": 0.00005}, False), ({"silence_before": 1.00005}, False),
                         ({"silence_before": 2.5e-5}, False), ({"silence_before": 59.99995}, False)):
        add("validate_settings", value, scene)
    for value in ({"video_type": "speaking"}, {"video_type": "singing"}, {}, {"video_type": None}):
        add("require_speaking", value)
    scene = segment(1, 0, 5)
    for defaults, own in ((None, None), ({"silence_before": 1}, None), ({"silence_before": 1}, {"use_project_defaults": False}), ({"silence_before": 1}, {"use_project_defaults": False, "silence_after": 2, "fit_duration": False}),
                          ({}, {"use_project_defaults": True, "silence_after": 3}), ({"bogus": 1}, None), (None, {"bogus": 1}), ({"silence_before": 99}, None), ({"fit_duration": False}, {"use_project_defaults": True})):
        add("effective_settings", session(speaking_audio_defaults=defaults) if defaults is not None else session(), {**scene, **({"scene_audio_settings": own} if own is not None else {})})
    # update_audio_settings: (session, settings, scene_id, attachment, dialogue)
    attach = {"saved_path": "C:/audio/new.mp3", "audio_name": "new.mp3", "duration": 4.5, "peaks": [0.5]}
    for settings, scene_id, attachment, dialogue in (
        ({"silence_before": 0.5}, None, None, None),
        ({"silence_after": 1.5}, None, None, None),
        ({"silence_before": 1, "silence_after": 2, "fit_duration": True}, None, None, None),
        ({"fit_duration": False}, None, None, None),
        ({}, None, None, None),
        ({"silence_before": 0.5}, "scene_2", None, None),
        ({"silence_after": 2, "use_project_defaults": False}, "scene_2", None, None),
        ({"silence_before": 1, "use_project_defaults": False, "fit_duration": False}, "scene_1", None, None),
        ({"use_project_defaults": False}, "scene_3", None, None),
        ({}, "scene_2", attach, None),
        ({"silence_before": 1}, "scene_1", attach, None),
        ({}, "scene_2", {}, None),
        ({}, "scene_2", {"saved_path": "C:/a.mp3"}, None),
        ({}, "scene_2", {"duration": 2}, None),
        ({}, "scene_2", None, {"speaker_id": "mara", "text": "Hello there", "delivery": "soft", "model_id": "eleven_v3"}),
        ({}, "scene_2", None, {"speaker_id": "joe", "text": "Hi"}),
        ({}, "scene_2", None, {"speaker_id": "", "text": "No speaker"}),
        ({}, "scene_2", None, {"model_id": "bogus"}),
        ({}, None, None, {"text": "x"}),
        ({"bogus": 1}, None, None, None),
        ({"silence_before": 1}, "scene_9", None, None),
        ({"use_project_defaults": True}, None, None, None),
        ({"silence_before": 0.00005}, "scene_1", None, None),
    ):
        add("update_audio_settings", session(), settings, scene_id, attachment, dialogue)
    add("update_audio_settings", session(video_type="singing"), {}, None, None, None)
    add("update_audio_settings", session(timing_frozen=True), {}, None, None, None)
    add("update_audio_settings", session(segments=[segment(1, 0, 5, video_status="running"), segment(2, 5, 9)]), {}, None, None, None)
    add("update_audio_settings", session(segments=[segment(3, 9, 12), segment(1, 0, 5), segment(2, 5, 9)]), {"silence_before": 1}, None, None, None)
    add("update_audio_settings", session(segments=[segment(1, 0, 5, video_path="C:/v/1.mp4"), segment(2, 5, 9, video_output="C:/v/2.mp4"), segment(3, 9, 12)]), {"silence_before": 1}, None, None, None)
    add("update_audio_settings", session(segments=[segment(1, 0, 5, video_path="C:/v/1.mp4")], audio_clips=[clip(1, 0, 3)]), {"silence_after": 0.0}, None, None, None)
    add("update_audio_settings", session(audio_clips=[clip(1, 0, 3), clip(2, 5, 2.5), clip(3, 9, 1.25, role="music"), {"id": "bed", "scene_id": "scene_2", "role": "music", "start": 5, "duration": 4}]), {"silence_before": 1}, None, None, None)
    add("update_audio_settings", session(audio_clips=[clip(1, 0, 3), clip(1, 3.5, 1), clip(2, 5, 2.5)]), {"silence_before": 0.25}, "scene_1", None, None)
    add("update_audio_settings", session(speaking_audio_defaults={"silence_before": 2}), {"silence_after": 1}, None, None, None)
    add("update_audio_settings", session(segments=[segment(1, 0, 5, scene_audio_settings={"use_project_defaults": False, "silence_before": 1}), segment(2, 5, 9), segment(3, 9, 12)]), {"silence_before": 0.5}, None, None, None)
    add("update_audio_settings", session(segments=[segment(1, 0, 5, scene_audio_settings={"bogus": 1}), segment(2, 5, 9)]), {"silence_before": 0.5}, None, None, None)
    add("update_audio_settings", session(segments=[segment(1, 0, 5, custom_audio_timeline_start=0.0), segment(2, 5, 9)], audio_clips=[]), {"silence_before": 0.5}, None, None, None)
    legacy = [segment(1, 0, 5, custom_audio_path="C:/a/1.mp3", custom_audio_name="One", custom_audio_duration=3, custom_audio_timeline_start=0.5, custom_audio_peaks=[0.1]),
              segment(2, 5, 9, custom_audio_path="C:/a/2.mp3", custom_audio_source_start=0.25), segment(3, 9, 12)]
    add("update_audio_settings", {"video_type": "speaking", "segments": legacy}, {"silence_before": 0.5}, None, None, None)
    add("update_audio_settings", {"video_type": "speaking", "segments": [segment(1, 0, 5), segment(2, 5, 9)], "audio_path": "C:/p.mp3", "audio_duration": 9.5, "audio_peaks": [1]}, {"silence_before": 0.5}, None, None, None)
    add("update_audio_settings", {"video_type": "speaking", "segments": [segment(1, 0, 5)], "audio_path": "C:/p.mp3", "audio_duration": 0}, {"silence_before": 0.5}, None, None, None)
    add("update_audio_settings", {"video_type": "speaking", "segments": [{"id": "a", "label": "A"}]}, {}, None, None, None)
    add("update_audio_settings", {"video_type": "speaking"}, {"silence_before": 1}, None, None, None)
    add("update_audio_settings", {"video_type": "speaking", "segments": []}, {"silence_before": 1}, None, None, None)
    return out


def run_pure(modules):
    elevenlabs, speech, design, audio, dialogue = modules
    table = {
        "validate_api_key": elevenlabs.validate_api_key, "normalize_voice": elevenlabs.normalize_voice, "validate_voice": elevenlabs.validate_voice,
        "normalize_dialogue": speech.normalize_dialogue, "validate_dialogue": speech.validate_dialogue, "dialogue_speaker": speech.dialogue_speaker,
        "required_text": design.required_text, "normalize_design_draft": design.normalize_design_draft, "validate_design_draft": design.validate_design_draft,
        "spoken_words": dialogue.spoken_words, "validate_settings": audio.validate_settings, "require_speaking": audio.require_speaking,
        "effective_settings": audio.effective_settings, "update_audio_settings": audio.update_audio_settings,
    }
    out = []
    for name, args in pure_cases():
        call = [json.loads(json.dumps(a, allow_nan=True)) if not isinstance(a, float) else a for a in args]
        try:
            if name == "_voice_request_error":
                error = http_error(call[0], call[1].encode("utf-8"))
                result = {"return": elevenlabs._voice_request_error(error, call[2])}
            else:
                result = {"return": table[name](*call)}
        except Exception as exc:  # noqa: BLE001
            result = {"error": str(exc)}
        out.append({"fn": name, "args": json.loads(json.dumps(args, allow_nan=True)), **result})
    return out


# -- http cases ----------------------------------------------------------------------------------------------------------------

class Reply:
    def __init__(self, body, content_type):
        self._body = body
        self.headers = {"Content-Type": content_type} if content_type else {}

    def read(self, count=-1):
        return self._body if count is None or count < 0 else self._body[:count]

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False


def scripted(script, requests):
    queue = list(script)

    def urlopen(request, timeout=None):
        requests.append({
            "method": request.get_method(), "url": request.full_url, "headers": {k.lower(): v for k, v in request.header_items()},
            "body": json.loads(request.data) if request.data else None, "timeout": timeout,
        })
        step = queue.pop(0)
        if "http_error" in step:
            raise http_error(step["http_error"], step.get("body", ""))
        if "connect" in step:
            raise {"url": URLError("down"), "timeout": TimeoutError("slow"), "os": OSError("broken")}[step["connect"]]
        body = base64.b64decode(step["bytes_b64"]) if "bytes_b64" in step else (step["body"] if isinstance(step["body"], str) else json.dumps(step["body"])).encode("utf-8")
        return Reply(body, step.get("content_type", "application/json"))

    return urlopen


def voice_list(*voices, **extra):
    return {"body": {"voices": list(voices), **extra}}


def previews(count=2, **extra):
    return {"body": {"previews": [{"generated_voice_id": f"gen_{n}", "audio_base_64": MP3, "media_type": "audio/mpeg"} for n in range(count)], "text": "Preview text from ElevenLabs.", **extra}}


def http_cases():
    out = []

    def add(name, args, script):
        out.append((name, args, script))

    page = voice_list({"voice_id": "v1", "name": "Ann", "category": "premade", "preview_url": "https://cdn.example.com/a.mp3"}, {"voice_id": "v2", "preview_url": "http://insecure.example.com/b.mp3"},
                      {"voice_id": "v3", "name": "", "preview_url": "https://user:pw@example.com/c.mp3", "category": None}, {"voice_id": ""}, {"name": "No id"}, 5, {"voice_id": 7, "preview_url": "https://"},
                      {"voice_id": "v8", "preview_url": "HTTPS://Example.com/x"}, {"voice_id": "v9", "preview_url": "https://:8080/x"})
    add("list_voices", [KEY, ""], [page])
    add("list_voices", [KEY, "page 2/+&"], [voice_list({"voice_id": "v1"}, has_more=True, next_page_token="tok-2")])
    add("list_voices", [KEY, "tok-1"], [voice_list({"voice_id": "v1"}, has_more=True, next_page_token="tok-1")])
    add("list_voices", [KEY, ""], [voice_list({"voice_id": "v1"}, has_more=True)])
    add("list_voices", [KEY, ""], [voice_list({"voice_id": "v1"}, has_more=True, next_page_token="")])
    add("list_voices", [KEY, ""], [voice_list({"voice_id": "v1"}, has_more=True, next_page_token=5)])
    add("list_voices", [KEY, ""], [voice_list({"voice_id": "v1"}, has_more=False, next_page_token="ignored")])
    add("list_voices", [KEY, ""], [voice_list()])
    add("list_voices", [KEY, ""], [{"body": {"voices": "x"}}])
    add("list_voices", [KEY, ""], [{"body": [1, 2]}])
    add("list_voices", [KEY, ""], [{"body": "not json"}])
    add("list_voices", [KEY, ""], [{"body": ""}])
    add("list_voices", [KEY, ""], [{"bytes_b64": base64.b64encode(b"\xff\xfe").decode("ascii")}])
    for code in (401, 403, 429, 500):
        add("list_voices", [KEY, ""], [{"http_error": code, "body": ""}])
    add("list_voices", [KEY, ""], [{"http_error": 401, "body": json.dumps({"detail": {"status": "missing_permissions"}})}])
    add("list_voices", [KEY, ""], [{"http_error": 401, "body": json.dumps({"detail": {"status": "invalid_api_key"}})}])
    add("list_voices", [KEY, ""], [{"http_error": 401, "body": "x" * 9000}])
    for kind in ("url", "timeout", "os"):
        add("list_voices", [KEY, ""], [{"connect": kind}])
    add("list_voices", [None, ""], [])
    add("list_voices", ["", ""], [])
    add("list_voices", [KEY, None], [])
    add("list_voices", [KEY, 5], [])
    add("list_voices", [KEY, "t" * 2049], [])
    add("list_voices", ["bad key", ""], [])
    dialogue = {"speaker_id": "mara", "text": "[whispers] Hello there", "delivery": "soft", "model_id": "eleven_v4"}
    speech = {"bytes_b64": MP3, "content_type": "audio/mpeg"}
    add("generate_speech", [KEY, "voice_abc-123", dialogue], [speech])
    add("generate_speech", [KEY, "voice_abc-123", {**dialogue, "model_id": "eleven_v3"}], [speech])
    add("generate_speech", [KEY, "voice_abc-123", dialogue], [{**speech, "content_type": "audio/mpeg; charset=binary"}])
    add("generate_speech", [KEY, "voice_abc-123", dialogue], [{**speech, "content_type": "audio/mp3"}])
    add("generate_speech", [KEY, "voice_abc-123", dialogue], [{**speech, "content_type": "application/octet-stream"}])
    add("generate_speech", [KEY, "voice_abc-123", dialogue], [{**speech, "content_type": "text/html"}])
    add("generate_speech", [KEY, "voice_abc-123", dialogue], [{**speech, "content_type": "Audio/MPEG"}])
    add("generate_speech", [KEY, "voice_abc-123", dialogue], [{"bytes_b64": "", "content_type": "audio/mpeg"}])
    for code in (401, 403, 404, 422, 429, 500):
        add("generate_speech", [KEY, "voice_abc-123", dialogue], [{"http_error": code, "body": "secret text"}])
    for kind in ("url", "timeout", "os"):
        add("generate_speech", [KEY, "voice_abc-123", dialogue], [{"connect": kind}])
    add("generate_speech", [KEY, "voice_abc-123", {"text": ""}], [])
    add("generate_speech", [KEY, "voice_abc-123", None], [])
    add("generate_speech", [KEY, "bad voice", dialogue], [])
    add("generate_speech", [KEY, "", dialogue], [])
    add("generate_speech", [None, "voice_abc-123", dialogue], [])
    add("generate_speech", ["bad key", "voice_abc-123", dialogue], [])
    add("generate_speech", [KEY, "voice_abc-123", {"text": "Caf\u00e9 \u00e9t\u00e9 \u2014 done."}], [speech])
    design = {"voice_description": "A warm, low voice with a slight rasp and unhurried cadence.", "model_id": "eleven_ttv_v3"}
    add("design_voice", [KEY, design], [previews()])
    add("design_voice", [KEY, {**design, "text": "x" * 120}], [previews(3)])
    add("design_voice", [KEY, {**design, "text": "   "}], [previews(1)])
    add("design_voice", [KEY, {"voice_description": design["voice_description"]}], [previews()])
    add("design_voice", [KEY, {**design, "model_id": "eleven_multilingual_ttv_v2"}], [previews(10)])
    add("design_voice", [KEY, design], [previews(2, text="")])
    add("design_voice", [KEY, design], [previews(0)])
    add("design_voice", [KEY, design], [previews(11)])
    add("design_voice", [KEY, design], [{"body": {"previews": "x"}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [5]}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [{"generated_voice_id": "bad id", "audio_base_64": MP3, "media_type": "audio/mpeg"}]}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [{"generated_voice_id": None, "audio_base_64": MP3, "media_type": "audio/mpeg"}]}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [{"generated_voice_id": "g1", "audio_base_64": MP3, "media_type": "audio/wav"}]}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [{"generated_voice_id": "g1", "audio_base_64": 5, "media_type": "audio/mp3"}]}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [{"generated_voice_id": "g1", "audio_base_64": "!!notbase64!!", "media_type": "audio/mp3"}]}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [{"generated_voice_id": "g1", "audio_base_64": "", "media_type": "audio/mp3"}]}}])
    add("design_voice", [KEY, design], [{"body": {"previews": [{"generated_voice_id": "g1", "audio_base_64": MP3[:-1], "media_type": "audio/mp3"}]}}])
    add("design_voice", [KEY, design], [{"body": [1]}])
    add("design_voice", [KEY, design], [{"body": "nope"}])
    for code in (401, 403, 409, 422, 429, 500, 404):
        add("design_voice", [KEY, design], [{"http_error": code, "body": ""}])
    add("design_voice", [KEY, design], [{"http_error": 403, "body": json.dumps({"detail": {"status": "missing_permissions"}})}])
    add("design_voice", [KEY, design], [{"connect": "url"}])
    add("design_voice", [KEY, {"voice_description": "short"}], [])
    add("design_voice", [KEY, {}], [])
    add("design_voice", [KEY, {**design, "model_id": "bogus"}], [])
    add("design_voice", [KEY, {**design, "model_id": ["x"]}], [])
    add("design_voice", [KEY, {**design, "text": 5}], [])
    add("design_voice", [KEY, {**design, "text": "too short but not blank"}], [])
    add("design_voice", [None, design], [])
    create = {"voice_name": "Warm Narrator", "voice_description": design["voice_description"], "generated_voice_id": "gen_1"}
    add("create_designed_voice", [KEY, create], [{"body": {"voice_id": "voice_saved-1"}}])
    add("create_designed_voice", [KEY, create], [{"body": {"voice_id": "bad id"}}])
    add("create_designed_voice", [KEY, create], [{"body": {}}])
    add("create_designed_voice", [KEY, create], [{"body": [1]}])
    add("create_designed_voice", [KEY, create], [{"body": "x"}])
    for code in (401, 403, 409, 422, 429, 500, 408, 503, 400):
        add("create_designed_voice", [KEY, create], [{"http_error": code, "body": ""}])
    for kind in ("url", "timeout"):
        add("create_designed_voice", [KEY, create], [{"connect": kind}])
    add("create_designed_voice", [KEY, {**create, "voice_name": ""}], [])
    add("create_designed_voice", [KEY, {**create, "voice_name": "n" * 257}], [])
    add("create_designed_voice", [KEY, {**create, "voice_description": "short"}], [])
    add("create_designed_voice", [KEY, {**create, "generated_voice_id": None}], [])
    add("create_designed_voice", [KEY, {**create, "generated_voice_id": "bad id"}], [])
    add("create_designed_voice", [KEY, {}], [])
    add("create_designed_voice", ["", create], [])
    return out


def run_http(modules):
    elevenlabs, speech, design = modules
    functions = {"list_voices": (elevenlabs, elevenlabs.list_voices), "generate_speech": (speech, speech.generate_speech), "design_voice": (design, design.design_voice), "create_designed_voice": (design, design.create_designed_voice)}
    out = []
    for name, args, script in http_cases():
        module, function = functions[name]
        requests = []
        original = module.urlopen
        module.urlopen = scripted(script, requests)
        try:
            result = {"return": function(*copy.deepcopy(args))}
        except Exception as exc:  # noqa: BLE001
            result = {"error": str(exc)}
        finally:
            module.urlopen = original
        out.append({"fn": name, "args": args, "script": script, "requests": requests, **result})
    return out


# -- writer cases --------------------------------------------------------------------------------------------------------------

def writer_cases():
    dialogue = {"speaker_id": "mara", "text": "Where were you last night?", "delivery": "quiet, controlled anger", "allow_rewrite": False, "model_id": "eleven_v4"}
    tagged = "[quietly] Where were you [sighs] last night?"
    brief = "A warm, low female voice, mid thirties, slight rasp, unhurried."
    description = "A warm, low female voice in her mid thirties with a slight rasp, an unhurried cadence and clear, relaxed articulation."
    voice = [
        ({"user_input": brief}, [description]),
        ({"user_input": brief, "character_name": "Mara", "character_description": "a tired detective", "temperature": 0.9}, [description]),
        ({"user_input": brief, "character_name": "N" * 300, "character_description": "D" * 5000}, ["  " + description + "  "]),
        ({"user_input": "  "}, []),
        ({"user_input": ""}, []),
        ({}, []),
        ({"user_input": None}, []),
        ({"user_input": "x" * 4001}, []),
        ({"user_input": "x" * 4000}, [description]),
        ({"user_input": brief}, ["Too short."]),
        ({"user_input": brief}, ["x" * 1001]),
        ({"user_input": brief}, ["x" * 1000]),
        ({"user_input": brief}, [""]),
        ({"user_input": brief, "text_runner": "llm_api"}, [description]),
    ]
    craft = [
        ({"dialogue": dialogue}, [tagged]),
        ({"dialogue": dialogue, "character_name": "Mara", "character_description": "a detective"}, [tagged]),
        ({"dialogue": {**dialogue, "model_id": "eleven_v3"}}, [tagged]),
        ({"dialogue": {**dialogue, "allow_rewrite": True}}, ["[angrily] Where on earth were you last night, honestly?"]),
        ({"dialogue": dialogue}, ["[angrily] Where on earth were you last night, honestly?"]),
        ({"dialogue": dialogue}, ["[quietly] Where were you, last night?"]),
        ({"dialogue": dialogue}, ["[quietly] Where were you last night\u2019s?"]),
        ({"dialogue": {**dialogue, "text": "It's over, isn't it? Don\u2019t lie."}}, ["[sighs] It's over, isn\u2019t it? [pause] Don't lie."]),
        ({"dialogue": dialogue}, [""]),
        ({"dialogue": dialogue}, ["x " * 6000]),
        ({"dialogue": {**dialogue, "model_id": "eleven_v3"}}, ["x " * 3000]),
        ({"dialogue": {**dialogue, "text": "x"}, "temperature": 0.2, "top_p": 0.5, "max_new_tokens": 400}, ["x"]),
        ({"dialogue": {**dialogue, "text": ""}}, []),
        ({"dialogue": {"text": "   "}}, []),
        ({"dialogue": None}, []),
        ({}, []),
        ({"dialogue": {**dialogue, "model_id": "bogus"}}, []),
        ({"dialogue": dialogue, "project_folder": "<TMP>"}, [tagged]),
        ({"dialogue": dialogue, "project_folder": "<TMP>", "scene_id": "scene_2"}, [tagged]),
        ({"dialogue": dialogue, "text_runner": "llm_api"}, [tagged]),
    ]
    return voice, craft


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.voice_design", "llm.scene_dialogue"])
    names = {name: importlib.import_module(f"vrgdg_pack.builder.{name}") for name in ("elevenlabs", "elevenlabs_speech", "elevenlabs_voice_design", "scene_audio_settings")}
    OUT.mkdir(parents=True, exist_ok=True)

    pure = run_pure((names["elevenlabs"], names["elevenlabs_speech"], names["elevenlabs_voice_design"], names["scene_audio_settings"], modules["llm.scene_dialogue"]))
    (OUT / "pure.json").write_text(json.dumps({"cases": pure}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"pure.json: {len(pure)} cases, errors {sum(1 for c in pure if 'error' in c)}")

    http = run_http((names["elevenlabs"], names["elevenlabs_speech"], names["elevenlabs_voice_design"]))
    (OUT / "http.json").write_text(json.dumps({"cases": http}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"http.json: {len(http)} cases, errors {sum(1 for c in http if 'error' in c)}")

    voice, craft = writer_cases()
    folder, _ = h.temp_files({})
    custom = Path(folder.name) / "project_context" / "custom_builder_instructions"
    (custom / "scenes" / "scene_2").mkdir(parents=True)
    (custom / "elevenlabs_dialogue.txt").write_text("Project dialogue instructions.\n", encoding="utf-8")
    (custom / "scenes" / "scene_2" / "elevenlabs_dialogue.txt").write_text("Scene dialogue instructions.\n", encoding="utf-8")
    writers = []
    with folder:
        real = str(Path(folder.name).resolve())
        for name, function, cases in (("voice", modules["llm.voice_design"].generate_voice_description, voice), ("craft", modules["llm.scene_dialogue"].craft_dialogue, craft)):
            for request, replies in cases:
                payload = {**RUNNER, **json.loads(json.dumps(request).replace("<TMP>", json.dumps(real)[1:-1]))}
                outcome = h.run_case(recorder, function, payload, replies)
                writers.append({"fn": name, "request": request, "replies": replies, **h.scrub(outcome, folder.name)})
    (OUT / "writers.json").write_text(json.dumps({"cases": writers}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"writers.json: {len(writers)} cases, errors {sum(1 for c in writers if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
