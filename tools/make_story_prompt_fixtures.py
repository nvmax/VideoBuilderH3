"""Dev tool: golden cases for the Story Mode instruction builders (`llm/prompts/story_mode.py`), from the pack's own functions.

    python tools/make_story_prompt_fixtures.py --pack <pack folder>

Writes ``fixtures/story/prompts.json``: each builder with its keyword arguments and the text it returned.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

CAST = "- Mara (character, main character): a woman with red hair\n  Personality and emotions: guarded\n- Joe (character, supporting)\n\nRelationships:\n- Mara and Joe: exes (estranged)"
PLACES = "Kitchen: warm and small\nBack porch"
TEMPLATE = "Custom {{medium_rule}} with {{acts}} acts and {{scene_count}} scenes. {{cast_rule}} {{lyric_guidance}} {{unknown}}"


def cases():
    out = []

    def add(name, **kwargs):
        out.append((name, kwargs))

    for strength in (None, 0, 2, 5, 7.9, 9, 12, "x", "3"):
        add("story_mode_lyric_guidance", strength=strength)
    for count, acts in ((1, 0), (7, 0), (20, 0), (46, 0), (46, 2), (0, 0), (100, 12)):
        add("story_book_size", scene_count=count, act_count=acts)
    for cast, allow in ((CAST, False), (CAST, True), ("", False), ("- Solo (character, main character)", False)):
        add("story_cast_rule", cast_text=cast, allow_new_cast=allow)
    for style, lead in (("music_video", "Mara"), ("spoken", ""), ("story_only", "Mara"), ("other", "")):
        add("story_medium_rule", video_style=style, lead_name=lead)
    for text in ("", "  ", "Kitchen"):
        add("user_places_block", places_text=text)
        add("made_places_block", made_places_text=text)
    for template in (None, TEMPLATE):
        for places in ("", PLACES):
            add("story_book_instruction", premise="A long night.", cast_text=CAST, tone_notes="noir", scene_count=24, song_seconds=187.4, lyric_guidance="Lyric Story Strength: 5/10.",
                lyrics="0:00-0:12 Intro (no vocals)", previous_book="", seed="7", allow_new_cast=False, video_style="music_video", lead_name="Mara", template=template, act_count=0, places_text=places)
    add("story_book_instruction", premise="", cast_text="", tone_notes="", scene_count=5, song_seconds=30, lyric_guidance="g", lyrics="", previous_book="old", seed="", act_count=4)
    for template in (None, TEMPLATE):
        add("story_chapter_rewrite_instruction", book_text="Title: T\n\nAct 1: One\n\nText.", chapter_number=2, chapter_text="Act 2: Two\n\nMore text.", direction="Make it darker.", cast_text=CAST,
            lyric_guidance="g", allow_new_cast=False, video_style="spoken", lead_name="Mara", template=template, places_text=PLACES)
    add("story_chapter_rewrite_instruction", book_text="b", chapter_number=1, chapter_text="c", direction="", cast_text="", lyric_guidance="g")
    add("story_split_instruction", numbered_paragraphs="[1] a\n\n[2] b", fixed_starts=[1, 3], scene_count=24)
    add("story_split_instruction", numbered_paragraphs="[1] a", fixed_starts=[], scene_count=3)
    add("story_remove_people_instruction", story_text="Act 1: One\n\nText", cast_text=CAST, names=["Zed", "Amy"])
    add("story_remove_people_instruction", story_text="t", cast_text="", names=["Zed"])
    add("story_cast_scan_instruction", book_text="Story text", known_names="- Mara\n- Joe")
    add("story_cast_scan_instruction", book_text="Story text", known_names="")
    for made, places in (("", ""), ("- Kitchen: warm", ""), ("", PLACES), ("- Kitchen: warm", PLACES)):
        for template in (None, TEMPLATE):
            add("story_acts_instruction", chapter_number=1, chapter_title="One", setting_name="Home", chapter_text="Text.", scenes_text="1. 0:00 4s", cast_text=CAST, notes_text="",
                target_acts=3, template=template, places_text=places, made_places_text=made)
    add("story_acts_instruction", chapter_number=2, chapter_title="Two", setting_name="", chapter_text="T", scenes_text="s", cast_text="", notes_text="Scene 3: rain", target_acts=2)
    for style in ("music_video", "spoken", "story_only", "x"):
        for made, places in (("", ""), ("- Kitchen: warm", PLACES), ("", PLACES)):
            add("story_scene_plan_instruction", act_title="Wait", act_summary="She waits.", setting_name="Home", chapter_text="Text.", scenes_text="Scene 1: 0:00", cast_text=CAST, spot_min_scenes=3,
                previous_spot="Kitchen", lyric_guidance="g", known_spots="Kitchen, Porch", video_style=style, act_cast="Mara, Joe", main_name="Mara", template=None, places_text=places, made_places_text=made)
    add("story_scene_plan_instruction", act_title="A", act_summary="s", setting_name="", chapter_text="c", scenes_text="s", cast_text="", spot_min_scenes=2, previous_spot="", lyric_guidance="g",
        known_spots="", template=TEMPLATE)
    add("story_act_summary_instruction", act_title="Wait", setting_name="Home", scenes_text="1. x", user_text="Where: kitchen", cast_text=CAST, old_summary="Old.")
    add("story_act_summary_instruction", act_title="Wait", setting_name="", scenes_text="1. x", user_text="", cast_text="", old_summary="")
    for places in ("", PLACES):
        for template in (None, TEMPLATE):
            add("story_setting_looks_instruction", settings_text="- Home", book_summary="Chapter 1", style_theme="noir", cast_text=CAST, template=template, places_text=places)
    add("story_setting_looks_instruction", settings_text="- Home", book_summary="c", style_theme="", cast_text="")
    for template in (None, TEMPLATE):
        add("story_spots_instruction", setting_name="Home", setting_look="warm wood", spots_text="- Kitchen: x", style_theme="noir", template=template)
    add("story_spots_instruction", setting_name="Home", setting_look="", spots_text="- Kitchen: x", style_theme="")
    add("story_place_list_instruction", settings_text="- Home: text", user_places_text=PLACES, style_theme="noir")
    add("story_place_list_instruction", settings_text="- Home: text")
    for template in (None, TEMPLATE):
        add("story_revise_scene_instruction", book_text="book", chapter_number=1, chapter_title="One", setting_name="Home", chapter_text="Chapter text", act_title="Wait", act_summary="She waits.",
            act_scenes_text="1. x", scene_number=3, scene_cast="Mara, Joe", user_note="Joe appears", cast_text=CAST, lyric_guidance="g", template=template)
    add("story_revise_scene_instruction", book_text="b", chapter_number=1, chapter_title="One", setting_name="", chapter_text="c", act_title="A", act_summary="s", act_scenes_text="s",
        scene_number=1, scene_cast="", user_note="", cast_text="", lyric_guidance="g")
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    module = importlib.import_module("vrgdg_pack.llm.prompts.story_mode")
    out = []
    for name, kwargs in cases():
        result = getattr(module, name)(**kwargs)
        if isinstance(result, tuple):
            result = list(result)
        out.append({"fn": name, "args": kwargs, "return": result})
    target = Path(__file__).resolve().parents[1] / "fixtures" / "story" / "prompts.json"
    target.write_text(json.dumps({"cases": out}, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"prompts.json: {len(out)} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
