"""Dev tool: golden cases for the Builder Agent reply (`_generate_builder_agent_reply`), from the pack's own code with the LLM replaced by a recorder.

    python tools/make_agent_fixtures.py --pack <pack folder>

Writes ``fixtures/llm/agent.json``. The model's reply is canned, so the cases cover what the writer does with it: which actions survive and how the
user's own words (merge, split, director notes) correct them. See ``llm_harness.py``.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
CONTEXT = {"project_status": {"scene_count": 3, "has_audio": True}, "active_scene": {"scene_id": "scene_0002", "label": "Scene 2", "lyrics": "la la, été"}, "timeline_markers": [{"start": 0, "end": 2.5, "label": "verse"}]}
AUTO = {"auto_apply": True, "context": CONTEXT}


def reply(message, actions=None):
    return json.dumps({"reply": message, "actions": actions if actions is not None else []})


def one(action, message="ok", **extra):
    return ({**AUTO, "message": "do it", **extra}, [reply(message, [action])])


def cases():
    long_text = "word " * 2000
    out = [
        ({"message": "hello", "context": CONTEXT}, [reply("Hi there.")]),
        ({"message": "hello", "context": CONTEXT}, [reply("Hi there.", [{"type": "select_scene", "scene_number": 2}])]),
        ({"message": "hello", "context": CONTEXT, "auto_apply": True}, [reply("Hi there.", [{"type": "select_scene", "scene_number": 2}])]),
        ({"message": "hello", "auto_apply": True, "context": "not a dict"}, [reply("Hi.")]),
        ({"message": "hello", "context": {"big": long_text}}, [reply("Hi.")]),
        ({"messages": [{"role": "user", "content": "first"}, {"role": "assistant", "content": "second"}], "message": "third"}, [reply("Ok.")]),
        ({"messages": [{"role": "User", "content": " spaced "}, {"role": "system", "content": "hidden"}, 5, {"role": "assistant", "content": ""}, {"role": "assistant"}]}, [reply("Ok.")]),
        ({"messages": [{"role": "user", "content": f"m{i}"} for i in range(14)], "message": long_text}, [reply("Ok.")]),
        ({"messages": "x", "message": "hello"}, [reply("Ok.")]),
        ({"messages": [{"role": "user", "content": "only history"}]}, [reply("Ok.")]),
        ({}, []),
        ({"message": "   "}, []),
        ({"messages": [{"role": "bot", "content": "x"}]}, []),
        ({"message": "hello", "agent_purpose": "Walkthrough"}, [reply("Welcome.")]),
        ({"message": "hello", "agent_purpose": "story_builder", "auto_apply": True}, [reply("Planning.")]),
        ({"message": "hello", "agent_purpose": "troubleshoot"}, [reply("Looking.")]),
        ({"message": "hello", "agent_purpose": "bogus"}, [reply("Ok.")]),
        ({"message": "hello", "agent_purpose": "  "}, [reply("Ok.")]),
        ({"message": "hello", "temperature": 0.2, "top_p": 0.5, "max_new_tokens": 900}, [reply("Ok.")]),
        ({"message": "hello", "temperature": "hot"}, []),
        ({"message": "hello"}, ["```json\n" + reply("Fenced.") + "\n```"]),
        ({"message": "hello"}, ["Just some plain text, no JSON."]),
        ({"message": "hello"}, ["Sure! " + reply("After text.") + " Hope that helps."]),
        ({"message": "hello"}, [json.dumps({"reply": "```text\nIn a fence\n```", "actions": []})]),
        ({"message": "hello"}, [json.dumps({"reply": 42, "actions": []})]),
        ({"message": "hello"}, [json.dumps({"reply": None})]),
        ({"message": "hello", **AUTO}, [json.dumps({"reply": "x", "actions": "nope"})]),
        ({"message": "hello", **AUTO}, [json.dumps({"reply": "", "actions": [{"type": "get_story_source"}]})]),
        ({"message": "hello"}, ['[1, 2]']),
        ({"message": "hello"}, ['"quoted"']),
        ({"message": "hello"}, ["42"]),
        ({"message": "hello"}, ["1.5"]),
        ({"message": "hello"}, ["null"]),
        ({"message": "hello"}, ["true"]),
        ({"message": "hello"}, [""]),
    ]
    actions = [
        {"type": "select_scene", "scene_number": 2},
        {"type": "select_scene", "scene_id": "scene_0007", "scene_number": "3"},
        {"type": "select_scene", "scene_label": "Scene 5: Chorus"},
        {"type": "select_scene", "label": "scene12a"},
        {"type": "select_scene", "scene_number": "x", "scene": "Scene 4"},
        {"type": "select_scene", "scene_number": 2.9},
        {"type": "select_scene", "scene_number": True},
        {"type": "select_scene", "scene_number": None, "title": "scene 9 - intro"},
        {"type": "select_scene"},
        {"type": "bogus", "scene_number": 1},
        {"scene_number": 1},
        {"type": "get_scene_lyrics", "scene_id": "x" * 300},
        {"type": "get_scene_context", "scene_number": 4},
        {"type": "set_scene_notes", "scene_number": 2, "text": "A woman at the window."},
        {"type": "set_scene_notes", "scene_number": 2, "notes": "From the notes field."},
        {"type": "set_scene_notes", "scene_number": 2, "concept_prompt": "From the concept field.", "notes": " "},
        {"type": "set_scene_notes", "scene_number": 2},
        {"type": "set_scene_notes", "scene_number": 2, "text": "x" * 6000},
        {"type": "set_flux_notes", "scene_number": 2, "prompt": "Flux alias."},
        {"type": "set_nb_notes", "scene_number": 2, "nano_banana_notes": "NB alias."},
        {"type": "set_video_notes", "scene_number": 2, "i2v_notes": "Video alias."},
        {"type": "set_video_notes", "scene_number": 2, "motion_notes": "Motion alias."},
        {"type": "set_scene_plan", "scene_number": 2, "director_note": "Quiet.", "scene_notes": "Rain.", "flux_notes": "f", "nb_notes": "n", "video_notes": "v"},
        {"type": "set_scene_plan", "scene_number": 2, "director_note": "  ", "scene_notes": ""},
        {"type": "set_scene_plan", "scene_number": 2, "text": "plan text"},
        {"type": "set_scene_plan", "scene_number": 2, "video_notes": "v" * 6000},
        {"type": "set_image_model_mode", "image_mode": "flux_klein"},
        {"type": "set_image_model_mode", "image_mode": "bogus"},
        {"type": "set_video_model_mode", "video_mode": "t2v"},
        {"type": "set_video_model_mode", "video_mode": "i2v", "text": "x"},
        {"type": "request_reference_images", "scene_id": "scene_0002", "image_mode": "nano_banana"},
        {"type": "request_reference_images", "scene_number": 2},
        {"type": "generate_image_prompt_for_current_mode", "scene_number": 2, "image_mode": "zimage"},
        {"type": "run_image_for_current_mode", "scene_id": "scene_0002", "image_mode": "ernie_image"},
        {"type": "generate_video_prompt_for_current_mode", "scene_number": 2, "video_mode": "rtv"},
        {"type": "run_video_for_current_mode", "scene_number": 2, "video_mode": "ingredients"},
        {"type": "run_video_for_current_mode", "scene_number": 2, "video_mode": "other"},
        {"type": "get_context_prompts", "context_type": "Story_Idea"},
        {"type": "get_context_prompts", "context_type": "nope"},
        {"type": "get_context_prompts"},
        {"type": "set_context_prompt", "context_type": "theme_style", "text": "Noir.", "mode": "append"},
        {"type": "set_context_prompt", "context_type": "story_idea", "text": "Story.", "mode": "weird"},
        {"type": "set_context_prompt", "context_type": "subject_scene", "text": "s" * 60000},
        {"type": "set_context_prompt", "context_type": "all", "text": "x"},
        {"type": "set_context_prompt", "context_type": "theme_style"},
        {"type": "save_story_source", "text": "Lyrics here."},
        {"type": "save_story_source", "text": "L" * 60000},
        {"type": "save_story_source"},
        {"type": "get_story_source"},
        {"type": "create_story_scenes_from_source", "scene_count": 500},
        {"type": "create_story_scenes_from_source", "scene_count": "abc"},
        {"type": "create_story_scenes_from_source", "scene_count": 0},
        {"type": "create_story_scenes_from_source", "scene_count": "12"},
        {"type": "create_concept_prompts"},
        {"type": "create_concept_prompts", "source": "Director", "scope": "Selected", "batch": 20, "use_story": False, "use_theme": 0},
        {"type": "create_concept_prompts", "source_mode": "weird", "scope": "weird", "batch_size": "x"},
        {"type": "create_concept_prompts", "batch_size": -4},
        {"type": "create_motion_notes"},
        {"type": "create_motion_notes", "source_mode": "concept_timeline", "scope": "range", "batch_size": 3},
        {"type": "create_motion_notes", "source": "director"},
        {"type": "get_selected_timeline_range"},
        {"type": "set_active_scene_to_selected_range", "scene_id": "scene_0002"},
        {"type": "set_active_scene_to_selected_range", "scene_number": 2, "text": "x"},
        {"type": "set_active_scene_to_selected_range"},
        {"type": "create_scene_from_selected_range", "label": "New", "director_note": "d", "scene_notes": "s", "flux_notes": "f", "nb_notes": "n", "video_notes": "v", "note": "x"},
        {"type": "create_scene_from_selected_range"},
        {"type": "split_selected_range_into_scenes", "scene_count": 3, "label_prefix": "Part", "director_notes": ["a", None, 5], "scene_notes": ["x"] * 130},
        {"type": "split_selected_range_into_scenes", "director_notes": "x"},
        {"type": "split_scene_into_subscenes", "scene_number": 4, "scene_count": 3, "label_prefix": "Scene 4", "scene_notes": ["a", "b", "c"]},
        {"type": "split_scene_into_subscenes", "scene_number": 4, "scene_count": 1},
        {"type": "split_scene_into_subscenes", "scene_number": 4},
        {"type": "split_scene_into_subscenes", "scene_number": 4, "scene_count": 500},
        {"type": "merge_scenes", "scene_numbers": [2, 3, 4], "label": "Scene 2"},
        {"type": "merge_scenes", "scenes": ["5", "x", 5, 6, -1, 0]},
        {"type": "merge_scenes", "scene_numbers": [2]},
        {"type": "merge_scenes", "scene_numbers": [2], "start_scene_number": 2, "end_scene_number": "4"},
        {"type": "merge_scenes", "start_scene_number": 2, "end_scene_number": 0},
        {"type": "merge_scenes", "scene_numbers": "2,3"},
        {"type": "renumber_scene_labels"},
        {"type": "normalize_dual_vocal_director_notes", "replacement": "female and male"},
        {"type": "normalize_dual_vocal_director_notes", "replacement": "  "},
        {"type": "normalize_dual_vocal_director_notes"},
        {"type": "replace_director_note_text", "find": "male singer", "replace": "male"},
        {"type": "replace_director_note_text", "from": "a", "to": "b"},
        {"type": "replace_director_note_text", "find": "a"},
        {"type": "assign_selected_range_note", "label": "Female vocal", "marker_type": "female vocal", "note": "she sings"},
        {"type": "assign_selected_range_note"},
        {"type": "sync_existing_scenes_to_timeline_markers", "create_missing": False},
        {"type": "sync_existing_scenes_to_timeline_markers"},
    ]
    out += [one(action) for action in actions]
    out.append(({**AUTO, "message": "do it"}, [reply("Many.", [{"type": "select_scene", "scene_number": n} for n in range(1, 20)])]))
    out.append(({**AUTO, "message": "do it", "agent_purpose": "story_builder"}, [reply("Many.", [{"type": "select_scene", "scene_number": n} for n in range(1, 80)])]))
    out.append(({**AUTO, "message": "do it"}, [reply("Mixed.", [5, "x", None, {"type": "select_scene", "scene_number": 1}])]))
    # The user's own words correct the model.
    merge_model = [reply("", [{"type": "split_scene_into_subscenes", "scene_number": 2, "scene_count": 3}])]
    out += [
        ({**AUTO, "message": "merge scenes 2 to 4"}, merge_model),
        ({**AUTO, "message": "please combine scenes 2, 3 and 4"}, merge_model),
        ({**AUTO, "message": "Join scenes 5 through 6 now"}, [reply("Joining.", [{"type": "split_selected_range_into_scenes", "scene_count": 2}])]),
        ({**AUTO, "message": "merge scenes 3 and 3"}, [reply("")]),
        ({**AUTO, "message": "merge scenes 4 to 2"}, [reply("")]),
        ({**AUTO, "message": "merge scenes 1 to 400"}, [reply("")]),
        ({**AUTO, "message": "merge scenes 7"}, [reply("")]),
        ({**AUTO, "message": "consolidate everything"}, [reply("")]),
        ({**AUTO, "message": "merge scenes 2-3"}, [reply("Merged.", [{"type": "merge_scenes", "scene_numbers": [2, 3]}])]),
        ({**AUTO, "message": "merge scenes 2-3"}, [reply("Nope.", [{"type": "merge_scenes", "scene_numbers": [9, 10]}])]),
        ({**AUTO, "messages": [{"role": "assistant", "content": "Do you want me to merge scenes 5 and 6?"}], "message": "yes"}, [reply("")]),
        ({**AUTO, "messages": [{"role": "assistant", "content": "Shall I combine scenes 5 to 7?"}], "message": "Okay."}, [reply("Sure.")]),
        ({**AUTO, "messages": [{"role": "assistant", "content": "Shall I combine scenes 5 to 7?"}], "message": "yes, but split scene 2"}, [reply("")]),
        ({**AUTO, "messages": [{"role": "assistant", "content": "Nothing to merge here."}], "message": "go ahead"}, [reply("")]),
        ({**AUTO, "message": "split scene 4 into 3"}, [reply("")]),
        ({**AUTO, "message": "Split scene 4 into 3 sub-scenes"}, [reply("Splitting.", [{"type": "merge_scenes", "scene_numbers": [4, 5]}])]),
        ({**AUTO, "message": "break scene 6 into 40 segments"}, [reply("")]),
        ({**AUTO, "message": "divide scene 2 into 1 scenes"}, [reply("")]),
        ({**AUTO, "message": "make scene 8 into 2 sub-segments"}, [reply("")]),
        ({**AUTO, "message": "split it"}, [reply("")]),
        ({**AUTO, "message": "split scene 4"}, [reply("")]),
        ({**AUTO, "messages": [{"role": "assistant", "content": "How many sub-scenes for split scene 7?"}], "message": "3"}, [reply("")]),
        ({**AUTO, "messages": [{"role": "assistant", "content": "How many sub-scenes for split scene 7?"}], "message": "55"}, [reply("")]),
        ({"auto_apply": False, "message": "split scene 4 into 3"}, [reply("")]),
        ({"auto_apply": False, "message": "merge scenes 2 to 4"}, [reply("")]),
        ({**AUTO, "message": "update the director notes", "context": CONTEXT}, [reply("Updating.", [{"type": "sync_existing_scenes_to_timeline_markers"}, {"type": "split_scene_into_subscenes", "scene_number": 2, "scene_count": 2}, {"type": "merge_scenes", "scene_numbers": [1, 2]}, {"type": "set_scene_notes", "scene_number": 1, "text": "kept"}])]),
        ({**AUTO, "message": "reword the notes to male and female"}, [reply("")]),
        ({**AUTO, "message": "make every director note say female and male"}, [reply("")]),
        ({**AUTO, "message": "make all director notes for both singers say 'female and male'"}, [reply("")]),
        ({**AUTO, "message": "the dual vocal director notes should say Male and Female"}, [reply("Done it.", [{"type": "normalize_dual_vocal_director_notes", "replacement": "male & female"}])]),
        ({**AUTO, "message": "when a director note says \"male singer and female singer\" change it to \"female and male\" for dual-vocal scenes"}, [reply("")]),
        ({**AUTO, "message": "when a director note says 'male singer' to 'both singers' for both singers"}, [reply("", [{"type": "replace_director_note_text", "find": "a", "replace": "b"}])]),
        ({**AUTO, "message": "update the director notes for both vocals"}, [reply("")]),
        ({**AUTO, "message": "dual-vocal text"}, [reply("")]),
        ({**AUTO, "message": "just chatting"}, [reply("")]),
        ({**AUTO, "message": "just chatting"}, [reply("Hello.", [{"type": "get_story_source"}])]),
    ]
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["llm.builder_agent"])
    function = modules["llm.builder_agent"]._generate_builder_agent_reply
    out = []
    for request, replies in cases():
        outcome = h.run_case(recorder, function, {**RUNNER, **request}, replies)
        out.append({"fn": "agent", "request": request, "replies": replies, **outcome})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "llm" / "agent.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"agent.json: {len(out)} cases, errors {sum(1 for c in out if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
