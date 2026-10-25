"""Dev tool: the tool catalog, endpoint list and OpenAPI file the app's built-in MCP server ships with.

    python tools/export_mcp_catalog.py [--pack <pack folder>]

Reads the pack's own MCP server (`mcp_server`, standard library only) for the tool names, descriptions and argument schemas agents already know, and
`agent_api/endpoints.json` / `agent_api/openapi.json` for the endpoints. What the app does not serve (the endpoints listed in `LEFT_OUT` of
`crates/h3-api/tests/route_coverage.rs`) is left out, and a few descriptions are reworded for an app that makes MiniMax H3 videos only.
Writes `crates/h3-api/src/mcp/catalog.json`, `endpoints.md` and `openapi.json`.
"""

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates" / "h3-api" / "src" / "mcp"
PARAM = re.compile(r"\{[^}]+\}")

# Named tools the app does not offer, or offers with other words. The key is the tool name.
DROP = {"api_request"}
SETTINGS_GROUPS = (
    "`settings` is grouped: {\"minimax_h3\": {...}, \"project\": {...}}. `project` holds video_engine, video_type (singing, speaking, no_lip_sync or story), "
    "timing_frozen, auto_save_enabled, omit_lyrics_from_video_prompts, automatic_memory_cleanup and scene_render_wait_hours. Only the keys you send change."
)


def served_endpoints(pack: Path):
    """The pack's endpoints (method, path) that the app serves."""
    listed = json.loads((ROOT / "fixtures" / "api" / "pack_endpoints.json").read_text(encoding="utf-8"))
    coverage = (ROOT / "crates" / "h3-api" / "tests" / "route_coverage.rs").read_text(encoding="utf-8")
    block = coverage.split("const LEFT_OUT", 1)[1].split("];", 1)[0]
    left_out = set(re.findall(r'\("([A-Z]+)",\s*"([^"]+)",\s*"[^"]*"\)', block))
    return {(e["method"], e["path"]) for e in listed} - left_out


def adjust(path: str, text: str) -> str:
    """The pack's wording for this app: no Storyboard cards, and the ElevenLabs key may come from App Settings."""
    text = text.replace(
        "The change is saved on the timeline scene and on its Storyboard card, the way the Builder saves an edit, and appears in the open Video Builder and Storyboard.",
        "The change is saved on the timeline scene and appears in the open Video Builder.")
    text = text.replace(
        "Scene-card fields are saved on the timeline scene with the Builder's keys and on the Storyboard card (when the project has a saved Storyboard, or the field is one only the card stores), and the open Video Builder and Storyboard show the change:",
        "Fields are saved on the timeline scene with the Builder's keys, and the open Video Builder shows the change (fields marked card only belong to a Storyboard card, which this app does not have):")
    if "API version (`v1`), pack version" in text:
        text = "API version (`v1`), the app's name and version, and the capabilities this app serves."
    if ("/elevenlabs" in path or "/voice-design" in path or path.endswith("/dialogue/generate")) and "App Settings" not in text:
        text += " The project's own key is used; without one, the key saved in H3 Studio's App Settings."
    return text


SCENE_ARGUMENT = (
    "The scene: its id (seg_...) or its number on the timeline, the one the Video Builder shows. Send \"11\" for Scene 11. "
    "Numbers move when scenes are added or removed, ids do not."
)
SCENES_ARGUMENT = "The scenes to work on, each an id or a timeline number (\"11\" for Scene 11)."
SCENE_TOOLS = {
    "scene_list": " Each scene has `number` (1 is the first scene, the number the Video Builder shows as Scene N) and `id`. To read one scene, call scene_get with its number as scene_id instead of searching this list.",
    "scene_get": " scene_id can be the scene's number: scene_id \"11\" is Scene 11.",
}


