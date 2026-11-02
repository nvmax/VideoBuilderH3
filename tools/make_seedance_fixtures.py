"""Dev tool: golden cases for the Seedance 2.5 prompt conversion, from the pack.

    python tools/make_seedance_fixtures.py --pack <pack folder>

Runs ``prepare_seedance_blocks`` (retag the references, translate the MiniMax marks, split the prompt into immutable blocks) and
``assemble_seedance_prompt`` (lay the blocks out under the LLM's headings, refusing a plan that drops or reorders one) of
``llm/seedance_conversion.py``. The file backup and the LLM call are not run: they hold no decisions. Also records the pack's plan
schema and conversion instructions.

Writes ``fixtures/prompt/seedance.json``. The Rust port (``h3_core::seedance``) must give the same blocks, errors and prompts.
"""

import argparse
import importlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

SHOTS = (
    "detailed_description:\n"
    "The target video is in a photorealistic cinematic music-video style.\n\n"
    "[Shot 1] <Subject 1> sings <d>[English, longing, singing] Hold me <i>close</i> tonight.</d> in sync with <Audio 1>. "
    "Hard cut, the same moment on the same stage. The action takes place in the environment from <Picture 2>.\n\n"
    "[Shot 2] Close-up of <Subject 1> spinning once while she sings."
)

SCENE_AUDIO_PROMPT = (
    "summary:\nAva sings on a roof.\n\n"
    "detailed_description:\n[Shot 1] <Subject 1> sings in sync with <Audio 1>, Original soundtrack playing. Lip-sync to the Original soundtrack.\n\n"
    "overall_soundscape:\nOriginal soundtrack"
)

ALIASES = {"<Picture 1>": "@Image 1", "<Picture 2>": "@Image 2", "<Subject 1>": "Ava"}


def scene(source_prompt, **extra):
    base = {"id": "s1", "label": "Scene 1", "source_key": "minimax_h3_prompt", "source_prompt": source_prompt,
            "image_references": [{"label": "Ava"}, {"label": "Roof"}], "reference_aliases": dict(ALIASES)}
    base.update(extra)
    return base


