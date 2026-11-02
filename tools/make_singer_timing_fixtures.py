"""Dev tool: golden cases for timing a multi-shot singing scene before its prompt is written, from the pack.

    python tools/make_singer_timing_fixtures.py --pack <pack folder>

Two functions of the pack are run:

* ``shot_prompt.shot_aligned_singer_cues``: timed words and a cut plan in, the scene's cue rows out.
* ``lyrics_orchestrator.time_scene_singer_cues``: a scene in, and what it decides: nothing to time, an error, or the audio clip and
  the timestamp request it asks for, and the cue fields it saves for a canned Stable-ts reply. ComfyUI, ffmpeg and the audio clip are
  replaced by recorders, so the cases hold the decisions and the requests only.

Writes ``fixtures/prompt/singer_timing.json``. The Rust port (``h3_core::prompt::singer_timing``) must give the same answers.
"""

import argparse
import copy
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

ROOT = "Z:\\h3fixture\\proj"
CLIP = "Z:\\h3fixture\\proj\\scene_audio\\scene_0001.wav"


def word(text, start, end):
    return {"text": text, "start": start, "end": end}


def plan(duration, cuts):
    return {"exact_duration_seconds": duration, "cut_times_seconds": cuts}


SINGER = {"id": "a", "name": "Ava"}

# Timed words in the shape the timestamp workflow returns (segments of words).
ALIGN_CASES = [
    ("no_words", {"segments": []}, plan(8.0, [4.0])),
    ("only_instrumental", {"segments": [{"type": "instrumental", "words": [word("la", 1.0, 1.5)]}]}, plan(8.0, [4.0])),
    ("one_cut_between_words", {"segments": [{"words": [word("hold", 0.4, 0.9), word("me", 1.0, 1.4), word("close", 5.0, 5.5), word("tonight", 5.6, 6.4)]}]}, plan(8.0, [4.0])),
    ("cut_inside_a_word_moves_to_the_gap", {"segments": [{"words": [word("hold", 0.5, 1.0), word("me", 1.1, 1.6), word("close", 3.7, 4.6), word("tonight", 5.2, 6.0)]}]}, plan(8.0, [4.0])),
    ("cut_inside_a_word_no_gap_near", {"segments": [{"words": [word("held", 0.0, 7.9)]}]}, plan(8.0, [4.0])),
    ("three_cuts", {"segments": [{"words": [word("a", 0.5, 1.0), word("b", 2.2, 2.8), word("c", 4.1, 4.9), word("d", 6.0, 6.9), word("e", 7.4, 7.9)]}]}, plan(9.0, [2.0, 4.5, 7.0])),
    ("cut_beside_the_ends", {"segments": [{"words": [word("a", 0.5, 1.0)]}]}, plan(5.0, [0.0005, 4.9995, 2.5])),
    ("unsorted_words_and_text_key", {"segments": [{"words": [{"word": "later", "start": 3.0, "end": 3.5}, {"text": "first", "start": 0.2, "end": 0.7}, {"text": "  ", "start": 1.0, "end": 1.2}, {"text": "bad", "start": 2.0, "end": 1.9}, {"text": "none", "start": None, "end": 2.0}]}]}, plan(6.0, [3.0])),
    ("negative_start_is_clamped", {"segments": [{"words": [word("a", -0.5, 0.4), word("b", 4.0, 4.5)]}]}, plan(6.0, [2.0])),
    ("cut_at_the_scene_end_is_ignored", {"segments": [{"words": [word("a", 0.5, 1.0)]}]}, plan(5.0, [5.0])),
    ("equal_cuts_do_not_double", {"segments": [{"words": [word("a", 0.5, 1.0), word("b", 3.0, 3.5)]}]}, plan(5.0, [2.0, 2.0005])),
    ("word_on_the_last_boundary", {"segments": [{"words": [word("a", 3.9, 4.1)]}]}, plan(4.0, [2.0])),
    ("two_gaps_tie", {"segments": [{"words": [word("a", 0.0, 2.9), word("b", 3.1, 4.0), word("c", 4.2, 4.6)]}]}, plan(8.0, [3.0])),
]


