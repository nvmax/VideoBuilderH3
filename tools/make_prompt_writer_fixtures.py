"""Dev tool: golden MiniMax prompts for the prompt writer (``h3-pipeline::minimax_prompts``), from the pack.

    python tools/make_prompt_writer_fixtures.py --pack <pack folder>

Each case is a project session, a scene and the shots a stand-in LLM answers. The pack's ``generate_scene_prompt`` turns
them into the final prompt. The Rust writer must produce the same text from the same reply. The request is compared too: the
shot task (``task``) the LLM is given, with the directions the scene gives and the Theatrical presets it selects. The pack adds the
Storyboard card JSON to it, which this app has no use for, so the pack is asked for the context without that block.

Writes ``fixtures/prompt/writer.json``.
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


def base_session(**extra):
    session = {
        "project_folder": ROOT,
        "video_engine": "minimax_h3",
        "minimax_h3_settings": {"audio_mode": "input_audio", "aspect_ratio": "16:9"},
        "segments": [
            {"id": "s1", "start": 0.0, "end": 4.0, "label": "Scene 1", "lyric_text": "hold me close tonight", "story_beat": "She sings on a roof."},
            {"id": "s2", "start": 4.0, "end": 9.0, "label": "Scene 2", "lyric_text": "[instrumental]", "notes": "slow drift", "i2v_notes": "gentle push in"},
            {"id": "s3", "start": 9.0, "end": 15.5, "label": "Scene 3", "lyric_text": "we run through the night\nand never look back", "camera_motion_speed": 7, "lyric_section": "Chorus"},
        ],
        "flux_reference_builder": {
            "subjects": [{"id": "a", "name": "Ava", "description": "short dark hair, denim jacket", "image": {"path": ROOT + "\\subject_location\\subject\\Ava.png"}}],
            "locations": [{"id": "l1", "name": "Roof", "description": "a rooftop at dusk with string lights", "image": {"path": ROOT + "\\subject_location\\location\\Roof.png"}}],
            "subject_scene_map": {"s1": ["a"], "s2": ["a"], "s3": ["a"]},
            "scene_map": {"s1": "l1", "s2": "l1", "s3": "l1"},
            "use_subject_reference": True,
            "use_location_references": True,
        },
    }
    session.update(extra)
    return session


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "prompt"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    orch = importlib.import_module("vrgdg_pack.agent_api.orchestrator.minimax_prompt_orchestrator")
    sync = importlib.import_module("vrgdg_pack.storyboard.session_sync")
    store = importlib.import_module("vrgdg_pack.storyboard.persistence")
    store._load_storyboard = lambda payload: {"exists": False}
    orch.prepare_llm_payload = lambda body: body
    orch.llm_payload_from_session = lambda session, overrides=None: {}

    state = {"reply": "", "task": ""}

    def stand_in_llm(request):
        state["task"] = request["t2i_prompt"]
        return {"prompt": state["reply"]}

    orch.vid_gen._generate_builder_t2v_prompt = stand_in_llm
    scene_context = importlib.import_module("vrgdg_pack.agent_api.scene_card_context")
    theatrical = importlib.import_module("vrgdg_pack.storyboard.theatrical_presets")
    # The context without the Storyboard card JSON: the directions and the Theatrical presets.
    orch.storyboard_video_context = lambda card, defaults=None: (chr(10) * 2).join(scene_context.video_direction_lines(card) + theatrical.theatrical_direction_lines(card, defaults))

    def shots_json(count, extra=""):
        lines = [f"<Subject 1> (Ava) stands on the roof, wind lifting the denim jacket, the camera drifting closer in a steady glide{extra}, shot {i + 1}." for i in range(count)]
        return json.dumps({"shots": [{"description": text} for text in lines]})

    variants = {
        "plain": {},
        "structured": {"use_structured_outputs": True},
        "cuts_5": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 5}},
        "cuts_10_style": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 10, "video_style": "cinematic_realism"}},
        "custom_style_fx": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 3, "video_style": "custom", "video_style_custom": "Neon noir", "fx_preset": "lighting"}},
        "temporal": {"builder_storyboard_defaults": {"temporal_world_effect": "frozen_world", "minimax_h3_cut_frequency": 2}},
        "structured_all": {"use_structured_outputs": True, "builder_storyboard_defaults": {"minimax_h3_cut_frequency": 4, "video_style": "dark_fantasy", "fx_preset": "glitch", "temporal_world_effect": "day_night_sweep"}},
        "built_in_audio": {"minimax_h3_settings": {"audio_mode": "built_in_audio", "aspect_ratio": "9:16 (Portrait Widescreen)"}},
        # Theatrical presets for the whole project, and scenes with Scene Defaults of their own on top.
        "theatrical_project": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 2, "performance_style": "theatrical", "camera_flow": "pretty_little_malfunction"}, "default_facial_performance": "theatrical"},
        "theatrical_scoped": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 2, "performance_style": "theatrical", "camera_flow": "pretty_little_malfunction"}, "default_facial_performance": "theatrical"},
        "scoped_look": {"builder_storyboard_defaults": {"minimax_h3_cut_frequency": 2, "video_style": "cinematic_realism"}},
    }

    def scene_overrides(session):
        segments = session["segments"]
        segments[0]["scene_defaults"] = {"minimax_h3_cut_frequency": 3, "minimax_h3_cut_style": "hard", "camera_motion_speed": 9, "camera_guidance": "slow orbit", "character_guidance": "stay still", "performance_style": "theatrical"}
        segments[1]["scene_defaults"] = {"camera_flow": "pretty_little_malfunction", "facial_performance": "theatrical", "character_motion_speed": 0}
        segments[2]["scene_defaults"] = {"performance_style": "calm", "facial_performance_custom": "wry brows", "camera_flow": "close_ups"}

    def look_overrides(session):
        segments = session["segments"]
        segments[0]["scene_defaults"] = {"video_style": "dark_fantasy", "fx_preset": "glitch", "temporal_world_effect": "frozen_world", "minimax_h3_cut_frequency": 4}
        segments[2]["scene_defaults"] = {"video_style": "custom", "video_style_custom": "Neon noir", "minimax_h3_cut_frequency": 1}

    cases = []
    for name, extra in variants.items():
        session = base_session(**copy.deepcopy(extra))
        if name == "theatrical_scoped":
            scene_overrides(session)
        elif name == "scoped_look":
            look_overrides(session)
        cards = sync.scene_cards(session)
        for index, segment in enumerate(session["segments"]):
            defaults = {**(session.get("builder_storyboard_defaults") or {}), **(segment.get("scene_defaults") or {})}
            freq = int(defaults.get("minimax_h3_cut_frequency") or 0)
            pa = importlib.import_module("vrgdg_pack.minimax.prompt_assembly")
            duration = segment["end"] - segment["start"]
            count = pa.storyboard_cut_plan_for_duration(duration, freq)["shot_count"]
            state["reply"] = shots_json(count)
            entry = {"name": f"{name}_scene{index + 1}", "session": session, "index": index, "reply": state["reply"]}
            try:
                result = orch.generate_scene_prompt(session, ROOT, segment, cards[index], index, {})
                entry["prompt"] = result["prompt"]
                entry["characters"] = result["characters"]
                entry["shots"] = result["shots"]
                entry["task"] = state["task"]
            except Exception as exc:  # noqa: BLE001
                entry["error"] = str(exc)
            cases.append(entry)
    # An unmapped scene and a continued scene.
    unmapped = base_session()
    unmapped["flux_reference_builder"]["subject_scene_map"] = {}
    unmapped["flux_reference_builder"]["scene_map"] = {}
    unmapped["flux_reference_builder"]["subjects"] = []
    unmapped["flux_reference_builder"]["locations"] = []
    cards = sync.scene_cards(unmapped)
    state["reply"] = shots_json(1)
    entry = {"name": "unmapped", "session": unmapped, "index": 0, "reply": state["reply"]}
    try:
        orch.generate_scene_prompt(unmapped, ROOT, unmapped["segments"][0], cards[0], 0, {})
    except Exception as exc:  # noqa: BLE001
        entry["error"] = str(exc)
    cases.append(entry)
    # Timed cue maps and scenes continued from the previous scene (with and without its final frame).
    def add(name, session, index, previous_frame=False, shots=None):
        cards = sync.scene_cards(session)
        segment = session["segments"][index]
        pa = importlib.import_module("vrgdg_pack.minimax.prompt_assembly")
        entry = {"name": name, "session": session, "index": index, "previous_frame": previous_frame}
        try:
            probe = {}
            state["reply"] = shots_json(shots or 1)
            if shots is None:
                # One shot per planned cut, found by asking the pack for the plan it will use.
                scoped = {**(session.get("builder_storyboard_defaults") or {}), **(segment.get("scene_defaults") or {})}
                freq = int(scoped.get("minimax_h3_cut_frequency") or 0)
                shots = pa.storyboard_cut_plan_for_duration(segment["end"] - segment["start"], freq)["shot_count"]
                state["reply"] = shots_json(shots)
            entry["reply"] = state["reply"]
            result = orch.generate_scene_prompt(session, ROOT, segment, cards[index], index, {}, previous_frame="Z:\\frame.png" if previous_frame else "")
            entry.update({"prompt": result["prompt"], "characters": result["characters"], "shots": result["shots"], "used_previous_frame": result["used_previous_frame"], "warnings": result["warnings"],
                          "task": state["task"]})
            _ = probe
        except Exception as exc:  # noqa: BLE001
            entry["reply"] = state["reply"]
            entry["error"] = str(exc)
        cases.append(entry)

    def cue_session(rows, mode="cue_map", **extra):
        session = base_session(**extra)
        session["segments"][2]["lyric_performance_mode"] = mode
        session["segments"][2]["lyric_cue_map"] = rows
        return session

    two = [
        {"type": "vocal", "text": "we run through the night", "singer_name": "Ava", "start": 0.0, "vocal_start": 0.3, "vocal_end": 2.8},
        {"type": "vocal", "text": "and never look back", "singer_name": "Ava", "start": 3.4, "vocal_start": 3.6, "vocal_end": 6.0},
    ]
    three = two[:1] + [{"type": "instrumental", "start": 3.0}] + [dict(two[1], start=4.5, vocal_start=4.6, vocal_end=6.3)]
    add("cue_two_rows", cue_session(two), 2, shots=2)
    add("cue_three_rows_instrumental", cue_session(three), 2, shots=3)
    add("cue_unknown_singer", cue_session([dict(two[0], singer_name="Zed"), dict(two[1], singer_name="")]), 2, shots=2)
    add("cue_missing_start_is_ignored", cue_session([dict(two[0]), {k: v for k, v in two[1].items() if k != "start"}]), 2)
    add("cue_ignored_built_in_audio", cue_session(two, minimax_h3_settings={"audio_mode": "built_in_audio", "aspect_ratio": "16:9"}), 2)
    add("cue_not_cue_map_mode", cue_session(two, mode="together"), 2)
    add("cue_with_cut_style", cue_session(two, builder_storyboard_defaults={"minimax_h3_cut_frequency": 4, "minimax_h3_cut_style": "hard"}), 2, shots=2)
    add("cue_structured", cue_session(two, use_structured_outputs=True), 2, shots=2)

    def continued(preset="normal", custom="", start=None, direction="", location_for_s2="l1", saved_prompt=True, **extra):
        session = base_session(**extra)
        settings = {"audio_mode": "input_audio", "aspect_ratio": "16:9", "continuity_mode": "latent_continuation_masked", "location_transition_preset": preset, "location_transition_custom": custom}
        session["minimax_h3_settings"] = {**settings, **(extra.get("minimax_h3_settings") or {})}
        session["flux_reference_builder"]["locations"].append({"id": "l2", "name": "Club", "description": "a neon club floor. Smoke hangs low.", "image": {"path": ROOT + "\\subject_location\\location\\Club.png"}})
        session["flux_reference_builder"]["scene_map"]["s2"] = location_for_s2
        if saved_prompt:
            session["segments"][0]["minimax_h3_prompt"] = "detailed_description:\nstyle line\n\n[Shot 1] At 0:00, Ava sings on the roof, the camera easing in.\n\n[Shot 2] At 0:02, Ava turns toward the skyline, wind lifting her jacket and the lights behind her."
        if start is not None:
            session["segments"][1]["minimax_h3_continuation_start_seconds"] = start
        if direction:
            session["segments"][1]["minimax_h3_continuation_direction"] = direction
        session["segments"][1]["lyric_text"] = "we keep going"
        session["segments"][1]["lyric_no_lip_sync"] = False
        return session

    for preset in ("normal", "masked", "surreal", "cinematic", "inner_world", "match", "motion", "creative_auto", "custom"):
        add(f"continued_{preset}_new_location", continued(preset, custom="walk through the door" if preset == "custom" else "", location_for_s2="l2"), 1)
    add("continued_same_location", continued(), 1)
    add("continued_start_and_direction", continued("masked", start=1.5, direction="  push in\nslowly  ", location_for_s2="l2"), 1)
    add("continued_no_saved_prompt", continued(saved_prompt=False), 1)
    add("continued_with_frame", continued("masked", location_for_s2="l2"), 1, previous_frame=True)
    add("continued_with_frame_same_location", continued(), 1, previous_frame=True)
    add("continued_cut_frequency", continued("masked", location_for_s2="l2", builder_storyboard_defaults={"minimax_h3_cut_frequency": 5}), 1)
    add("continued_first_scene_is_standalone", continued(), 0)
    add("frame_given_but_not_continued", base_session(), 1, previous_frame=True)
    # Lyric-free prompts ("omit lyrics from video prompts") for singing scenes with the song supplied.
    def omit_session(**extra):
        session = base_session(omit_lyrics_from_video_prompts=True, **extra)
        session["flux_reference_builder"]["subjects"].append({"id": "b", "name": "Ben", "description": "grey beard", "image": {"path": ROOT + "\\subject_location\\subject\\Ben.png"}})
        session["flux_reference_builder"]["subject_scene_map"] = {"s1": ["a"], "s2": ["a"], "s3": ["a", "b"]}
        return session

    add("omit_plain_scene1", omit_session(), 0)
    add("omit_instrumental_scene2", omit_session(), 1)
    add("omit_two_singers_scene3", omit_session(), 2)
    add("omit_structured", omit_session(use_structured_outputs=True), 2)
    add("omit_cuts", omit_session(builder_storyboard_defaults={"minimax_h3_cut_frequency": 5, "video_style": "cinematic_realism"}), 2)
    add("omit_facial_preset", {**omit_session(), "segments": [dict(s, facial_performance="pop_polished") for s in omit_session()["segments"]]}, 0)
    add("omit_facial_off", {**omit_session(), "segments": [dict(s, facial_performance="off") for s in omit_session()["segments"]]}, 0)
    add("omit_custom_facial", {**omit_session(), "segments": [dict(s, facial_performance="custom", facial_performance_custom="soft eyes, parted lips and a tender glance") for s in omit_session()["segments"]]}, 2)
    with_cues = omit_session()
    with_cues["segments"][2].update(lyric_performance_mode="cue_map", lyric_singers=["Ava", "Ben"], lyric_cue_map=[
        {"type": "vocal", "text": "we run through the night", "singer_name": "Ava", "singer_id": "a", "start": 0.0, "vocal_start": 0.3, "vocal_end": 2.8, "end": 3.0},
        {"type": "instrumental", "start": 3.0, "end": 3.6},
        {"type": "vocal", "text": "and never look back", "singer_name": "Ben", "singer_id": "b", "start": 3.6, "vocal_start": 3.8, "vocal_end": 6.4, "end": 6.5},
    ])
    add("omit_cue_map", with_cues, 2, shots=3)
    add("omit_cue_map_together_mode", {**with_cues, "segments": [dict(s, lyric_performance_mode="together") for s in with_cues["segments"]]}, 2)
    add("omit_built_in_audio_is_off", omit_session(minimax_h3_settings={"audio_mode": "built_in_audio", "aspect_ratio": "16:9"}), 0)
    add("omit_speaking_is_off", omit_session(video_type="speaking"), 0)
    add("omit_continuous_location_shot", {**omit_session(builder_storyboard_defaults={"minimax_h3_cut_frequency": 5}), "segments": [dict(s, location_continuous_shot=True) for s in omit_session()["segments"]]}, 2, shots=1)
    add("omit_with_emotion_tags", {**omit_session(), "segments": [dict(s, emotion_expression_tags="longing, hope") for s in omit_session()["segments"]]}, 0)
    omit_continued = continued("masked", location_for_s2="l2", omit_lyrics_from_video_prompts=True)
    add("omit_continued_scene2", omit_continued, 1)
    # Theatrical presets and Scene Defaults with lyric-free prompts and with a continued scene.
    scoped_omit = omit_session(default_facial_performance="theatrical", builder_storyboard_defaults={"minimax_h3_cut_frequency": 2})
    scoped_omit["segments"][0]["scene_defaults"] = {"facial_performance": "pop_polished", "minimax_h3_cut_frequency": 4}
    scoped_omit["segments"][2]["scene_defaults"] = {"facial_performance": "theatrical", "performance_style": "theatrical", "minimax_h3_cut_frequency": 3, "minimax_h3_cut_style": "hard"}
    add("omit_scoped_scene1", scoped_omit, 0)
    add("omit_scoped_scene2_project_face", scoped_omit, 1)
    add("omit_scoped_scene3", scoped_omit, 2)
    add("omit_theatrical_project_face", omit_session(default_facial_performance="theatrical"), 0)
    scoped_continued = continued("masked", location_for_s2="l2", builder_storyboard_defaults={"minimax_h3_cut_frequency": 5, "performance_style": "theatrical"})
    scoped_continued["segments"][1]["scene_defaults"] = {"performance_style": "theatrical", "camera_flow": "pretty_little_malfunction", "minimax_h3_cut_frequency": 1}
    add("continued_scoped", scoped_continued, 1)
    (out / "writer.json").write_text(json.dumps({"cases": cases}, indent=1, ensure_ascii=False), encoding="utf-8")
    print("writer cases:", len(cases), "errors:", sum(1 for c in cases if "error" in c))
    for c in cases:
        if "error" in c:
            print("  ", c["name"], "->", c["error"][:110])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