def prepare_cases():
    cases = []

    def add(name, value):
        cases.append((name, value))

    add("shots_and_tags", scene(SHOTS, audio_paths=["a.wav"]))
    add("subject_without_alias_keeps_its_number", scene(SHOTS, audio_paths=["a.wav"], reference_aliases={"<Picture 1>": "@Image 1", "<Picture 2>": "@Image 2"}))
    add("picture_without_alias_is_an_error", scene(SHOTS.replace("<Picture 2>", "<Picture 3>"), audio_paths=["a.wav"]))
    add("aliases_default_from_image_references", scene(SHOTS, audio_paths=["a.wav"], reference_aliases={}))
    add("alias_value_is_text", scene("[Shot 1] <Picture 1> stands.", reference_aliases={"<Picture 1>": "@Image 1 and @Image 2"}))
    add("bare_labels_and_case", scene("[Shot 1] picture 1 meets IMAGE 2 and image 1 again; @Image 2 stays; <Picture 02> too.", reference_aliases={}))
    add("leading_zero_number", scene("[Shot 1] <Picture 001> and <Image 0002>.", reference_aliases={}))
    add("unknown_video_label_is_an_error", scene("[Shot 1] See <Video 1>.", reference_aliases={}))
    add("video_and_audio_labels", scene("[Shot 1] See <Video 1> and <Audio 2>.", reference_aliases={}, video_paths=["a.mp4"], audio_paths=["a.wav", "b.wav"]))
    add("original_soundtrack_alias", scene("[Shot 1] She sings to <Audio 1>.", original_soundtrack=True, reference_aliases={}))
    add("original_soundtrack_with_audio_paths", scene("[Shot 1] She sings to <Audio 1>.", original_soundtrack=True, audio_paths=["a.wav"], reference_aliases={}))
    add("delivery_tag_with_directions_and_emphasis", scene("[Shot 1] <d>[English, tender, singing] <i>Hold</i> me <i>close</i></d> now.", reference_aliases={}))
    add("delivery_tag_without_directions", scene("[Shot 1] <d>just words</d> and <D>  more words </D>.", reference_aliases={}))
    add("delivery_tag_multiline_and_spacing", scene("[Shot 1] <d>[ English ]\n  line one\nline two</d>", reference_aliases={}))
    add("stray_tags", scene("[Shot 1] <i>lone</i> words </d> and <d> never closed", reference_aliases={}))
    add("headings_set_the_hint", scene("summary:\nA roof at dusk.\n\nretention_analysis:\nKeep the jacket.\n\ndetailed_description:\n[Shot 1] She sings.\n\noverall_soundscape:\nWind.\n\nnon_diegetic_music:\nSoft synth.\n\nsubject_definitions:\nAva.", reference_aliases={}))
    add("known_destination_headings", scene("References:\nAva.\nShots:\n[Shot 1] She sings.\nFinal assembly:\nKeep the song.", reference_aliases={}))
    add("unknown_heading_stays_content", scene("Notes for later:\nSome text.\n\nsummary:\nBody.", reference_aliases={}))
    add("blank_lines_split_blocks", scene("One.\n\n\nTwo.\nStill two.\n\n  \nThree.", reference_aliases={}))
    add("odd_line_ends", scene("One.\r\nTwo.\rThree. Four.\x0bFive.\x0cSix.\x85Seven.", reference_aliases={}))
    add("hard_cut_forms_are_repaired", scene("[Shot 1] Hard cut, the same moment on the same stage. [Shot 2] Hard cut,  SAME MOMENT. [Shot 3] Hard cut, the same moment, again.", reference_aliases={}))
    add("hard_cut_inside_a_lyric_is_left_alone", scene('[Shot 1] She sings "Hard cut, the same moment" and “Hard cut, same moment” aloud. Hard cut, same moment.', reference_aliases={}))
    add("spinning_in_a_vocal_close_up", scene("[Shot 1] Close-up of Ava as she sings, spinning once. [Shot 2] A wide shot, spinning a full turn.", reference_aliases={}))
    add("spinning_in_a_close_up_without_vocals", scene("[Shot 1] Close up of Ava, spinning once.", reference_aliases={}))
    add("spinning_inside_the_lyric", scene('[Shot 1] Close-up of Ava who sings "spinning once" and spinning a full turn.', reference_aliases={}))
    add("shot_text_in_a_scene_block", scene("Plain intro.\n\n[Shot 1] Close-up of Ava singing, spinning once.", reference_aliases={}))
    add("escaped_quote_in_a_lyric", scene('[Shot 1] Close-up of Ava who sings "she said \\"hard cut, same moment\\"" and spins once, spinning once.', reference_aliases={}))
    add("unclosed_quote", scene('[Shot 1] Close-up of Ava who sings "never closed, spinning once.', reference_aliases={}))
    add("stock_phrases", scene("[Shot 1] fully_preserved look, fully_copy audio [reference generation + audio reuse]. preserving every person's exact position, posture, and movement across every camera cut.", reference_aliases={}))
    add("scene_audio_inserts_the_definition", scene(SCENE_AUDIO_PROMPT, use_scene_audio=True, original_soundtrack=True, reference_aliases=ALIASES))
    add("scene_audio_with_audio_paths_is_an_error", scene(SCENE_AUDIO_PROMPT, use_scene_audio=True, audio_paths=["a.wav"], reference_aliases=ALIASES))
    add("scene_audio_without_vocals", scene("[Shot 1] Ava walks across the roof.\n\nBuilder note: Original soundtrack.", use_scene_audio=True, reference_aliases=ALIASES))
    definition = "@Audio 1 is the supplied project/custom audio excerpt for this scene, beginning at scene time 0. Follow its original words, vocal phrasing, pauses and tempo. The full original soundtrack is retained by the Builder during final assembly."
    add("scene_audio_reconversion_keeps_one_definition", scene("References:\n" + definition + "\n\nShots:\n[Shot 1] <Subject 1> sings in sync with <Audio 1>.", use_scene_audio=True, original_soundtrack=True, reference_aliases=ALIASES))
    guidance = importlib.import_module("vrgdg_pack.llm.seedance_conversion").SEEDANCE_VOCAL_ROLE_GUIDANCE
    add("scene_audio_reconversion_keeps_one_guidance", scene("[Shot 1] <Subject 1> sings in sync with <Audio 1>.\n\nSound:\n" + guidance, use_scene_audio=True, original_soundtrack=True, reference_aliases=ALIASES))
    add("scene_audio_lyric_in_quotes_only", scene('[Shot 1] Ava stands. She says "sings" and "lip-sync" aloud.', use_scene_audio=True, reference_aliases=ALIASES))
    add("image_beyond_the_references", scene("[Shot 1] @Image 3 sings.", reference_aliases={}))
    add("image_number_zero", scene("[Shot 1] @Image 0 sings.", reference_aliases={}))
    add("video_without_a_reference", scene("[Shot 1] @Video 1 plays.", reference_aliases={}))
    add("video_reference_present", scene("[Shot 1] @Video 1 plays.", reference_aliases={}, video_paths=["a.mp4"]))
    add("audio_reference_in_scene_audio_mode", scene("[Shot 1] @Audio 1 and @Audio 2 play.", use_scene_audio=True, reference_aliases={}))
    add("audio_reference_present", scene("[Shot 1] @Audio 2 plays.", reference_aliases={}, audio_paths=["a.wav", "b.wav"]))
    add("empty_prompt", scene(""))
    add("blank_prompt", scene("   \n  "))
    add("only_headings", scene("summary:\nshots:\n", reference_aliases={}))
    add("number_in_a_longer_word", scene("[Shot 1] Image 1x is fine, and Subject 3 too, and reimage 1.", reference_aliases={}))
    add("source_prompt_is_a_number", scene(12345))
    return cases