def name_scenes(tool: dict) -> None:
    """Say in every tool that a scene is given by its id or by its number on the timeline, which is how a user says it (Scene 11)."""
    props = tool["inputSchema"].get("properties", {})
    if isinstance(props.get("scene_id"), dict):
        props["scene_id"]["description"] = SCENE_ARGUMENT
    if isinstance(props.get("scene_ids"), dict):
        props["scene_ids"]["description"] = SCENES_ARGUMENT + (" Separate several with commas." if props["scene_ids"].get("type") == "string" else "")
    tool["description"] += SCENE_TOOLS.get(tool["name"], "")


def reword(name: str, tool: dict) -> dict:
    """The tool as the app offers it: no ticket numbers, and only what this app does."""
    text = adjust("", re.sub(r"\s*\(T\d+\)", "", tool["description"]).strip())
    schema = json.loads(json.dumps(tool["inputSchema"]))
    props = schema.get("properties", {})
    if "flf" in props.get("pipeline", {}).get("enum", []):
        props["pipeline"]["enum"] = [value for value in props["pipeline"]["enum"] if value != "flf"]
    if name == "project_update_settings":
        text = re.sub(r"`settings` is grouped:.*?Only the keys you send change\.", lambda _m: SETTINGS_GROUPS, text, flags=re.S)
    elif name == "post_apply":
        text = "Apply a LUT to one scene (op `lut`, with scene_id) or to every scene (op `apply_all`). The LUT goes in `settings`."
        props["op"]["enum"] = ["lut", "apply_all"]
    elif name == "pipeline_build_full_video":
        text = "Run the whole build for a project as one background job: prompts, images where needed, videos, and the stitch. Returns a job id for job_wait."
        props["pipeline"]["enum"] = ["full_video"]
    elif name == "story_generate":
        text = "Write the story arc, the story brief or the scene beats with the project's LLM, or concept prompts or motion notes. For the full story steps use story_create."
        props["what"]["enum"] = ["arc", "brief", "beats", "concepts", "motion_notes"]
    elif name == "lyrics_transcribe":
        text = "Time the project's lyrics against its song (same as lyrics_align). Runs as a job: use job_wait."
    elif name == "latents_status_rebuild":
        text = "Check the latent status of the scenes, list the dirty ones, rebuild the dirty chain, or delete a scene's latent."
    elif name == "audio_analyze":
        text = "Read the project's beat markers and tempo. They were found by the app's own beat tracker when the song was added."
    elif name == "instructions_get_set":
        text = "Read, save or reset a prompt-writing instruction (the system prompt an LLM step uses). `key` names it, `project_id` limits it to one project, `scope` is all or scene."
    result = {"name": name, "description": text, "inputSchema": schema}
    if tool.get("annotations"):
        result["annotations"] = tool["annotations"]
    return result


SPEAKING = """

---

## Part 5 - Speaking mode (a short film with ElevenLabs voices)

Speaking mode builds a film where characters say lines made with ElevenLabs. The ElevenLabs key is saved once in H3 Studio (Menu, App Settings).
You never see it and never set it.

1. `project_update_settings` with `{"project": {"video_type": "speaking"}}`. The speaking tools only work in this mode.
2. `api_get_elevenlabs` says whether a key is available (`configured`, `source`) and `api_post_elevenlabs_test` checks it. If there is none, stop and ask
   the user to add the key in App Settings.
3. `api_get_elevenlabs_voices` lists the account's voices (pass `next_page_token` for more). To make a new voice: `api_post_references_subjects_voice_design_description`
   (a job, the LLM writes the description), then `api_post_elevenlabs_voice_design_previews`, then `api_post_elevenlabs_voice_design_create` with the preview the user picked.
4. Give each speaking character a voice: `reference_upsert` with `kind: "subjects"` and `payload: {"elevenlabs_voice": {"enabled": true, "voice_id": "...", "name": "..."}}`.
5. For each scene with a spoken line: `api_patch_scenes_audio_settings` with `dialogue: {"speaker_id": ..., "text": ..., "delivery": ...}` saves the draft,
   `api_post_scenes_dialogue_craft` (a job) adds audio tags to the text, and `api_post_scenes_dialogue_generate` makes one MP3 take and returns it as `audio_data`.
6. Put the take on the scene with `api_patch_scenes_audio_settings` and `audio_data` plus `audio_name`. `silence_before`, `silence_after` (seconds) and
   `fit_duration` set the timing. `api_patch_speaking_audio_defaults` sets them for every scene.
7. Then render the scenes and stitch as in Part 3 (steps 15 to 18).
"""


