"""Dev tool: write docs/BUILDER_PARITY.md, the list of everything the Video Builder front end asks the server for, with what
this app serves and what is left.

    python tools/parity_report.py

It reads fixtures/api/builder_routes.json (from tools/inventory_builder_routes.py) and the route files of crates/h3-api.
"""

import json
import re
from collections import OrderedDict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Routes this app does not need. The first matching rule wins.
EXCLUDED = [
    (r"build_(i2v|t2v|rtv|ingredients|flf|id_lora)_prompt$|generate_(i2v|chained_i2v)$|(enhance|edit)_video_prompt$|id_lora_dialogue_scenes$|face_fix/|storyboard/gemma_video_prompt$", "LTX only (MiniMax H3 scenes use the Builder's own prompt writers)"),
    (r"browser_image/|flow_gpt", "Browser automation of other sites"),
    (r"wizard_draft$|import_latest_prompt_creator_outputs$|copy_prompt_creator_outputs$|music_prompt_creator/(?!build_whisper_prompt)", "Wizard and Prompt Creator panel (not on the screens this app keeps)"),
    (r"build_minimax_h3_3pass_prompt$|build_transcribe_prompt$|luts/example$|luts/apply_image$|film_grain/apply_image$|latent_status$|list_scene_latent_takes$", "Nothing in the front end calls it"),
]

GROUPS = OrderedDict([
    ("Projects and files", r"music_builder/(list_projects|load_session|save_session|new_project|delete_project|save_project_as|export_project|import_project|load_srt|load_prompt_json|load_text_file|save_text_file|open_local_file|pick_path|default_|project_prompt|save_render_log|error_log|delete_project_media|audio$)"),
    ("Settings, profiles and defaults", r"music_builder/(model_defaults|llm_runner|list_|load_|save_|set_last|delete_)(video|ui|default_ui|scene_defaults)|model_root|post_process/adjust/presets"),
    ("Timeline, scenes and latents", r"renumber|scan_scene_videos|restore_scene_video|latent|dirty|archive_scene_image|save_scene_image|extract_video_final_frame"),
    ("Audio and Audio Mask", r"audio_mask|analyze_audio|scene_audio|project_audio|project_srt|single_scene_srt|trim_scene_audio|silent_audio|audio_mix|audio_clip_mix|preview_scene_audio|capcut|elevenlabs|dialogue|generate_scene_speech|prepare_scene_audio_clip"),
    ("Render (graphs and files)", r"workflow_runner/(build_|collect|trim|match|find|cleanup|stitch|render|save_image|prepare)"),
    ("LLM and prompts", r"gemma|llm_api|test_llm|own_server|lm_studio|instruction|clear_memory_direct|generate_|agent_chat|edit_image_prompt|enhance_minimax|describe_reference|analyze_story|flux_reference|lm_scout|wizard_locations|gemma4"),
    ("References and RefMods", r"reference|refmod"),
    ("Story Mode and Story Builder", r"storyboard/"),
    ("Post process", r"luts|post_process|film_grain|adjust/"),
    ("System", r"resource-monitor|update/|custom_nodes|video_editor|events"),
])

UI_ITEMS = [
    ("1", "Story Mode and the Story Builder", "Story Builder button, story layer, acts, locations, scene story beats", "storyboard routes"),
    ("2", "UI layout options", "UI Layout menu, default layout, named layouts, + and - sizes", "UI profile routes (served)"),
    ("3", "Minimise buttons for the left and right panes", "The two arrows beside the preview", "front end only"),
    ("4", "Scenes, Tools and Post Process", "Scene cards, scene tools, LUT, film grain and adjust", "post process routes"),
    ("5", "Timeline tools and buttons", "Timeline Tools bar, Bulk, notes, Segment, Audio Mask, Hear and Show Stems", "timeline, audio and mask routes (mask served)"),
    ("6", "Audio Mask (Hear Stems, Show Stems)", "Stem lanes, regions, masked mix", "mask routes (served, checked against the pack)"),
    ("7", "Stop, Models and Clear RAM", "Right side utility buttons and the VRAM and RAM meters", "system routes (served), Models dialog"),
    ("8", "Image, Video and Audio tabs", "Right panel tabs and their settings", "image and H3 graph routes (served)"),
    ("9", "Ref Builder, RefMods Studio and Viewer, Line Mapping, LLM Runner, Prompt Options", "Header buttons and their dialogs", "reference, RefMod, LLM and lyric routes"),
    ("10", "Ctrl and shift in the timeline, scroll to zoom", "Timeline mouse handling", "front end only"),
    ("11", "Hold ctrl to select several scenes", "Timeline selection", "front end only"),
    ("12", "Right click menu on timeline scenes", "Scene context menu", "front end only"),
    ("13", "Pop out prompt window and pop out viewer", "Independent windows that can move to another screen", "needs the desktop shell (own windows)"),
    ("14", "Space plays all scenes one after another", "Timeline playback", "front end only"),
]