def assemble_cases(blocks):
    ids = [block["id"] for block in blocks]

    def plan(*sections):
        return {"sections": [{"heading": heading, "block_ids": list(items)} for heading, items in sections]}

    big = [{"id": "b1", "heading_hint": "Scene", "text": "x" * 9000}, {"id": "b2", "heading_hint": "Shots", "text": "y" * 6000}]
    return [
        ("one_section", blocks, plan(("Shots", ids))),
        ("two_sections", blocks, plan(("References", ids[:1]), ("Shots", ids[1:]))),
        ("every_block_its_own_section", blocks, plan(*[("Shots", [i]) for i in ids])),
        ("reordered", blocks, plan(("Shots", ids[::-1]))),
        ("dropped_block", blocks, plan(("Shots", ids[:-1]))),
        ("duplicated_block", blocks, plan(("Shots", ids + ids[:1]))),
        ("unknown_block", blocks, plan(("Shots", ids + ["b99"]))),
        ("unknown_heading", blocks, plan(("Intro", ids))),
        ("lower_case_heading", blocks, plan(("shots", ids))),
        ("empty_block_ids", blocks, {"sections": [{"heading": "Shots", "block_ids": []}]}),
        ("block_ids_not_a_list", blocks, {"sections": [{"heading": "Shots", "block_ids": "b1"}]}),
        ("section_not_an_object", blocks, {"sections": ["Shots"]}),
        ("no_sections", blocks, {"sections": []}),
        ("sections_not_a_list", blocks, {"sections": {}}),
        ("plan_without_sections", blocks, {}),
        ("plan_not_an_object", blocks, ["sections"]),
        ("numeric_block_ids", blocks, {"sections": [{"heading": "Shots", "block_ids": [1, 2, 3]}]}),
        ("exactly_the_limit", [{"id": "b1", "heading_hint": "Scene", "text": "z" * (15000 - len("Shots:\n"))}], plan(("Shots", ["b1"]))),
        ("over_the_limit", big, plan(("Scene", ["b1"]), ("Shots", ["b2"]))),
        ("blocks_keep_their_text", [{"id": "b1", "heading_hint": "Scene", "text": "line one\nline two ünï"}, {"id": "b2", "heading_hint": "Scene", "text": ""}], plan(("Scene", ["b1", "b2"]))),
    ]


