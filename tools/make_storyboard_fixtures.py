"""Dev tool: golden cases for the Storyboard's pure functions (scene cards, layers, reference cards, prompt wording), from the pack's own code.

    python tools/make_storyboard_fixtures.py --pack <pack folder>

Writes ``fixtures/story/helpers.json``: for each case the function, its arguments and what the pack returned (or the error it raised).
"""

import argparse
import importlib
import json
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

TEXTS = ["", "  Chorus  ", "Line one\r\nLine two", None, 0, 5, True, False, "x" * 300, "a, b; c\nd", 2.5, "They are walking", "  "]
MODES = ["singing", "Speaking", "short film", "no-lip-sync", "silent", "dialog", "", None, "weird", " visual_only "]
ENGINES = ["minimax_h3", "ltx", "MiniMax_H3", "", None, "other"]
CARD = {"id": "s1", "name": "Ann", "description": "a tall woman with red hair", "image": {"path": "C:/a.png", "data": "", "name": "a.png"}, "trigger": "ann_trigger", "triggerPosition": "end",
        "minimax_voice": {"presetId": "warm", "gender": "f", "voiceDescription": "soft"}, "reference_type": "character", "wears": "o1"}


def pick(rng, items):
    return items[rng.randrange(len(items))]


def random_scene(rng):
    scene = {}
    text_keys = [
        ("id", "id"), ("label", "label"), ("lyrics", "lyric_text"), ("lyric_section", "section"), ("story_beat", "scene_story_beat"), ("image_prompt", "t2i_prompt"),
        ("video_prompt", "i2v_prompt"), ("image_path", "approved_image_path"), ("image_data", "image_reference_data"), ("image_name", "image_reference_name"),
        ("motion_summary", "video_notes"), ("prompt_summary", "summary"), ("setting", "location"), ("shot_type", "shot"), ("camera_motion", "motion_preset"),
        ("character_motion", "subject_motion"), ("performance_style", "song_style"), ("performance_direction", "performance_direction"), ("facial_performance", "facialPerformance"),
        ("facial_performance_custom", "facialPerformanceCustom"), ("facial_performance_direction", "facialPerformanceDirection"), ("trigger_phrase", "trigger"),
        ("trigger_position", "triggerPosition"), ("video_prompt_type", "video_type"), ("video_style", "videoStyle"), ("video_style_custom", "videoStyleCustom"),
        ("temporal_world_effect_override", "temporalWorldEffectOverride"), ("temporal_world_effect_custom", "temporalWorldEffectCustom"), ("notes", "notes"),
        ("timeline_note", "timeline_note"), ("audio_direction", "audioDirection"), ("continuity", "continuityDirection"), ("id_lora_character_id", "character_id"),
        ("id_lora_location_id", "location_id"), ("flf_start_state", "flf_start_state"), ("lyric_performance_mode", "lyric_performance_mode"), ("status", "status"),
        ("timed_lyric_cue_contract", "timed_lyric_cue_contract"), ("emotion_expression_tags", "emotion_expression_tags"), ("minimax_h3_pass2_prompt", "pass2_prompt"),
    ]
    for canonical, alias in text_keys:
        if rng.random() < 0.45:
            scene[canonical if rng.random() < 0.6 else alias] = pick(rng, TEXTS)
    if rng.random() < 0.6:
        scene[pick(rng, ["performance_mode", "performanceMode", "video_performance_mode"])] = pick(rng, MODES)
    if rng.random() < 0.7:
        scene[pick(rng, ["project_video_engine", "projectVideoEngine"])] = pick(rng, ENGINES)
    if rng.random() < 0.5:
        scene[pick(rng, ["minimax_h3_mode", "minimaxH3Mode"])] = pick(rng, ["text_to_video", "Image-To-Video", "reference to video", "x", None, ""])
    if rng.random() < 0.5:
        scene[pick(rng, ["minimax_h3_audio_mode", "minimaxH3AudioMode"])] = pick(rng, ["input_audio", "native-audio", "Built In Audio", "", None, "x"])
    for key in ("scene_number", "number"):
        if rng.random() < 0.3:
            scene[key] = pick(rng, [1, 7, "3", "x", None, 0, -4, 2.9, True])
    for start, end, duration in (("timeline_start", "timeline_end", "exact_duration"), ("start", "end", "duration")):
        if rng.random() < 0.4:
            scene[start] = pick(rng, [0, 1.5, "2.25", None, "x", ""])
            scene[end] = pick(rng, [4, 9.75, "6", None, ""])
            scene[duration] = pick(rng, [4, -1, "3.5", None, 0])
    if rng.random() < 0.6:
        scene[pick(rng, ["subjects", "singers", "mapped_subjects"])] = pick(rng, [["Ann", " Bob ", ""], "Ann, Bob; Cy\nDee", None, "", ["x"] * 20, 5])
    if rng.random() < 0.4:
        scene["lyric_singers"] = pick(rng, [["Ann"], "a,b", None])
    if rng.random() < 0.5:
        scene["subject_refs"] = pick(rng, [[CARD, {"name": "Bob"}, "x", None], [], None, "no", [CARD] * 14])
    if rng.random() < 0.4:
        scene["location_ref"] = pick(rng, [{"id": "l1", "name": "Dock", "description": "wet planks", "image": {"path": "C:/d.png"}}, None, "x", {}])
    if rng.random() < 0.4:
        scene[pick(rng, ["speaker_assignments", "minimax_speaker_assignments", "dialogue_cues"])] = pick(
            rng, [[{"id": "c1", "speakerId": "s1", "speaker": "Ann", "dialogue": "Hi"}, "x", {"line": "yo"}], [], None])
    if rng.random() < 0.4:
        scene[pick(rng, ["extra_subjects", "extraSubjects"])] = pick(
            rng, [[{"name": "Dancer", "count": 12, "interaction": "alongside"}, {"title": "Fan", "count": "x", "interaction": "weird"}, "no", {"count": 500}], [], None])
    for flag in ("lyric_no_lip_sync", "lyric_instrumental", "no_character_present", "noCharacterPresent", "no_visible_subject", "lyric_shot_word_timing_enabled",
                 "include_microphone", "microphone"):
        if rng.random() < 0.25:
            scene[flag] = pick(rng, [True, False, 1, 0, "yes", "", None])
    if rng.random() < 0.3:
        scene["lyric_cue_map"] = pick(rng, [[{"start": 0, "end": 2, "text": "hi"}], "x", None, []])
    if rng.random() < 0.3:
        scene[pick(rng, ["video_prompt_origin", "i2v_prompt_origin"])] = pick(rng, ["gemma", "Gemma", "manual", "", None])
    return scene