def base_session(**extra):
    session = {
        "project_folder": ROOT,
        "audio_path": "Z:\\h3fixture\\song.wav",
        "video_type": "singing",
        "flux_reference_builder": {
            "subjects": [
                {"id": "a", "name": "Ava"},
                {"id": "b", "name": "Ben Stone"},
            ],
            "performer_scene_map": {},
        },
        "segments": [
            {"id": "s1", "start": 0.0, "end": 8.0, "lyric_text": "hold me close tonight", "lyric_singers": ["a"]},
        ],
    }
    session.update(extra)
    return session


def timing_cases():
    cases = []

    def add(name, session=None, segment=None, cut_plan=None, index=0, reply=None):
        session = copy.deepcopy(session or base_session())
        segment = copy.deepcopy(segment) if segment is not None else session["segments"][0]
        session["segments"][0] = segment
        cases.append({"name": name, "session": session, "index": index, "cut_plan": cut_plan or plan(8.0, [4.0]), "reply": reply or ALIGN_CASES[2][1]})

    seg = base_session()["segments"][0]
    add("singing_scene")
    add("no_cuts", cut_plan={"exact_duration_seconds": 8.0, "cut_times_seconds": []})
    add("no_character", segment=dict(seg, no_character_present=True))
    add("no_lip_sync", segment=dict(seg, lyric_no_lip_sync=True))
    add("no_lyric", segment=dict(seg, lyric_text=""))
    add("instrumental_lyric", segment=dict(seg, lyric_text="[instrumental]"))
    add("scene_performance_not_singing", segment=dict(seg, performance_mode="speaking"))
    add("project_performance_not_singing", session=base_session(video_type="speaking"))
    add("scene_performance_singing_wins", session=base_session(video_type="speaking"), segment=dict(seg, performance_mode="singing"))
    complete = [{"type": "vocal", "text": "hold me", "start": 0.0, "end": 4.0}, {"type": "vocal", "text": "close tonight", "start": 4.0}]
    add("complete_cue_map_needs_nothing", segment=dict(seg, lyric_performance_mode="cue_map", lyric_cue_map=complete))
    add("cue_map_row_without_start", segment=dict(seg, lyric_performance_mode="cue_map", lyric_cue_map=[{"text": "hold me", "end": 4.0}, complete[1]]))
    add("cue_map_row_ends_before_it_starts", segment=dict(seg, lyric_performance_mode="cue_map", lyric_cue_map=[{"text": "hold me", "start": 5.0, "end": 4.0}]))
    add("cue_map_last_row_ends_at_scene_end", segment=dict(seg, lyric_performance_mode="cue_map", lyric_cue_map=[{"text": "all", "start": 7.9}]))
    add("cue_map_last_row_starts_at_scene_end", segment=dict(seg, lyric_performance_mode="cue_map", lyric_cue_map=[{"text": "all", "start": 8.0}]))
    add("cue_map_not_in_cue_mode", segment=dict(seg, lyric_performance_mode="together", lyric_cue_map=complete))
    incomplete = [{"type": "vocal", "text": "hold me", "start": 0.0}, {"type": "instrumental", "start": 3.0}, {"type": "vocal", "text": "close tonight"}]
    add("incomplete_cue_map_keeps_its_words", segment=dict(seg, lyric_performance_mode="cue_map", lyric_cue_map=incomplete))
    add("no_mapped_singer", segment=dict(seg, lyric_singers=[]))
    add("unknown_singer", segment=dict(seg, lyric_singers=["zed"]))
    add("singer_by_name_lowercase", segment=dict(seg, lyric_singers=["AVA"]))
    add("singer_by_comma_text", segment=dict(seg, lyric_singers="zed; ben stone"))
    add("singer_text_with_edge_separators", segment=dict(seg, lyric_singers=",a,"))
    empty_name = base_session()
    empty_name["flux_reference_builder"]["subjects"].insert(0, {"id": "", "name": ""})
    add("empty_subject_matches_an_empty_name", session=empty_name, segment=dict(seg, lyric_singers=",a"))
    mapped = base_session()
    mapped["flux_reference_builder"]["performer_scene_map"] = {"s1": ["b"], "1": ["a"]}
    add("performer_map_by_scene_id", session=mapped)
    by_number = base_session()
    by_number["flux_reference_builder"]["performer_scene_map"] = {"1": "b"}
    add("performer_map_by_scene_number", session=by_number)
    add("performer_map_empty_falls_to_scene", session=dict(base_session(), flux_reference_builder={"subjects": [{"id": "a", "name": "Ava"}], "performer_scene_map": {"s1": []}}))
    add("no_reference_builder", session=dict(base_session(), flux_reference_builder=None))
    add("no_audio", session=base_session(audio_path="", audio_file=""))
    add("audio_file_key", session=base_session(audio_path="", audio_file="Z:\\h3fixture\\older.wav"))
    add("custom_scene_audio", segment=dict(seg, custom_audio_path="Z:\\h3fixture\\clip.wav", custom_audio_source_start=2.5, custom_audio_duration=6.0))
    add("custom_scene_audio_defaults", segment=dict(seg, custom_audio_path="Z:\\h3fixture\\clip.wav"))
    add("scene_two_of_three", segment=dict(seg, start=8.0, end=16.0), index=1)
    add("scene_starts_late", segment=dict(seg, start=12.25, end=18.25), cut_plan=plan(6.0, [3.0]))
    add("short_scene_window", segment=dict(seg, start=0.0, end=0.5), cut_plan=plan(0.5, [0.25]))
    add("reference_lyrics_from_saved_cues", segment=dict(seg, lyric_performance_mode="cue_map", lyric_cue_map=[{"type": "vocal", "text": " hold me ", "start": 0.0}, {"type": "instrumental", "text": "ignored"}, {"type": "vocal", "text": "", "start": 3.0}, {"type": "vocal", "text": "close", "start": 3.5}]))
    add("no_words_back", reply={"segments": []})
    add("words_back_all_instrumental", reply={"segments": [{"type": "instrumental", "words": [word("x", 1.0, 2.0)]}]})
    return cases


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "prompt"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    shot_prompt = importlib.import_module("vrgdg_pack.minimax.shot_prompt")
    lyrics = importlib.import_module("vrgdg_pack.agent_api.orchestrator.lyrics_orchestrator")
    errors = importlib.import_module("vrgdg_pack.agent_api.errors")

    align = []
    for name, payload, cut_plan in ALIGN_CASES:
        align.append({"name": name, "payload": payload, "cut_plan": cut_plan, "performer": SINGER,
                      "cues": shot_prompt.shot_aligned_singer_cues(payload, cut_plan, SINGER)})

    seen = {}

    def clip(request):
        seen["clip"] = request
        return {"audio_path": CLIP}

    def graph(request):
        seen["graph"] = request
        return {"workflow_path": "lyrics_timestamp_api.json", "prompt": {"1": {"class_type": "Recorded"}}}

    class Client:
        def queue_prompt(self, prompt, client_id=None):
            return {"prompt_id": "p1"}

        async def wait_for_prompt(self, prompt_id, timeout_seconds=0, check_cancel=None):
            return {"p1": {"outputs": {}}}

    lyrics._prepare_scene_audio_clip = clip
    lyrics.utility_workflows._build_timestamped_transcribe_api_prompt = graph
    lyrics.get_comfy_client = lambda: Client()

    timing = []
    for case in timing_cases():
        seen.clear()
        lyrics.extract_text_from_history = lambda history, prompt_id, reply=case["reply"]: [json.dumps(reply)]
        session = copy.deepcopy(case["session"])
        segment = session["segments"][0]
        entry = {k: case[k] for k in ("name", "session", "index", "cut_plan", "reply")}
        try:
            fields = lyrics.time_scene_singer_cues(session, ROOT, segment, case["index"], copy.deepcopy(case["cut_plan"]))
        except errors.ValidationError as exc:
            entry["error"] = str(exc)
        else:
            if "clip" not in seen:
                entry["skip"] = True
                assert fields == {}, (case["name"], fields)
            else:
                entry["clip"] = seen["clip"]
                entry["graph"] = seen["graph"]
                entry["fields"] = fields
        timing.append(entry)

    out.mkdir(parents=True, exist_ok=True)
    (out / "singer_timing.json").write_text(json.dumps({"clip_audio": CLIP, "align": align, "timing": timing}, indent=1, ensure_ascii=False), encoding="utf-8", newline="\n")
    kinds = {"skip": sum(1 for t in timing if t.get("skip")), "error": sum(1 for t in timing if "error" in t), "timed": sum(1 for t in timing if "fields" in t)}
    print(f"singer_timing.json: {len(align)} alignment cases, {len(timing)} timing cases {kinds}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