def rewrite(text: str, pairs) -> str:
    for old, new in pairs:
        if old not in text:
            print(f"warning: the pack's text changed, no replacement for: {old[:70]!r}", file=sys.stderr)
        text = text.replace(old, new)
    return text


def write_docs(pack: Path) -> None:
    """The playbook and the chat prompt, worded for this app."""
    folder = OUT / "docs"
    folder.mkdir(exist_ok=True)
    playbook = (pack / "MUSIC_VIDEO_AGENT_PROMPT.md").read_text(encoding="utf-8")
    playbook = rewrite(playbook, [
        ("VRGDG MCP server, doing the same steps as the Video Builder, in the same order.", "H3 Studio MCP server, doing the same steps as the Video Builder, in the same order."),
        ("The agent must have the `vrgdg` MCP server connected (`python -m mcp_server` from this repository, with ComfyUI\nrunning at `http://127.0.0.1:8188`). If the tools below are missing, stop and tell the user.",
         "The agent must be connected to the H3 Studio MCP server (Menu, MCP Setup in the app shows how), with ComfyUI\nrunning. If the tools below are missing, stop and tell the user."),
        ("is done by the model that is already loaded in LM Studio. Never load, unload or switch models.\n   Call `llm_active` first. If it reports no loaded model, stop and ask the user to load one.",
         "is done by the LLM the app is set to use (the model already loaded in LM Studio, an own server or an API provider). Never load, unload or switch models.\n   Call `llm_active` first. If it reports no model, stop and ask the user to set one up."),
    ])
    (folder / "music_video_playbook.md").write_text(playbook.rstrip() + "\n" + SPEAKING, encoding="utf-8")
    chat = (pack / "CHAT_AGENT_PROMPT.md").read_text(encoding="utf-8")
    chat = rewrite(chat, [
        ("You have the VRGDG MCP tools, which\ndrive the AI Video Builder in ComfyUI.", "You have the H3 Studio MCP tools, which\ndrive the app's Video Builder."),
        ("is done by the model already loaded in LM Studio. Never load, unload or switch models.", "is done by the LLM the app is set to use. Never load, unload or switch models."),
        ("If ComfyUI is down or no LLM is loaded", "If ComfyUI is down or no LLM is available"),
        ("or `video_render` for that scene.",
         "or `video_render` for that scene.\n\nThe user names a scene by its number on the timeline (\"scene 11\"). Pass that number as `scene_id` (\"11\") to any tool,\n"
         "for example `scene_get` to read its prompt, and do not search `scene_list` for it. Numbers move when scenes are added or removed, ids do not."),
    ])
    (folder / "chat_agent_prompt.md").write_text(chat, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", type=Path, default=ROOT.parent)
    args = parser.parse_args()
    pack = args.pack.resolve()
    sys.path.insert(0, str(pack))
    from mcp_server import endpoint_tools as endpoints_module  # noqa: E402
    from mcp_server import tools as tools_module  # noqa: E402

    served = served_endpoints(pack)
    named = {name: tool for name, tool in tools_module.ALL_TOOLS.items() if name not in DROP}
    handwritten = {name: tool for name, tool in named.items() if not name.startswith("api_")}
    covered = endpoints_module.covered_by_handwritten_tools()
    all_endpoints = endpoints_module.load_endpoints()
    uncovered = [e for e in all_endpoints if not endpoints_module._is_covered(e, covered)]
    names = endpoints_module._unique_names(uncovered, set(handwritten))

    tools = [dict(reword(name, tool.to_mcp_dict()), kind="named") for name, tool in handwritten.items()]
    for index, endpoint in enumerate(uncovered):
        if (endpoint["method"], endpoint["path"]) not in served or endpoint["path"] == "/events":
            continue
        name = names[index]
        built = endpoints_module._build_tool(endpoint, name).to_mcp_dict()
        entry = {"name": name, "description": adjust(endpoint["path"], built["description"]), "inputSchema": built["inputSchema"], "kind": "endpoint"}
        if built.get("annotations"):
            entry["annotations"] = built["annotations"]
        entry["route"] = {
            "method": endpoint["method"], "path": endpoint["path"],
            "args": {param: endpoints_module._arg_name(param, endpoint["path"]) for param in endpoint["path_params"]},
            "job": bool(endpoint["job"]), "if_match": bool(endpoint["if_match"]),
        }
        tools.append(entry)
    for tool in tools:
        name_scenes(tool)

    listed = sorted((e for e in all_endpoints if (e["method"], e["path"]) in served and e["path"] != "/events"), key=lambda e: (e["path"], e["method"]))
    catalog = {"version": 1, "tools": tools, "endpoints": [
        {"method": e["method"], "path": e["path"], "description": adjust(e["path"], e["description"]), "job": bool(e["job"]), "body_keys": e["body_keys"], "query": e["query"], "if_match": bool(e["if_match"])} for e in listed
    ]}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "catalog.json").write_text(json.dumps(catalog, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")

    sections: dict = {}
    for e in listed:
        parts = [p for p in e["path"].split("/") if p]
        words = [p for p in parts if not PARAM.fullmatch(p) and p != "projects"]
        sections.setdefault(words[0] if words else "service", []).append(e)
    lines = ["# H3 Studio API endpoints", "",
             "Every endpoint of the agent API this app serves, under `/api/v1`. The MCP tools call these. `{pid}` is the project id, `{sid}` a scene id.",
             "A call that runs as a job returns a job id: poll `GET /jobs/{id}` (the `job_wait` tool does).", ""]
    for section in sorted(sections):
        lines += [f"## {section}", ""]
        for e in sections[section]:
            extras = []
            if e["body_keys"]:
                extras.append("body: " + ", ".join(e["body_keys"]))
            if e["query"]:
                extras.append("query: " + ", ".join(e["query"]))
            tail = f" ({'; '.join(extras)})" if extras else ""
            lines.append(f"- `{e['method']} {e['path']}`{' (job)' if e['job'] else ''} {adjust(e['path'], e['description'])}{tail}")
        lines.append("")
    (OUT / "endpoints.md").write_text("\n".join(lines), encoding="utf-8")

    spec = json.loads((pack / "agent_api" / "openapi.json").read_text(encoding="utf-8"))
    keep = {(m, PARAM.sub("{}", p)) for m, p in served}
    paths = {}
    for path, operations in spec["paths"].items():
        short = path.replace("/vrgdg/api/v1", "", 1)
        kept = {m: op for m, op in operations.items() if (m.upper(), PARAM.sub("{}", short)) in keep}
        if kept:
            paths[short] = kept
    spec["paths"] = paths
    spec["servers"] = [{"url": "http://127.0.0.1:8190/api/v1"}]
    spec["info"] = dict(spec.get("info", {}), title="VRGDG H3 Studio API")
    (OUT / "openapi.json").write_text(json.dumps(spec, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    write_docs(pack)
    print(f"{len(tools)} tools ({sum(1 for t in tools if t['kind'] == 'named')} named), {len(listed)} endpoints, {sum(len(v) for v in paths.values())} in the OpenAPI file")
    return 0


if __name__ == "__main__":
    sys.exit(main())