def scene_cases(rng):
    cases = [(scene, n) for n, scene in enumerate([
        {}, {"id": "a", "label": "Verse", "lyrics": "la la", "video_prompt": "A woman sings softly in a quiet room.", "subject_refs": [CARD], "project_video_engine": "ltx"},
        {"scene_number": 4, "video_prompt": "A singer stands.", "subjects": ["Ann"], "performance_mode": "singing", "vocal_status": {"lyric_text": "x"}},
        {"video_prompt": "Wide shot of an empty street.", "no_character_present": True}, {"video_prompt": "Close up of her face and eyes.", "project_video_engine": "ltx"},
        {"video_prompt": "Ann sings with quiet intensity. Her eyes narrow.", "project_video_engine": "ltx", "subjects": ["Ann"], "lyrics": "hello"},
    ], start=1)]
    cases += [(random_scene(rng), rng.randrange(1, 40)) for _ in range(90)]
    cases += [("not a dict", 3), (None, 1), ([], 2)]
    return cases


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    persistence = importlib.import_module("vrgdg_pack.storyboard.persistence")
    helpers = importlib.import_module("vrgdg_pack.storyboard.scene_helpers")
    prompts = importlib.import_module("vrgdg_pack.storyboard.scene_prompts")
    rng = random.Random(20260424)
    out = []

    def run(name, function, *args):
        try:
            result = function(*json.loads(json.dumps(args)))
            if isinstance(result, tuple):
                result = list(result)
            out.append({"fn": name, "args": list(args), "return": result})
        except Exception as exc:  # noqa: BLE001
            out.append({"fn": name, "args": list(args), "error": str(exc)})

    for scene, number in scene_cases(rng):
        run("scene", persistence._normalize_storyboard_scene, scene, number)

    script_cues = [{"index": 2, "speaker": "Ann", "text": "Hello there", "scene_index": 1}, {"speaker_alias": "Bob", "dialogue": "Hi", "word_count": 9}, {"speaker": "", "text": "x"}, "no"]
    for value in [
        {}, None, {"cues": script_cues, "speaker_matches": [{"speaker": "Ann", "speaker_id": "s1"}, {"speaker_alias": ""}], "maximum_scene_seconds": 99, "enabled": False},
        {"cues": script_cues, "scene_plan": {"scenes": [{"index": 3, "label": "", "speaker_assignments": [{"text": "Hi", "speaker_name": "Ann", "part_count": 2, "planned_end_seconds": "4.5"}, {"text": ""}],
                                                       "participant_ids": ["a", ""], "duration_seconds": 5}, {"speaker_assignments": []}], "estimated_total_seconds": 12, "split_cue_count": 1},
         "raw_text": "ANN: Hello", "importedAt": "2026-01-01", "unmatched_speakers": ["Zed", ""], "authoritative": 0, "max_scene_seconds": "2"},
    ]:
        run("script_import", persistence._normalize_script_import, value)

    layers = [
        {}, None, {"enabled": 0, "overallStoryIdea": " a story ", "lyricStoryStrength": "9.5", "imageWorldStyle": "full_surreal", "story_flow_version": "2"},
        {"image_world_style": "Weird", "lyric_story_strength": None, "story_book": {"title": " T ", "chapters": [{"title": "One", "paragraphs": ["p", ""], "purpose": "Turn"}]},
         "story_settings": [{"name": "Home", "look": "warm"}], "story_acts": [{"title": "A", "scene_ids": ["s1"], "location_note": "kitchen"}],
         "story_scene_plan": [{"scene_id": "s1", "performance_mode": "story", "cast_ids": ["a"], "performer_id": "a"}], "story_cast_roles": {"a": "Main", "b": "x"},
         "story_relationships": [{"a": "a", "b": "b", "type": "Friends"}], "story_chapter_counts": {"ch_1": 3, "ch_2": 0}, "story_act_count": 99, "story_allow_new_cast": "True",
         "story_video_style": "Story Only", "story_continuation_mode": "LLM", "story_user_locations": "Kitchen: warm", "story_tone_notes": "noir"},
    ]
    for value in layers:
        run("layer", helpers._normalize_story_layer, value)
        run("guidance", helpers._lyric_story_strength_guidance, value if isinstance(value, dict) else {})

    for value in [
        None, {}, {"subjects": [CARD, "x", {"name": "Bob", "refmod": {"name": "r1", "kind": "image", "tokens": "12.7", "strength": 3}, "source": "refmod", "wears": "s1", "follow": False}],
                    "locations": [{"name": "Dock"}], "locations_cleared": 1, "trigger_position": "end", "subjectTriggerPosition": "End", "location_trigger_position": "x"},
    ]:
        run("catalog", helpers._normalize_reference_catalog, value)
    for value in [CARD, {}, None, {"source": "refmod", "refmod": {"name": "q", "folder": "f", "type": "t", "kind": "video", "frames": "x"}, "clothing_set": "c", "reference_type": "outfit"}]:
        run("item", helpers._normalize_reference_item, value, "Subject 1", "subject_1")

    for value in [None, "", "a, b; c\nd", [" a ", "", "b" * 200] + ["x"] * 20, 5, True]:
        run("tags", helpers._normalize_tags, value)
    for value, fallback in [("Home - Kitchen!", "ref"), ("", "ref"), ("...", "r"), ("a" * 200, "x"), ("ünï", "z")]:
        run("stem", helpers._safe_file_stem, value, fallback)

    visible = {"subjects": ["Ann"], "subject_refs": [CARD], "performance_mode": "singing", "lyrics": "hello", "vocal_status": {"lyric_text": "hello"}}
    prompts_text = [
        "A woman sings with quiet intensity in a studio.", "Wide establishing shot of rain on a road.", "She sings. Her face is lit softly.", "Ann looks at the camera and blinks slowly.",
        "Quiet singing, quiet performance, quiet emotion near her eyes.", "", "Close on her gaze, eyes shift, blinking now.",
    ]
    for prompt in prompts_text:
        for scene in (visible, {"no_character_present": True, "subjects": ["Ann"]}, {}, {"subjects": ["Ann"], "performance_mode": "speaking", "vocal_status": {"no_lip_sync": True}}):
            run("facial", prompts._enforce_storyboard_video_facial_requirements, prompt, scene)
    motion_prompts = ["The camera holds on her steady, powerful gaze. Then holds. Static hold.", "She stands still with quiet poised authority and subtle body motion.", "A slow orbit and gentle pan reveal.", "", "She walks across the room."]
    for prompt in motion_prompts:
        for scene in ({"camera_motion_speed": 8, "character_motion_speed": 5}, {"cameraMotionSpeed": "9", "characterMotionSpeed": 2}, {}, {"camera_motion_speed": 3, "character_motion_speed": 4}):
            run("motion", prompts._enforce_storyboard_high_motion_language, prompt, scene)
    for value in ["slow cinematic drift", "Slow left orbit with gentle pan reveal", "subtle handheld movement, slow zoom out", "quiet handheld hold", "", "locked-off shot", "slow tracking shot"]:
        for speed in (3, 7, 9.5, "x", None):
            run("camera", prompts._camera_motion_for_storyboard_speed, value, speed)

    shots = [None, {"starting_shot": {"required": True, "selected_starting_shot": "Eyes Shot"}, "subject_refs": [{"name": "Ann"}]}, {"starting_shot": {"required": True, "shot_type": "wide shot"}, "subjects": ["Bob"]},
             {"starting_shot": {"required": True, "selected_starting_shot": "Hands Shot"}}, {"starting_shot": {"required": False, "shot_type": "x"}}, {"starting_shot": {"required": True}, "shot_type": "over the shoulder"},
             {"starting_shot": {"required": True, "selected_starting_shot": "extreme close-up"}, "visible_subjects": ["Zed"]}]
    for prompt in ["Ann sings in the rain.", "The video begins with a wide shot of Bob. He walks.", "Eyes widen as she sings.", "", "Opening shot: wide shot of the street."]:
        for scene in shots:
            run("start_shot", prompts._ensure_storyboard_starting_shot, prompt, scene or {})

    refs = {"subject_refs": [{"image": {"path": "a.png"}}, {"image_path": "b.png"}], "location_ref": {"image": {"data": "x"}}}
    for prompt in ["Using the provided character reference image and location reference image, create a woman in a room.", "create a quiet street", "A man walks.", "", "Using the provided references, make a dock", "and location reference images create a hall"]:
        for scene in (refs, {"subject_refs": [{"image": {"path": "a.png"}}]}, {"location_ref": {"image_path": "d.png"}}, {}, {"no_character_present": True, "location_ref": {"image": {"path": "d.png"}}}):
            run("ref_opening", prompts._ensure_storyboard_reference_opening, prompt, scene)

    cues = [{"start": 0, "end": 2.5, "text": "hello there", "singer_name": "Ann"}, {"start": 2.5, "end": 4, "type": "instrumental"}, {"start": 4, "end": 3, "text": "bad"}, {"start": "x", "end": 1}, "no", {"start": 5, "end": 6, "text": ""}]
    for scene in [{"lyric_cue_map": cues}, {"lyric_cue_map": cues[:1]}, {"vocal_status": {"lyric_cue_map": cues[1:2]}}, {"timed_lyric_cue_contract": "  stored  "}, {}, "x", {"lyric_cue_map": [cues[2]]}]:
        run("timed", prompts._storyboard_timed_lyric_contract, scene)

    bundle = lambda scene, selected=None: {"scenes": [scene], "selected_scene_number": selected}  # noqa: E731
    for prompt in ["They are walking. Their hands hold them. They sing, and theirs is lost.", "THEY ARE here. They move; they turn.", "Nothing to change."]:
        for subject in ({"subjects": ["Ann"]}, {"subject_refs": [{"name": "Ann", "description": "a woman"}]}, {"subject_refs": [{"name": "Bob", "description": "a man in a hat"}]},
                        {"subject_refs": [{"name": "Zed", "description": "a robot"}]}, {"subjects": ["A", "B"]}, {"subject_count": 1, "subjects": ["Mx"]}, {}):
            run("pronouns", prompts._fix_single_subject_prompt_pronouns, prompt, bundle({"scene_number": 1, **subject}, 1))
    run("pronouns", prompts._fix_single_subject_prompt_pronouns, "They are", {"scenes": []})
    run("pronouns", prompts._fix_single_subject_prompt_pronouns, "They are", {"scenes": [{"scene_number": 1, "subjects": ["Ann"]}, {"scene_number": 2, "subjects": ["Bob"]}], "selected_scene_number": 2})

    for payload in [{}, {"reference_builder": {"subjects": [CARD, {"name": "", "id": ""}], "locations": [{"id": "l1", "name": "Dock", "image": {"path": "p", "data": "zzz"}}]}}, {"referenceBuilder": {"subjects": [{"description": "d"}]}}]:
        run("dialogue_catalog", helpers._storyboard_dialogue_reference_catalog, payload)

    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "helpers.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"helpers.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