def scene_audio_cases():
    one = ["{dir}/excerpt.wav"]
    image = {"path": "{dir}/ava.png"}
    return [
        ("not_scene_audio", {"audio_mode": "uploaded", "reference_audios": one}),
        ("no_audio_mode", {}),
        ("without_an_excerpt", {"audio_mode": "scene_audio", "reference_audios": []}),
        ("two_excerpts", {"audio_mode": "scene_audio", "reference_audios": one + one}),
        ("singing_by_assignment", {"audio_mode": "scene_audio", "reference_audios": one, "reference_images": [image]}),
        ("singer_image_one", {"audio_mode": "scene_audio", "reference_audios": one, "reference_images": [image, image], "lip_sync_image": 1}),
        ("singer_image_as_text", {"audio_mode": "scene_audio", "reference_audios": one, "reference_images": [image, image], "lip_sync_image": "2"}),
        ("singer_image_beyond_the_images", {"audio_mode": "scene_audio", "reference_audios": one, "reference_images": [image], "lip_sync_image": 2}),
        ("singer_image_negative", {"audio_mode": "scene_audio", "reference_audios": one, "reference_images": [image], "lip_sync_image": -1}),
        ("singer_image_zero_means_assignment", {"audio_mode": "scene_audio", "reference_audios": one, "reference_images": [image], "lip_sync_image": 0}),
        ("no_lip_sync", {"audio_mode": "scene_audio", "reference_audios": one, "reference_images": [image], "lip_sync_image": 1, "no_lip_sync": True}),
        ("no_lip_sync_without_a_singer", {"audio_mode": "scene_audio", "reference_audios": one, "no_lip_sync": 1}),
    ]


