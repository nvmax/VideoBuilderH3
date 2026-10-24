"""Dev tool: golden cases for the Story Brief and the Story Arc (`storyboard/story_layer.py`), from the pack's own code.

    python tools/make_story_arc_fixtures.py --pack <pack folder>

Writes three files in ``fixtures/story``:
  arc_prompts.json  the instruction builders in ``llm/prompts/storyboard.py`` with their arguments and the text they returned
  arc_helpers.json  the lyric section, word limit, scene map and heading checks, and the cast guard over scene entries
  arc_steps.json    ``_build_story_layer_brief`` and ``_build_story_layer_arc`` with the LLM replaced by a recorder (see ``llm_harness.py``)

A reply that is not JSON where the pack parses JSON is left out of the step cases: the error text of Python's parser differs from the Rust one.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import llm_harness as h  # noqa: E402

RUNNER = {"text_runner": "lm_studio", "lmstudio_model": "test-model"}
OUT = Path(__file__).resolve().parents[1] / "fixtures" / "story"

MARA = {"id": "mara", "name": "Mara", "description": "a woman with red hair", "reference_type": "character"}
JOE = {"id": "joe", "name": "Joe", "description": "a tall man in a hat", "reference_type": "character"}
LABELS = ["Intro", "Verse 1", "Chorus", "Verse 2", "Chorus 2", "Outro"]
LYRICS = "[Intro]\n\n[Verse 1]\nwalking home alone\nthe street is empty\nI count the lights\n\n[Chorus]\noh oh oh\nnever going back\n\n[Verse 2]\nmorning comes\n\n[Chorus]\noh oh oh\n\n[Outro]\n"
PARAGRAPH = "Mara carries a lamp across the kitchen and sets it on the sill, her hands steady, and the warm light passes into the next room."


def listing(items):
    return json.dumps(items, ensure_ascii=False, indent=2) if items else "[none]"


def prompt_cases():
    out = []

    def add(name, **kwargs):
        out.append((name, kwargs))

    for purpose in ("brief", "premise"):
        for idea in ("", "A quiet drama."):
            add("_storyboard_script_story_instruction", purpose=purpose, story_idea=idea, subjects_json=listing([MARA]), locations_json="[none]", script_text="MARA: Hello.\nJOE: Hi.", compact_plan_json="[]")
    for guidance, arc, lyrics in (("Lyric Story Strength: 7/10. Strong.", "", ""), ("g", "Verse 1: she waits.", "[Verse 1]\nla"), ("g", "", "la la")):
        add("_storyboard_story_brief_instruction", lyric_story_strength_guidance_text=guidance, user_story_arc=arc, lyrics=lyrics, compact_scenes_json="[]")
    for labels in ([], ["Verse 1"], LABELS):
        add("_storyboard_story_arc_structure_instruction", required_section_labels=labels)
        add("_storyboard_story_arc_format_retry_instruction", required_section_labels=labels or ["Only"], original_instruction="ORIGINAL TEXT")
        add("_storyboard_story_arc_schema", required_section_labels=labels)
    for speed in (-1, 0, 2, 3, 5, 6, 8, 9, 10):
        add("_storyboard_story_arc_motion_guidance", character_motion_speed=speed)
    add("_storyboard_timeline_note_story_instruction", notes_json='[{"start": 1.5, "end": null}]')
    add("_storyboard_story_arc_json_retry_instruction", original_instruction="ORIGINAL")
    base = {
        "required_section_labels": LABELS, "section_word_limit": 100, "story_arc_seed": "123", "camera_flow": "Smooth flow", "camera_motion_speed": 4, "character_motion_speed": 7,
        "performance_style": "Natural", "facial_performance": "Subtle", "lyric_story_strength_guidance_text": "Lyric Story Strength: 7/10.", "story_idea": "A long night.",
        "previous_story_arc": "", "style_theme": "Noir.", "lyrics_source": "project full_lyrics.txt", "lyrics": LYRICS, "compact_scenes_json": "[]", "subjects_json": listing([MARA]),
        "locations_json": "[none]",
    }
    add("_storyboard_story_arc_instruction", **base)
    add("_storyboard_story_arc_instruction", **base, song_story_brief="A quiet song.")
    add("_storyboard_story_arc_instruction", **{**base, "required_section_labels": [], "story_arc_seed": "", "camera_flow": "", "performance_style": "", "facial_performance": "", "story_idea": "",
                                                "previous_story_arc": "Old arc.", "style_theme": "", "lyrics": "", "character_motion_speed": 1, "section_word_limit": 60})
    add("_storyboard_story_arc_instruction", **{**base, "character_motion_speed": 10, "camera_motion_speed": 0})
    entry = {
        "section_label": "Verse 1", "section_text": PARAGRAPH, "scene_number": 3, "location_name": "Kitchen", "location_description": "a warm kitchen", "cast_wall": "PEOPLE IN THIS SCENE: Mara.",
        "cast_names": ["Mara"], "lyric": "walking home", "previous_entry": "Mara waits.", "camera_flow": "Smooth", "camera_motion_speed": 4, "character_motion_speed": 7, "word_limit": 55,
        "story_arc_seed": "9",
    }
    add("_storyboard_story_arc_scene_entry_instruction", **entry)
    add("_storyboard_story_arc_scene_entry_instruction", **{**entry, "cast_names": [], "location_name": "", "location_description": "", "lyric": "", "previous_entry": "", "camera_flow": "", "section_text": "", "story_arc_seed": ""})
    add("_storyboard_story_arc_scene_entry_instruction", **{**entry, "cast_names": ["Mara", "Joe"], "character_motion_speed": 1})
    return out


def run_prompts(module):
    out = []
    for name, kwargs in prompt_cases():
        out.append({"fn": name, "args": kwargs, "return": getattr(module, name)(**kwargs)})
    return out


def lines(*parts):
    return "\n".join(parts)


def helper_cases():
    out = []

    def add(name, *args):
        out.append((name, list(args)))

    lyric_texts = [
        "", "no headers at all\njust lines", LYRICS, "[Verse 1]\na\n[Verse 1]\nb\n[Chorus]\nc", "[Verse 1][Whispered] soft line\nmore\n[Chorus] [High Energy]\nbig", "[Pre-Chorus]\na\n[Post Chorus]\nb\n[Bridge 2]\nc\n[Interlude]\nd",
        "[Part A]\nx\n[Part B]\ny", "[Verse]\na\n[Verse]\nb\n[Verse]\nc", "[Intro]\n[End]\nstray\n[Verse 1]\nok\n[End of song]\nbye", "[Whispered]\ntext\n[Verse 1]\nok", "[Verse 1\nbroken\n[Chorus]\nc",
        "[Verse 1]\r\na\r\n[Chorus]\r\nb", "  [Verse 1]  lyric after\nnext line\n\n[Outro]", "[Rap]\nx\n[Spoken Word]\ny\n[Instrumental Break]\nz\n[Solo]\nw\n[Breakdown]\nq", "[Verse I]\nx\n[Verse IV]\ny",
        "[Chorus]\na\n[Verse 1]\nb\n[Chorus]\nc\n[Chorus]\nd", "[Verse 1]\n" + "line\n" * 50, "[Dark Atmosphere][Verse 1]\nx", "[silence]\nx\n[Verse 1]\ny", "[Verse 1] [Chorus]\nx",
    ]
    for text in lyric_texts:
        add("_parse_story_arc_lyric_sections", text, True)
        add("_parse_story_arc_lyric_sections", text, False)
    long_text = " ".join(f"word{i}" for i in range(300))
    sentence_text = "First sentence is here. " * 20 + "Second part goes on and on without any more stops " * 12
    for text, maximum in (("", 100), ("a b c", 100), ("a b c d e", 3), (long_text, 100), (long_text, 30), (sentence_text, 100), (sentence_text, 150), ("x. " * 200, 100), ("Hello, world; and: more " * 30, 50),
                          ("Scene 1 (A) — one two three four five six\nScene 2 (B) — seven eight nine ten eleven twelve", 10), ("Scene 1 (A) — " + long_text + "\nScene 2 (B) — " + long_text, 100),
                          ("- Scene 1 - a b c\n* Scene 2 - d e f\nScene 3 - g h i", 24), ("Scene 1 only line", 2), ("Scene 1 a b\nnot a scene line", 3), ("\u2022 Scene 4 \u2014 " + long_text + "\n\u2022 Scene 5 \u2014 short", 40),
                          ("caf\u00e9 \u00e9t\u00e9 " * 80 + "fin. " + "na\u00efve " * 70, 100)):
        add("_cap_story_arc_words", text, maximum)
    for count, per, total in ((0, 100, 1500), (1, 100, 1500), (6, 100, 1500), (40, 100, 1500), (100, 100, 1500), (6, 240, 3600), (6, 60, 1000), (20, 160, 2400), (1000, 100, 1500)):
        add("_story_arc_section_word_limit", count, per, total)
    rows = [{"scene_number": n, "lyric_section": s, "location": "", "lyric": ""} for n, s in enumerate(["Intro", "Verse 1", "Verse 1", "Chorus", "Verse 2", "Chorus", "Outro"], start=1)]
    add("_story_arc_scene_map", rows, LABELS)
    add("_story_arc_scene_map", rows, ["Intro", "Verse 1", "Chorus", "Verse 2", "Chorus 2", "Outro"][:6])
    add("_story_arc_scene_map", rows, ["Verse 1", "Chorus"])
    add("_story_arc_scene_map", rows, ["Verse 1", "Verse 2", "Bridge"])
    add("_story_arc_scene_map", rows, [])
    add("_story_arc_scene_map", [], LABELS)
    add("_story_arc_scene_map", [5, "x"], LABELS)
    add("_story_arc_scene_map", [{"scene_number": 1, "lyric_section": "[Instrumental]"}, {"scene_number": 2, "lyric_section": "Verse 1"}, {"scene_number": 3, "lyric_section": "Chorus"}], ["Verse 1", "Chorus"])
    add("_story_arc_scene_map", [{"scene_number": 1, "lyric_section": "Verse 1"}, {"scene_number": 2, "lyric_section": None}, {"scene_number": 3, "lyric_section": "verse 1"}], ["Verse 1"])
    add("_story_arc_scene_map", [{"scene_number": n, "lyric_section": "Chorus"} for n in range(1, 4)] + [{"scene_number": 4, "lyric_section": "Verse 1"}], ["Chorus", "Verse 1"])
    good = "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)
    arc_texts = {
        "good": good,
        "padded": "\n\n  Intro:   " + PARAGRAPH + "\nVerse 1:\n" + PARAGRAPH + "\n\nChorus: " + PARAGRAPH + "\nVerse 2:\n" + PARAGRAPH + "\nChorus 2:\n" + PARAGRAPH + "\nOutro:\n" + PARAGRAPH,
        "brackets": "[Intro]:\n" + PARAGRAPH + "\n[Verse 1]:\n" + PARAGRAPH + "\n[Chorus]:\n" + PARAGRAPH + "\n[Verse 2]:\n" + PARAGRAPH + "\n[Chorus 2]:\n" + PARAGRAPH + "\n[Outro]:\n" + PARAGRAPH,
        "case": good.replace("Verse 1:", "VERSE 1:").replace("Outro:", "outro:"),
        "first_one": good.replace("Intro:", "Intro 1:").replace("Chorus:", "Chorus 1:"),
        "second_unnumbered": good.replace("Chorus 2:", "Chorus:"),
        "trailing": good + "\n\nBridge:\n" + PARAGRAPH + "\n\nExtra Outro:\n" + PARAGRAPH,
        "missing": "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS[:4]),
        "renamed": good.replace("Verse 2:", "Second Verse:"),
        "reordered": "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in [LABELS[0], LABELS[2], LABELS[1]] + LABELS[3:]),
        "instrumental_between": good.replace("\n\nVerse 2:", "\n\nInstrumental:\nA long guitar solo plays.\n\nVerse 2:"),
        "instrumental_first": "Instrumental:\nOpening music.\n\n" + good,
        "instrumental_last": good + "\n\nSilence:\nNothing happens.",
        "break_text": good.replace("\n\nOutro:", "\n\nB-Roll:\nStreet footage.\n\nOutro:"),
        "merged_runs": "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in ["Intro", "Verse 1", "Verse 1", "Verse 1", "Chorus", "Verse 2", "Chorus 2", "Outro"]),
        "meta_preamble": "User instructions:\nUse the exact sections.\n\n" + good,
        "meta_preamble_two": "Output format:\nok\n\nRequirements:\nok\n\n" + good,
        "invented_preamble": "Title:\nMy arc\n\n" + good,
        "entries": "\n\n".join(f"{label}:\nScene 1 (Kitchen): Mara walks.\nScene 2 (Porch): Joe waits." for label in LABELS),
        "entries_safe": "Scene 1 (Kitchen): Mara walks.\n" + good,
        "no_headings": "Just a paragraph with no headings at all, only prose that goes on " * 6,
        "empty": "",
        "colon_in_prose": good.replace("her hands steady", "her hands steady: calm"),
        "long_body": good.replace(PARAGRAPH, long_text, 1),
        "sentence_body": good.replace(PARAGRAPH, sentence_text, 1),
        "empty_section": good.replace(PARAGRAPH, "", 1),
        "many": "\n\n".join(f"Section {n}:\n{PARAGRAPH}" for n in range(1, 16)),
    }
    for key, text in arc_texts.items():
        add("_normalize_story_arc_output", text, LABELS, 100, "LM Studio")
    many_labels = [f"Section {n}" for n in range(1, 13)]
    add("_normalize_story_arc_output", "\n\n".join(f"Section {n}:\n{PARAGRAPH}" for n in range(1, 13)), many_labels, 40, "Gemma Local")
    add("_normalize_story_arc_output", "\n\n".join(f"Section {n}:\n{PARAGRAPH}" for n in range(1, 8)), many_labels, 40, "Gemma Local")
    add("_normalize_story_arc_output", "\n\n".join(f"Wrong {n}:\n{PARAGRAPH}" for n in range(1, 16)), many_labels, 40, "")
    add("_normalize_story_arc_output", "nothing useful " * 20, many_labels, 40, "Qwen Local")
    add("_normalize_story_arc_output", PARAGRAPH, [], 100, "LM Studio")
    add("_normalize_story_arc_output", long_text, [], 50, "LM Studio")
    add("_normalize_story_arc_output", "Verse:\nfree form\n\nChorus:\nmore", [], 50, "LM Studio")
    add("_normalize_story_arc_output", "", [], 50, "LM Studio")
    add("_normalize_story_arc_output", good, ["Verse 1"], 100, "LM Studio")
    add("_normalize_story_arc_output", "Verse 1:\n" + PARAGRAPH, ["Verse 1"], 20, None)
    for labels, text in ((["A", "B"], "A:\nx\n\nB:\ny"), (["A", "B"], "a:\nx\n\nb:\ny"), (["A", "A 2"], "A:\nx\n\nA:\ny"), (["Chorus", "Chorus 2"], "Chorus 1:\nx\n\nChorus 2:\ny")):
        add("_split_story_arc_sections", text, labels)
    add("_split_story_arc_sections", good, LABELS)
    add("_split_story_arc_sections", good + "\n\nVerse 1:\nrepeat", LABELS)
    add("_split_story_arc_sections", "intro text\n" + good, LABELS)
    add("_split_story_arc_sections", "", LABELS)
    add("_split_story_arc_sections", good, [])
    add("_split_story_arc_sections", "  Verse 1:  \nx\n  Chorus :\ny", ["Verse 1", "Chorus"])
    for payload, storyboard, detail in (({}, {}, "standard"), ({}, {}, "detailed"), ({}, {}, "rich"), ({}, {}, "compact"), ({"story_arc_scene_entries": True}, {}, "standard"), ({"story_arc_scene_entries": False}, {}, "rich"),
                                        ({"storyArcSceneEntries": "yes"}, {}, "standard"), ({"story_arc_scene_entries": "off"}, {}, "rich"), ({"story_arc_scene_entries": ""}, {}, "rich"), ({"story_arc_scene_entries": None}, {}, "rich"),
                                        ({}, {"story_arc_scene_entries": True}, "compact"), ({"story_arc_scene_entries": 0}, {"story_arc_scene_entries": True}, "compact"), ({"story_arc_scene_entries": " TRUE "}, {}, "compact"),
                                        ({"story_arc_scene_entries": 1}, {}, "compact"), ({"story_arc_scene_entries": []}, {}, "rich"), ({"story_arc_scene_entries": "1"}, {}, "compact")):
        add("_story_arc_scene_entries_enabled", payload, storyboard, detail)
    return out


GUARD_SPECS = {
    "1": {"all": [MARA, JOE], "cast": [MARA], "extras": []},
    "2": {"all": [MARA, JOE], "cast": [MARA, JOE], "extras": []},
    "3": {"all": [MARA, JOE], "cast": [], "extras": []},
    "4": {"all": [MARA, JOE], "cast": [JOE], "extras": ["Dancer"]},
}
ENTRY_TEXTS = [
    "Verse 1:\nScene 1 (Kitchen) \u2014 Mara carries the lamp. Joe waits by the door.\nScene 2 (Porch) \u2014 Joe watches. A woman stands behind him.",
    "Scene 3 (Hall) \u2014 She looks up. He nods.\nScene 4 - Joe waves. A crowd gathers.\nplain line without entry\n- Scene 1 \u2013 Mara walks. Joe follows.",
    "Scene 3 (Hall) \u2014 Mara and Joe talk.\nScene 9 (Roof) \u2014 Nobody here.\nScene 1 (Kitchen)  \u2014  Joe leaves alone.",
    "Scene 1 (Kitchen) \u2014 They stand side by side.\n\nScene 2 (Porch) \u2014 Mara sits.",
    "",
    "Scene 1 \u2014 Joe.",
]


def strip_cases():
    return [{"text": text} for text in ENTRY_TEXTS]


def run_helpers(story_layer, cast_guard):
    out = []
    for name, args in helper_cases():
        try:
            result = {"return": getattr(story_layer, name)(*args)}
        except Exception as exc:  # noqa: BLE001
            result = {"error": str(exc)}
        out.append({"fn": name, "args": args, **result})
    names = {1: ["Mara"], 2: ["Mara", "Joe"], 3: [], 4: ["Joe"]}
    for item in strip_cases():
        guards = {int(n): cast_guard.build_cast_guard(spec["all"], spec["cast"], spec["extras"]) for n, spec in GUARD_SPECS.items()}
        for with_names in (False, True):
            out.append({"fn": "strip_story_arc_entry_leaks", "args": {"text": item["text"], "guards": GUARD_SPECS, "names": names if with_names else {}},
                        "return": cast_guard.strip_story_arc_entry_leaks(item["text"], guards, names if with_names else None)})
    return out


def scene(n, section, lyric, cast, location="Kitchen", **extra):
    base = {"scene_number": n, "label": f"Scene {n}", "lyric_section": section, "lyrics": lyric, "subject_refs": cast, "location_ref": {"name": location, "description": f"a {location.lower()} with warm light"} if location else None}
    if not cast:
        base["no_character_present"] = True
    return {**base, **extra}


SCENES = [
    scene(1, "Intro", "", [], "Kitchen"), scene(2, "Verse 1", "walking home alone", [MARA], "Kitchen"), scene(3, "Verse 1", "the street is empty", [MARA], "Street"),
    scene(4, "Verse 1", "I count the lights", [MARA], "Street"), scene(5, "Chorus", "oh oh oh", [MARA], "Porch"), scene(6, "Chorus", "never going back", [MARA, JOE], "Porch"),
    scene(7, "Verse 2", "morning comes", [JOE], "Kitchen"), scene(8, "Chorus", "oh oh oh", [MARA], "Porch"), scene(9, "Outro", "", [], None),
]
SECTION_JSON = json.dumps({label: PARAGRAPH for label in LABELS})
ENTRY = "Medium shot, slow push in. Mara lifts the lamp and carries it to the sill, her hands steady. Warm light spills across the counter."
LEAK = "Medium shot. A woman stands beside Joe near the window, and Mara feels grief and longing."
REFERENCE = {"subjects": [MARA, JOE], "locations": [{"name": "Kitchen", "description": "warm"}, {"name": "Shed", "description": "cold"}]}
SCRIPT = {
    "enabled": True, "raw_text": "MARA: Hello.\nJOE: Hi there.", "cues": [{"speaker_alias": "MARA", "text": "Hello.", "speaker_id": "mara"}, {"speaker_alias": "JOE", "text": "Hi there.", "speaker_name": "Joe"}],
    "scene_plan": {"scenes": [{"index": 1, "duration_seconds": 6, "speaker_assignments": [{"speaker_id": "mara", "speaker_name": "Mara", "text": "Hello."}]},
                              {"index": 2, "duration_seconds": 5, "continuation_of_previous": True, "speaker_assignments": [{"speaker_alias": "JOE", "text": "Hi there."}]}]},
}


def arc_cases():
    pf = {"project_folder": "<TMP>"}
    detailed = {"story_arc_detail": "detailed", **pf}
    base = {"scenes": SCENES, "reference_builder": REFERENCE, **pf}
    entries = [ENTRY] * 9
    out = [
        ({**base}, [SECTION_JSON]),
        ({**base, "seed": 41, "story_arc_seed": "41", "temperature": 0.7, "top_p": 0.5, "max_new_tokens": 5000}, [SECTION_JSON]),
        ({**base, "story_idea": "A long night.", "previous_story_arc": "Old arc.", "style_theme": "Noir.", "performance_style": "Natural", "facial_performance": "Subtle", "camera_flow": "Smooth flow",
          "camera_motion_speed": "9", "character_motion": 2}, [SECTION_JSON]),
        ({**base, "storyArcDetail": "compact", "cameraMotionSpeed": 11, "characterMotionSpeed": -3}, [SECTION_JSON]),
        ({**base, "storyboard": {"story_arc_detail": "rich", "camera_flow": "From storyboard", "camera_motion_speed": 2, "character_motion_speed": 9, "performance_style_default": "Bold", "facial_performance_default": "Wide"}}, [SECTION_JSON] + entries),
        ({**base, "story_layer": {"song_story_brief": "A quiet song.", "lyric_story_strength": 3, "overall_story_idea": "From layer."}}, [SECTION_JSON]),
        ({**base, **detailed}, [SECTION_JSON] + entries),
        ({**base, **detailed}, [SECTION_JSON] + entries[:4]),
        ({**base, **detailed}, [SECTION_JSON, ENTRY, LEAK, ENTRY, ENTRY, ENTRY, ENTRY, ENTRY, ENTRY, ENTRY]),
        ({**base, **detailed}, [SECTION_JSON, ENTRY, LEAK, LEAK] + entries[3:]),
        ({**base, **detailed}, [SECTION_JSON, "Scene 2 (Kitchen) \u2014 " + ENTRY, "Scene 3: " + ENTRY] + entries[2:]),
        ({**base, **detailed, "timeline_markers": [{"start": 0.0, "end": 4.0, "label": "rain", "note": "it rains", "type": "note"}]}, [SECTION_JSON] + entries),
        ({**base, "story_arc_detail": "standard", "story_arc_scene_entries": True}, [SECTION_JSON] + entries),
        ({**base, "story_arc_detail": "rich", "story_arc_scene_entries": False}, [SECTION_JSON]),
        ({**base, **detailed, "scenes": [{**s, "lyric_section": "Mystery"} for s in SCENES]}, [SECTION_JSON]),
        ({**base, "text_runner": "llm_api"}, ["\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)]),
        ({**base, "text_runner": "llm_api", **detailed}, ["\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)] + entries),
        ({**base, "text_runner": "llm_api"}, ["Verse 1:\nonly this", "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)]),
        ({**base, "text_runner": "llm_api", "seed": 5}, ["Verse 1:\nonly this", "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)]),
        ({**base, "text_runner": "llm_api", "seed": "x"}, ["Verse 1:\nonly this", "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)]),
        ({**base, "text_runner": "llm_api", "seed": 2147483646}, ["no headings here", "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)]),
        ({**base, "text_runner": "llm_api"}, ["Verse 1:\nonly this", "Verse 1:\nstill wrong"]),
        ({**base, "text_runner": "llm_api"}, ["no headings", ""]),
        ({**base, "text_runner": "llm_api"}, ["no headings", "still none at all"]),
        ({**base}, ["not json at all", "\n\n".join(f"{label}:\n{PARAGRAPH}" for label in LABELS)]),
        ({**base}, ["not json at all", "Verse 1:\nwrong", SECTION_JSON]),
        ({**base}, ["not json at all", "Verse 1:\nwrong", json.dumps({"Intro": "only"})]),
        ({**base}, ["not json at all", "Verse 1:\nwrong", json.dumps({label: "" for label in LABELS})]),
        ({**base}, [json.dumps({"Intro": "partial"}), "Verse 1:\nwrong", SECTION_JSON]),
        ({**base}, ["", ""]),
        ({**base, "text_runner": "llm_api"}, [""]),
        ({"scenes": SCENES, "text_runner": "llm_api"}, [PARAGRAPH]),
        ({"scenes": SCENES}, [PARAGRAPH]),
        ({"scenes": [], "lyrics": ""}, [PARAGRAPH]),
        ({"scenes": SCENES, "line_mapping_lyrics": LYRICS}, [SECTION_JSON]),
        ({"scenes": SCENES, "line_mapping_lyrics": LYRICS, "story_arc_detail": "detailed"}, [SECTION_JSON] + entries),
        ({"scenes": SCENES, "lyrics": "[Verse 1]\nwalking home\n[Verse 1]\nthe street\n[Chorus]\noh\n[Chorus]\nagain"}, [json.dumps({"Verse 1": "a", "Chorus": "b"})]),
        ({"scenes": SCENES, "lyrics": "[Verse 1]\nwalking home\n[Verse 1]\nthe street\n[Chorus]\noh\n[Chorus]\nagain", "text_runner": "llm_api"}, ["Verse 1:\na\n\nChorus:\nb"]),
        ({"scenes": SCENES, "lyrics": "[Verse 1]\nwalking home", "projectFolder": "<TMP>"}, [json.dumps({"Intro": "a", "Verse 1": "b", "Chorus": "c", "Verse 2": "d", "Chorus 2": "e", "Outro": "f"})]),
        ({"scenes": SCENES, "project_folder": "C:/nope/missing", "lyrics": "[Verse 1]\nwalking home"}, [json.dumps({"Verse 1": "b"})]),
        ({"scenes": "x", "project_folder": "<TMP>"}, [SECTION_JSON]),
        ({"scenes": [5, "x", *SCENES[:2]], "project_folder": "<TMP>"}, [SECTION_JSON]),
        ({"script_import": SCRIPT, "story_layer": {"overall_story_idea": "A quiet drama."}, "reference_builder": REFERENCE}, ["A short film premise about two people."]),
        ({"scriptImport": SCRIPT, "story_arc_seed": 3, "timeline_markers": [{"start": 1.0, "end": None, "note": "door opens", "type": "note"}]}, ["A premise."]),
        ({"storyboard": {"script_import": SCRIPT}, "seed": 12}, ["A premise."]),
        ({"script_import": SCRIPT}, ["   "]),
        ({"script_import": {**SCRIPT, "raw_text": ""}, "max_new_tokens": 700, "temperature": 0.2, "top_p": 0.8}, ["Premise text."]),
        ({"script_import": {**SCRIPT, "enabled": False}, **base}, [SECTION_JSON]),
        ({"script_import": {"enabled": True, "cues": []}, **base}, [SECTION_JSON]),
        ({**base, "temperature": "hot"}, []),
        ({**base, "max_new_tokens": "many"}, []),
    ]
    return out


def brief_cases():
    base = {"lyrics": LYRICS, "scenes": SCENES}
    return [
        ({**base}, ["Story premise: A walk.\n\nEmotional arc: calm."]),
        ({**base, "lyrics_text": "x", "story_layer": {"user_story_arc": "Verse 1: she waits.", "lyric_story_strength": 2}}, ["A brief."]),
        ({"lyrics": "", "scenes": [], "story_layer": {"user_story_arc": "Verse 1: she waits."}}, ["A brief."]),
        ({"lyrics": "", "scenes": SCENES[:2]}, ["A brief."]),
        ({"lyrics": "just words", "scenes": []}, ["A brief."]),
        ({"lyrics": "", "scenes": []}, []),
        ({"lyrics": "", "scenes": [5, "x"]}, []),
        ({**base}, [""]),
        ({**base}, ["   "]),
        ({**base, "temperature": 0.9, "top_p": 0.6, "max_new_tokens": 300}, ["A brief."]),
        ({**base, "scenes": [{**SCENES[1], "extra_subjects": [{"name": "Dancer", "count": 3, "interaction": "alongside"}, {"name": ""}, {"name": "Fan"}, 5]}], "text_runner": "llm_api"}, ["A brief."]),
        ({"lyrics": "x" * 20000, "scenes": [{**SCENES[1], "lyrics": "y" * 900}] * 3}, ["z" * 5000]),
        ({"script_import": SCRIPT, "story_layer": {"overall_story_idea": "A quiet drama."}}, ["A film brief."]),
        ({"scriptImport": SCRIPT, "reference_builder": REFERENCE}, ["A film brief.", "unused"]),
        ({"storyboard": {"scriptImport": SCRIPT}}, [""]),
        ({"lyrics": 5, "scenes": SCENES[:1]}, ["A brief."]),
    ]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    recorder, modules = h.install(args.pack, ["storyboard.story_layer"])
    layer = modules["storyboard.story_layer"]
    prompts = importlib.import_module("vrgdg_pack.llm.prompts.storyboard")
    cast_guard = importlib.import_module("vrgdg_pack.storyboard.cast_guard")
    runner = importlib.import_module("vrgdg_pack.llm.builder_runner")
    OUT.mkdir(parents=True, exist_ok=True)

    cases = run_prompts(prompts)
    (OUT / "arc_prompts.json").write_text(json.dumps({"cases": cases}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"arc_prompts.json: {len(cases)} cases")

    helpers = run_helpers(layer, cast_guard)
    (OUT / "arc_helpers.json").write_text(json.dumps({"cases": helpers}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"arc_helpers.json: {len(helpers)} cases, errors {sum(1 for c in helpers if 'error' in c)}")

    # The calls also record a seed that differs from the request's own (the format retry bumps it).
    plain = runner._run_builder_text_llm

    def with_seed(payload, instruction, **kwargs):
        result = plain(payload, instruction, **kwargs)
        if payload.get("seed") != recorder.base_seed:
            recorder.calls[-1]["seed"] = payload.get("seed")
        return result

    runner._run_builder_text_llm = with_seed
    folder, _ = h.temp_files({})
    context = Path(folder.name) / "project_context"
    context.mkdir()
    (context / "full_lyrics.txt").write_bytes(("\ufeff" + LYRICS).encode("utf-8"))
    steps = []

    def failing(function, payload):
        try:
            return function(payload)
        except layer.StoryArcFormatError as exc:
            return {"__format_error__": {"message": str(exc), "runner": exc.runner, "expected_sections": exc.expected_sections, "raw_output": exc.raw_output, "cleaned_output": exc.cleaned_output}}

    with folder:
        real = str(Path(folder.name).resolve())
        for name, function, request_cases in (("arc", layer._build_story_layer_arc, arc_cases()), ("brief", layer._build_story_layer_brief, brief_cases())):
            for request, replies in request_cases:
                payload = {**RUNNER, **json.loads(json.dumps(request).replace("<TMP>", json.dumps(real)[1:-1]))}
                recorder.base_seed = payload.get("seed")
                outcome = h.run_case(recorder, lambda p, f=function: failing(f, p), payload, replies)
                steps.append({"fn": name, "request": request, "replies": replies, **h.scrub(outcome, folder.name)})
    (OUT / "arc_steps.json").write_text(json.dumps({"cases": steps}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"arc_steps.json: {len(steps)} cases, errors {sum(1 for c in steps if 'error' in c)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