def served_paths():
    text = "".join(p.read_text(encoding="utf-8") for p in (ROOT / "crates" / "h3-api" / "src" / "vrgdg").glob("*.rs"))
    paths = set(re.findall(r'"(/vrgdg/[A-Za-z0-9_/\-.{}]+)"', text))
    for base in re.findall(r'let base = "(/vrgdg/[A-Za-z0-9_/\-.]+)"', text):
        paths |= {f"{base}/{name}" for name in re.findall(r'\{base\}/([A-Za-z0-9_/\-.]+)', text)}
    return paths


def excluded_reason(path):
    for pattern, reason in EXCLUDED:
        if re.search(pattern, path):
            return reason
    return ""


def group_of(path):
    for name, pattern in GROUPS.items():
        if re.search(pattern, path):
            return name
    return "Other"


def main():
    routes = json.loads((ROOT / "fixtures" / "api" / "builder_routes.json").read_text(encoding="utf-8"))["routes"]
    served = served_paths()
    rows = []
    for route in routes:
        path = route["path"]
        reason = excluded_reason(path)
        status = "served" if path in served else ("excluded" if reason else "not built")
        rows.append({**route, "status": status, "reason": reason, "group": group_of(path)})
    counts = {s: sum(1 for r in rows if r["status"] == s) for s in ("served", "not built", "excluded")}
    lines = [
        "# Video Builder parity",
        "",
        "The app runs the pack's own Video Builder front end, copied unchanged, so every button, pane and mouse behaviour is the",
        "Builder's. What the app has to provide is the server that front end talks to. This list is generated by",
        "`tools/parity_report.py` from `fixtures/api/builder_routes.json`.",
        "",
        f"**{len(rows)} routes**: {counts['served']} served, {counts['not built']} still to build, {counts['excluded']} left out on purpose.",
        "",
        "## What you asked to match",
        "",
        "| # | Item | Where it is | What it needs |",
        "|---|------|-------------|---------------|",
    ]
    lines += [f"| {n} | {item} | {where} | {needs} |" for n, item, where, needs in UI_ITEMS]
    lines += ["", "Not kept, as you said: Wizard Legacy, Wizard Beta, Auto Build, the Buy me a coffee link, Update to Latest, most of Settings.", ""]
    lines += ["## Routes by area", ""]
    for group in list(GROUPS) + ["Other"]:
        members = [r for r in rows if r["group"] == group]
        if not members:
            continue
        done = sum(1 for r in members if r["status"] == "served")
        left = sum(1 for r in members if r["status"] == "not built")
        lines += [f"### {group} ({done} served, {left} to build, {len(members) - done - left} left out)", "", "| Route | Status | Used by |", "|-------|--------|---------|"]
        for r in sorted(members, key=lambda r: (r["status"] != "not built", r["path"])):
            used = ", ".join(c.replace(".mjs", "").replace(".js", "") for c in r["callers"][:3]) or "-"
            status = r["status"] if not r["reason"] or r["status"] == "served" else f"left out: {r['reason']}"
            lines.append(f"| `{r['method']} {r['path'].replace('/vrgdg/', '')}` | {status} | {used} |")
        lines.append("")
    (ROOT / "docs").mkdir(exist_ok=True)
    (ROOT / "docs" / "BUILDER_PARITY.md").write_text("\n".join(lines), encoding="utf-8")
    print(f"docs/BUILDER_PARITY.md: {counts}")


if __name__ == "__main__":
    main()