def graph_cases(api):
    """The ComfyUI graph for Seedance. Files are made fresh for each case (their names are in ``files``), and the random names of the
    staged references and of the output folder are replaced by ``<ID>`` so the Rust port can be compared with this record."""
    import re
    import shutil
    import sys
    import tempfile

    everything = ["ByteDance2ReferenceNodeV2", "SaveVideo", "LoadImage", "LoadVideo", "LoadAudio"]
    png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
    files = ["ava.png", "ben.jpg", "roof.JPEG", "clip.mp4", "clip.mov", "excerpt.wav", "song.mp3", "notes.txt", "picture.gif"]

    def case(name, payload, nodes=everything, **extra):
        return {"name": name, "payload": payload, "nodes": nodes, "files": files, **extra}

    img = lambda name: {"path": "{dir}/" + name}  # noqa: E731
    base = {"prompt": "Ava walks across the roof.", "reference_images": [img("ava.png")]}
    cases = [
        case("one_image", base),
        case("defaults_are_filled", {"prompt": "  Ava walks.  ", "reference_images": ["{dir}/ava.png"]}),
        case("every_kind", {**base, "reference_images": [img("ava.png"), img("ben.jpg"), img("roof.JPEG")], "reference_videos": ["{dir}/clip.mp4", {"path": "{dir}/clip.mov"}], "reference_audios": ["{dir}/excerpt.wav", "{dir}/song.mp3"], "duration": 12}),
        case("settings", {**base, "duration": 6.2, "resolution": "1080p", "ratio": "9:16", "generate_audio": False, "seed": 5}),
        case("seed_wraps", {**base, "seed": 2147483648 + 41}),
        case("negative_seed", {**base, "seed": -3}),
        case("duration_rounds_up", {**base, "duration": 4.01}),
        case("short_duration_is_four", {**base, "duration": 1.5}),
        case("the_longest_scene", {**base, "duration": 30}),
        case("image_from_a_data_url", {"prompt": "x", "reference_images": [{"data": "data:image/png;base64," + png}]}),
        case("jpeg_data_url", {"prompt": "x", "reference_images": [{"data": "data:image/jpeg;base64," + png}]}),
        case("webp_data_url", {"prompt": "x", "reference_images": [{"data": "data:image/webp;base64," + png}]}),
        case("data_without_a_header", {"prompt": "x", "reference_images": [{"data": png}]}),
        case("path_wins_over_data", {"prompt": "x", "reference_images": [{"path": "{dir}/ava.png", "data": "data:image/png;base64," + png}]}),
        case("quoted_path", {"prompt": "x", "reference_images": [{"path": '"{dir}/ava.png"'}]}),
        case("scene_audio_mode", {**base, "audio_mode": "scene_audio", "reference_audios": ["{dir}/excerpt.wav"], "lip_sync_image": 1}),
        case("scene_audio_without_a_singer", {**base, "audio_mode": "scene_audio", "reference_audios": ["{dir}/excerpt.wav"], "no_lip_sync": True}),
        case("scene_audio_needs_one_excerpt", {**base, "audio_mode": "scene_audio"}),
        case("scene_audio_singer_not_attached", {**base, "audio_mode": "scene_audio", "reference_audios": ["{dir}/excerpt.wav"], "lip_sync_image": 4}),
        case("empty_prompt", {"prompt": "   ", "reference_images": [img("ava.png")]}),
        case("prompt_at_the_limit", {"prompt": "p" * 15000, "reference_images": [img("ava.png")]}),
        case("prompt_over_the_limit", {"prompt": "p" * 15001, "reference_images": [img("ava.png")]}),
        case("scene_audio_text_over_the_limit", {"prompt": "p" * 14900, "reference_images": [img("ava.png")], "audio_mode": "scene_audio", "reference_audios": ["{dir}/excerpt.wav"]}),
        case("duration_zero", {**base, "duration": 0}),
        case("duration_too_long", {**base, "duration": 30.5}),
        case("duration_infinite_as_text", {**base, "duration": "inf"}),
        case("duration_as_text", {**base, "duration": "6.5"}),
        case("unknown_resolution", {**base, "resolution": "4k"}),
        case("unknown_ratio", {**base, "ratio": "5:4"}),
        case("adaptive_ratio", {**base, "ratio": "adaptive"}),
        case("no_references", {"prompt": "x"}),
        case("empty_reference_lists", {"prompt": "x", "reference_images": [], "reference_videos": [], "reference_audios": []}),
        case("too_many_images", {"prompt": "x", "reference_images": [img("ava.png")] * 31}),
        case("thirty_images", {"prompt": "x", "reference_images": [img("ava.png")] * 30}),
        case("too_many_videos", {"prompt": "x", "reference_videos": ["{dir}/clip.mp4"] * 11}),
        case("too_many_audios", {"prompt": "x", "reference_audios": ["{dir}/song.mp3"] * 11}),
        case("references_not_a_list", {"prompt": "x", "reference_images": "{dir}/ava.png"}),
        case("missing_image_file", {"prompt": "x", "reference_images": [img("nope.png")]}),
        case("empty_image_path", {"prompt": "x", "reference_images": [{"path": ""}]}),
        case("image_format_not_allowed", {"prompt": "x", "reference_images": [img("picture.gif")]}),
        case("video_format_not_allowed", {"prompt": "x", "reference_videos": ["{dir}/notes.txt"]}),
        case("audio_format_not_allowed", {"prompt": "x", "reference_audios": ["{dir}/clip.mov"]}),
        case("mp4_is_allowed_as_audio", {"prompt": "x", "reference_audios": ["{dir}/clip.mp4"]}),
        case("data_url_for_a_video_is_not_a_file", {"prompt": "x", "reference_videos": [{"data": "data:video/mp4;base64," + png}]}),
        case("partner_node_missing", base, nodes=["LoadImage"]),
        case("save_video_missing", base, nodes=["ByteDance2ReferenceNodeV2", "LoadImage"]),
        case("loader_missing", {**base, "reference_audios": ["{dir}/song.mp3"]}, nodes=["ByteDance2ReferenceNodeV2", "SaveVideo", "LoadImage"]),
    ]

    nodes_module = sys.modules["nodes"]
    hex_id = re.compile(r"[0-9a-f]{32}")
    results = []
    for entry in cases:
        folder = tempfile.mkdtemp(prefix="h3seedance")
        input_dir = Path(folder) / "comfy_input"
        input_dir.mkdir()
        for name in entry["files"]:
            (Path(folder) / name).write_bytes(b"x")
        base_folder = folder.replace("\\", "/")

        def fill(value):
            if isinstance(value, str):
                return value.replace("{dir}", base_folder)
            if isinstance(value, list):
                return [fill(item) for item in value]
            if isinstance(value, dict):
                return {key: fill(item) for key, item in value.items()}
            return value

        def clean(value):
            if isinstance(value, str):
                return hex_id.sub("<ID>", value.replace(base_folder, "{dir}").replace(folder, "{dir}"))
            if isinstance(value, list):
                return [clean(item) for item in value]
            if isinstance(value, dict):
                return {key: clean(item) for key, item in value.items()}
            return value

        nodes_module.NODE_CLASS_MAPPINGS = {name: object for name in entry["nodes"]}
        api.folder_paths.get_input_directory = lambda input_dir=input_dir: str(input_dir)
        api.folder_paths.get_output_directory = lambda: "<OUT>"
        record = {"name": entry["name"], "payload": entry["payload"], "nodes": entry["nodes"], "files": entry["files"]}
        try:
            record["result"] = clean(api._build_seedance25_reference_prompt(fill(entry["payload"])))
            record["staged"] = sorted(clean(path.name) for path in input_dir.iterdir())
        except ValueError as exc:
            record["error"] = clean(str(exc))
        shutil.rmtree(folder, ignore_errors=True)
        results.append(record)
    return results


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    args = parser.parse_args()
    out = Path(__file__).resolve().parents[1] / "fixtures" / "prompt"
    pack_env.install(args.pack)
    pack_env.install_server_stub()
    module = importlib.import_module("vrgdg_pack.llm.seedance_conversion")

    prepare, blocks_for_plans = [], None
    for name, value in prepare_cases():
        entry = {"name": name, "scene": value}
        try:
            entry["blocks"] = module.prepare_seedance_blocks(value)
        except ValueError as exc:
            entry["error"] = str(exc)
        prepare.append(entry)
        if name == "shots_and_tags":
            blocks_for_plans = entry["blocks"]
    blocks_for_plans = blocks_for_plans + [{"id": "b9", "heading_hint": "Sound", "text": "Wind."}]
    blocks_for_plans[-1]["id"] = f"b{len(blocks_for_plans)}"

    assemble = []
    for name, blocks, plan in assemble_cases(blocks_for_plans):
        entry = {"name": name, "blocks": blocks, "plan": plan}
        try:
            entry["prompt"] = module.assemble_seedance_prompt(blocks, plan)
        except ValueError as exc:
            entry["error"] = str(exc)
        assemble.append(entry)

    api = importlib.import_module("vrgdg_pack.runner.api_video_workflows")
    audio_prompt = []
    for name, payload in scene_audio_cases():
        entry = {"name": name, "payload": payload}
        try:
            entry["prompt"] = api._seedance_scene_audio_prompt("Prompt text.", payload)
        except ValueError as exc:
            entry["error"] = str(exc)
        audio_prompt.append(entry)

    graph = graph_cases(api)

    document = {
        "headings": list(module.HEADINGS),
        "plan_schema": module.PLAN_SCHEMA,
        "vocal_role_guidance": module.SEEDANCE_VOCAL_ROLE_GUIDANCE,
        "prepare": prepare,
        "assemble": assemble,
        "audio_prompt": audio_prompt,
    }
    out.mkdir(parents=True, exist_ok=True)
    (out / "seedance_graph.json").write_text(json.dumps({"cases": graph}, indent=1, ensure_ascii=False), encoding="utf-8", newline="\n")
    (out / "seedance.json").write_text(json.dumps(document, indent=1, ensure_ascii=False), encoding="utf-8", newline="\n")
    print(f"seedance_graph.json: {len(graph)} graph cases, {sum(1 for g in graph if 'error' in g)} errors; {len(audio_prompt)} scene audio prompt cases")
    for entry in prepare:
        if "error" in entry:
            print("   prepare error:", entry["name"], "->", entry["error"][:100])
    errors = sum(1 for e in prepare + assemble if "error" in e)
    print(f"seedance.json: {len(prepare)} prepare cases, {len(assemble)} assemble cases, {errors} errors")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
