# H3 Studio API endpoints

Every endpoint of the agent API this app serves, under `/api/v1`. The MCP tools call these. `{pid}` is the project id, `{sid}` a scene id.
A call that runs as a job returns a job id: poll `GET /jobs/{id}` (the `job_wait` tool does).

## assets

- `GET /projects/{pid}/assets` Files in the project folder: images, videos, thumbnails, audio and final videos.

## audio

- `PUT /projects/{pid}/audio` Attach the song: a file path (`audio_path`) or uploaded data (`audio_data`, `audio_name`). Saves the waveform peaks, beats and tempo. (body: audio_data, audio_name, audio_path)
- `GET /projects/{pid}/audio/beats` Beat markers and detected tempo.
- `PUT /projects/{pid}/audio/beats` Replace the beat markers (`beats`) and tempo (`tempo_bpm`). (body: beats, tempo_bpm)
- `POST /projects/{pid}/audio/silent` Create a silent audio track of a given `duration` instead of a song. (body: duration, scope)
- `POST /projects/{pid}/audio/stems` (job) Split the project's song into stems with Demucs (the Audio Mask's splitter, on the whole song) and write a vocal-free mix. Optional: `audio_path` (default the project song), `model_name` (htdemucs by default, htdemucs_ft, mdx_extra or htdemucs_6s), `device` (auto, cuda or cpu) and `input_gain_db` (-24 to 24, raise it when the vocals are quiet). The job result has `vocals_path`, `no_vocals_path`, `stems` (a wav for each stem), `model_name` and `duration_seconds`. The files are in project_audio/stems. To render a scene without the singer's voice, send `audio_path: <no_vocals_path>` in the render params, so a face that is not the singer has nothing to lip-sync to. (body: audio_path, device, input_gain_db, model_name)
- `GET /projects/{pid}/audio/waveform` Waveform peaks for drawing the audio. `peaks` sets how many. (query: peaks)

## duplicate

- `POST /projects/{pid}/duplicate` Copy a project to `new_name`. `options` chooses what to keep (scenes, mappings, notes, prompts, media). (body: new_name, options)

## effects

- `GET /effects` The effects the timeline's effect lane can hold: `effects` (each with `id`, `kind`, `category` (how the picker groups it), `origin` (`original`: the app's own), `author`, `license`, `description`, `tags` and `params`: name, type, default and range of each setting), and `count`. Use an `id` as `look` in the project's effects or in a stitch. An effect adds or bends something (a glitch, a blur, a flash). `progress` runs 0 to 1 over its block and `time` is the seconds since it started.
- `GET /projects/{pid}/effects` The project's effects in timeline order, as the stitch reads them: `video_effects`, each `{id, look, start, end, seconds, intensity, params}` (a block of the timeline like an overlay: `start` and `end` are seconds on the timeline, `seconds` its length, `intensity` 0 to 1 how much of its change is mixed in, `params` the settings changed from their defaults), `count`, and `revision`. Blocks may overlap: a frame gets every filter that covers it, then every effect. Every stitch draws them, and so do the Video Builder's Render All and Export, before the overlays are laid over the video.
- `PUT /projects/{pid}/effects` Replace all the project's effects with `video_effects`, a list of `{look, start, seconds, intensity, params, id}`. `look` is an id from GET /effects, `start` the time on the timeline in seconds, `seconds` the length 0.05 or more (default 1), `intensity` 0 to 1 (default 1), `params` the settings to change from their defaults, `id` optional. The reply is the saved list. (body: video_effects)
- `POST /projects/{pid}/effects/set` Add one effect, change one, or remove one, and leave the others alone. Without an `id` a new block is added at `start`. With the `id` of an existing block only the fields sent change. A `look` that is null, empty or `none` removes the block with that `id`. `seconds`, `intensity` and `params` are as in PUT /projects/{pid}/effects. The reply is the whole list. (body: id, look, start, seconds, intensity, params)

## elevenlabs

- `GET /projects/{pid}/elevenlabs` Speaking mode only: read whether an ElevenLabs API key is saved to this project, plus revision. Never returns the key. The project's own key is used; without one, the key saved in H3 Studio's App Settings.
- `PUT /projects/{pid}/elevenlabs` Speaking mode only: explicitly save `api_key` to `elevenlabs_api_key_project`, like the Builder's LLM runner key. An empty string clears it. Stored in the project session; existing export credential handling applies. Supports If-Match. Never echoes the key. The project's own key is used; without one, the key saved in H3 Studio's App Settings. (body: api_key)
- `POST /projects/{pid}/elevenlabs/test` Speaking mode only: test the saved API key's Voices read access. Returns connected: true on success. Does not save a key or generate speech. The project's own key is used; without one, the key saved in H3 Studio's App Settings.
- `POST /projects/{pid}/elevenlabs/voice-design/create` Speaking mode only: explicitly create an account voice from a chosen preview. Accepts voice_name (1–256), voice_description (20–1000) and generated_voice_id. Returns permanent voice_id and name. Does not assign it or mutate the project. Assign with PUT references/subjects/{rid} elevenlabs_voice and If-Match. Never retries account creation automatically; after an ambiguous failure refresh voices before retrying. The project's own key is used; without one, the key saved in H3 Studio's App Settings. (body: generated_voice_id, voice_description, voice_name)
- `POST /projects/{pid}/elevenlabs/voice-design/previews` Speaking mode only: generate ElevenLabs Voice Design previews using the saved project key. Accepts voice_description (20–1000 characters), model_id (eleven_ttv_v3 default or eleven_multilingual_ttv_v2), and optional text (100–1000 characters; blank generates sample dialogue). Returns temporary generated_voice_id and base64 MP3 candidates. Uses account credits; does not save a voice or project state. The project's own key is used; without one, the key saved in H3 Studio's App Settings. (body: model_id, text, voice_description)
- `GET /projects/{pid}/elevenlabs/voices` Speaking mode only: list one page of account voices using the saved project key. Returns voice_id, name, category, public HTTPS preview_url, has_more and next_page_token. Pass next_page_token for another page. Does not generate speech. The project's own key is used; without one, the key saved in H3 Studio's App Settings. (query: next_page_token)

## export

- `POST /projects/{pid}/export` Build a zip of the project for backup or sharing.

## filters

- `GET /filters` The filters the timeline's filter lane can hold: `filters` (each with `id`, `kind`, `category` (how the picker groups it), `origin` (`original`: the app's own), `author`, `license`, `description`, `tags` and `params`: name, type, default and range of each setting), and `count`. Use an `id` as `look` in the project's filters or in a stitch. A filter grades the colours: its settings are exposure, contrast, saturation, temperature, tint, highlights, shadows, fade, gamma, a split tone, a mono mix, a vignette, grain and sharpen. `Adjust` has every setting at its neutral value.
- `GET /projects/{pid}/filters` The project's filters in timeline order, as the stitch reads them: `video_filters`, each `{id, look, start, end, seconds, intensity, params}` (a block of the timeline like an overlay: `start` and `end` are seconds on the timeline, `seconds` its length, `intensity` 0 to 1 how much of its change is mixed in, `params` the settings changed from their defaults), `count`, and `revision`. Blocks may overlap: a frame gets every filter that covers it, then every effect. Every stitch draws them, and so do the Video Builder's Render All and Export, before the overlays are laid over the video.
- `PUT /projects/{pid}/filters` Replace all the project's filters with `video_filters`, a list of `{look, start, seconds, intensity, params, id}`. `look` is an id from GET /filters, `start` the time on the timeline in seconds, `seconds` the length 0.05 or more (default 1), `intensity` 0 to 1 (default 1), `params` the settings to change from their defaults, `id` optional. The reply is the saved list. (body: video_filters)
- `POST /projects/{pid}/filters/set` Add one filter, change one, or remove one, and leave the others alone. Without an `id` a new block is added at `start`. With the `id` of an existing block only the fields sent change. A `look` that is null, empty or `none` removes the block with that `id`. `seconds`, `intensity` and `params` are as in PUT /projects/{pid}/filters. The reply is the whole list. (body: id, look, start, seconds, intensity, params)

## finals

- `GET /projects/{project_id}/finals` List the project's final videos.

## health

- `GET /health` Health check: `healthy` or `degraded`, whether ComfyUI is up, its queue depth, whether FFmpeg is found and whether a GPU is available.

## images

- `POST /projects/{project_id}/images/generate` (job) Generate images for many scenes.

## instructions

- `GET /instructions` List the prompt-writing instructions (system prompts) the LLM steps use, with their override state.
- `PUT /instructions/presets/{name}` Save the current text of an instruction as a named preset. (body: key, name)
- `POST /instructions/presets/{name}/load` Load a named preset into an instruction. (body: key, name)
- `GET /instructions/{key}` One instruction's current text, its default and whether a project override is set.
- `PUT /instructions/{key}` Save an instruction override for a project or for all projects. (body: key, project_folder, project_id)
- `DELETE /instructions/{key}/override` Remove an override and return to the built-in instruction. (body: key, project_folder, project_id)

## jobs

- `GET /jobs` List jobs, newest first. Filter with `project_id`, `status` and `type`. (query: project_id, status, type)
- `GET /jobs/{id}` One job: status (`queued`, `running`, `succeeded`, `failed`, `cancelled`, `interrupted`), progress, result and error.
- `POST /jobs/{id}/cancel` Cancel a queued or running job. A running ComfyUI render is interrupted.
- `GET /jobs/{id}/log` The job's progress log. Pass `since` (a sequence number) to read only new lines. (query: since)
- `POST /jobs/{id}/retry` (job) Start a failed or interrupted job again. `resume: true` skips work that already finished. (body: resume)

## latents

- `GET /projects/{project_id}/latents` Latent chain status for every scene (MiniMax continuity).
- `GET /projects/{project_id}/latents/dirty` Scenes whose latent is out of date because an earlier scene changed.
- `POST /projects/{project_id}/latents/rebuild` (job) Rebuild out-of-date latents in order. (body: project_id)

## llm

- `GET /llm/active` Which LLM the API would use right now, for a project (`project_id`) or, with none, for a new project (your saved model defaults). For LM Studio this is the model that is loaded. The API never loads or switches a model. (query: project_id)
- `GET /llm/models` Models the configured LLM provider offers. `provider` selects LM Studio, an own server or an API provider. (query: provider)
- `POST /llm/test` Send a short test request to the configured LLM runner and report whether it answers. (body: provider, text_runner)

## lyrics

- `GET /projects/{pid}/lyrics` The project's lyrics text and the lyric text on each scene.
- `PUT /projects/{pid}/lyrics` Set the lyrics (`lyrics_text`) or an SRT (`srt_text`). The text is the reference Line Mapping works from. (body: lyrics_text, srt_text)
- `POST /projects/{pid}/lyrics/align` (job) Time the lyrics against the song with the Stable-ts workflow in ComfyUI and save the timed lines.

## meta

- `GET /meta` API version (`v1`), the app's name and version, and the capabilities this app serves.

## minimax

- `POST /projects/{project_id}/minimax/cleanup` Delete MiniMax scratch outputs that are no longer needed.
- `GET /projects/{project_id}/minimax/index` Index of the MiniMax scratch outputs per scene.

## minimax-prompts

- `POST /projects/{pid}/minimax-prompts` (job) Write MiniMax H3 reference-to-video prompts with the project's LLM for scenes that have none (`replace_existing`, `scene_ids`, `limit`). Each singing scene's prompt has its lyric in double quotes after 'sings the lyric line,'. Saves them on the scenes and in the Storyboard Builder's copy (`storyboard/storyboard.json` and the `prompts/` files). Needs a mapped character with an image on each scene. Each prompt is the Builder's full format: subject definitions tying `<Subject N>` to `<Picture N>`, summary, retention analysis, the shots, and the soundscape.

## models

- `GET /models` Models, checkpoints and LoRAs installed in ComfyUI, grouped by type.

## modes

- `GET /modes` Supported image and video modes with their requirements and capabilities.

## overlay-layers

- `GET /projects/{pid}/overlay-layers` The project's overlays as the render sees it: `overlay_layer_track` (`enabled`), `overlay_layers` (every setting filled in and clamped), `count`, `revision`, and `warnings` for layers that will not draw as asked (a missing media file, a LUT or font that is not installed, an unknown kind, empty text). Overlays are drawn onto the final video by the stitch (`stitch_final`, `pipeline_build_full_video`) and by the Video Builder's Render All and Export. The timeline is in seconds from the start of the song.
- `PUT /projects/{pid}/overlay-layers` Replace the project's overlays (`overlay_layers`, the whole list) and/or the switch for all of them (`overlay_layer_track`, `{"enabled": false}` turns every overlay off for every render without deleting the layers). Read first (GET), change what you need, send the list back. Later layers draw on top. Every layer has `kind` (media, grain, vignette, text, lut), `id`, `start` and `end` in seconds, `enabled`, `locked`, `opacity` (0 to 1), `fade_in` and `fade_out` (seconds). media: `path` (a file in the project, `overlay/<name>`, from POST /projects/{pid}/overlay-layers/media), `blend_mode` (screen drops black and suits dust, smoke and flares, then add, lighten, darken, overlay, softlight, hardlight, colordodge, colorburn, multiply, normal), `scale` (cover, contain, stretch), `loop`, `speed` (0.5 to 2), `flip_h`, `brightness` (-1 to 1), `contrast` (0 to 2), `tint` (#rrggbb). grain: `strength` (0 to 60, 20 to 30 is film), `chroma` (0 to 20), `size` (fine, medium, coarse), `animated`. vignette: `strength` (0 to 1, 0.7 is the default). text: `text`, `font`, `size` (pixels on a 1080 tall picture), `color`, `outline_width`, `outline_color`, `shadow`, `x` and `y` (0 to 1, the centre of the text), `animation` (none or slide_up). lut: `lut` (an installed .cube name), `strength` (0 to 1). Values out of range are clamped and the reply shows what was saved. A clip sent without `locked` is unlocked. Send If-Match with the revision you read. (body: overlay_layers, overlay_layer_track)
- `POST /projects/{pid}/overlay-layers/add` Add `layers` (a list of overlay layers, fields as in PUT /projects/{pid}/overlay-layers) to the end of the list without touching the others. The reply is the whole list with `added`, the ids the new layers got. (body: layers)
- `POST /projects/{pid}/overlay-layers/lyric_captions` Add one text clip for each lyric line, timed to the words of the song when the project has word timing (`project_context/timestamped_lyrics.json`, from the lyrics alignment) and otherwise one caption over each scene that has lyrics. `style` is clean (default), bold or lower (a lower third). `font` is a name from the library's `fonts` list and replaces the font of the style. `from` and `to` (seconds) limit it to a range. Captions from an earlier run are replaced (only those inside the range when one is given) unless `replace` is false. The reply is the whole list with `source` (timestamped_lyrics or scenes), `style` and `captions`. (body: style, font, from, to, replace)
- `POST /projects/{pid}/overlay-layers/media` Add a clip of the overlay library (`library_id`, from GET /overlay-library) as an overlay: it is copied into the project's `overlay` folder and starts at `start` (default 0) and runs to `end` (default the end of the timeline, looping its file). The blend mode is the library's suggestion (screen for clips on black). Any other media clip field (opacity, blend_mode, speed, fades) can be sent too. The reply is the whole list with `added` and the project `path`. (body: library_id, start, end)

## overlay-library

- `GET /overlay-library` The overlay library: `items` (clips to overlay, each with `id`, `category`, `duration`, size and `suggested_blend`), `fonts` (names a text clip's `font` can use: the bundled fonts, font files in the fonts folder and the fonts installed on the computer), `luts` (installed .cube files for a LUT clip), `caption_styles` (clean, bold, lower) and `kinds` (media, grain, vignette, text, lut). The person fills the library in the app (Post Process, Overlays). Use an item's `id` with POST /projects/{pid}/overlay-layers/media.

## pipelines

- `POST /pipelines/from-song` (job) Song to final video in one job: create the project if needed, attach audio and lyrics, make the scenes, then run the full build. (body: project_id, project_name)
- `POST /projects/{project_id}/pipelines/build-full-video` (job) Run the whole build for an existing project (prompts, images, videos, stitch) as one job. (body: project_id)
- `GET /projects/{project_id}/pipelines/plan` Dry-run plan for the full build: steps, estimated GPU minutes and missing prerequisites.

## post

- `GET /post/luts` List the available LUT (color look-up table) files.
- `POST /post/luts/upload` Upload a LUT file (base64 `content` and `filename`). (body: content, data, filename; query: filename)
- `POST /projects/{project_id}/post/apply-all` (job) Apply the project's post-processing settings to every scene video. (body: project_id)

## prompts

- `POST /projects/{pid}/prompts/batch` (job) Write image or video prompts for many scenes (`kind`, `scope`, `run_mode`, `scene_ids`). Image prompts use each scene's notes, lyric and references.
- `POST /projects/{pid}/prompts/concepts` (job) Write scene concept prompts for the whole project with the LLM.
- `POST /projects/{pid}/prompts/motion-notes` (job) Write motion and camera notes for scenes with the LLM.

## queue

- `GET /queue` Summary of the API job queue: running, queued and finished jobs.

## references

- `GET /projects/{pid}/references` Characters, locations and the scene mappings.
- `POST /projects/{pid}/references/assign-scenes` Assign characters and locations to scenes by pattern (`random`, `rotate`, `blocks`, `unchanged`). `dry_run` previews.
- `POST /projects/{pid}/references/locations/extract` (job) Ask the project's LLM for filming locations from the lyrics and `style_theme` (LM Extract) and add them.
- `PUT /projects/{pid}/references/locations/{rid}` Create or update a location (name, description, trigger phrase, optional `image`). Fields that are not sent keep their saved value. `source: "refmod"` with `refmod: {"name", "strength"}` binds a saved RefMod from GET /refmods, as on a character.
- `GET /projects/{pid}/references/scene-mapping` Read which characters, locations, ingredients and extras each scene uses. A scene maps to one location here. To see every reference a MiniMax scene can pick from, and the order it sends, use `GET /projects/{pid}/scenes/{sid}/minimax-references`. Add `?include=scenes` (every scene) or `?scene_id=<id or number>` (one scene) to also get `pipeline` and `scenes`: each scene's `scene_id`, `scene_number`, `label`, `no_character_present` and `cards` (subjects, extras, location, ingredients in that order, as mapped), each with `map`, `card_id`, `name`, `reference_type`, `source` (`refmod` or `image`), `refmod` (`name`, `found`, `kind`, `tokens`, `strength`, read from the RefMod file like the render, or `null`) and `picture` (`path`, `exists`, or `null`). Read-only. An unknown include returns 400. (query: include, scene_id)
- `PUT /projects/{pid}/references/scene-mapping` Set which characters, locations, ingredients and extras each scene uses. Sending `subjects` or `locations` also turns on that "use reference" switch, as the Builder does.
- `PUT /projects/{pid}/references/subjects/{rid}` Create or update a character (name, description, `reference_type`, voice, trigger phrase, `image`). To render with a saved RefMod, send `source: "refmod"` and `refmod: {"name": "vehicle/C8_Corvette", "strength": 1}` (a name from GET /refmods, strength 0 to 1, default 1): the folder, type, kind, tokens and frames are read from the RefMod file, and an unknown name is a 400. `source: "image"` takes the RefMod off again. In Speaking mode also accepts `elevenlabs_voice`: {enabled: boolean, voice_id: string, name: string}. Enabled assignments require a voice ID and a primary character reference. Also accepts elevenlabs_voice_design editing fields: user_input, voice_description, preview_text, voice_name, model_id. Only durable draft fields are stored; preview audio and temporary IDs are discarded. Preserves existing voice and design fields when omitted; supports If-Match.
- `POST /projects/{pid}/references/subjects/{rid}/voice-design/description` (job) Speaking mode only: submit an LLM job to turn user_input (1–4000 characters) and an existing primary character into a reviewable voice_description. Uses the project's saved LLM Runner, and only an already loaded LM Studio model. Does not call ElevenLabs or persist the result. Wait for the returned job ID. The project's own key is used; without one, the key saved in H3 Studio's App Settings. (body: user_input)
- `DELETE /projects/{pid}/references/{kind}/{rid}` Delete a character or location (`kind` is `subjects` or `locations`) and its scene mappings.
- `POST /projects/{pid}/references/{kind}/{rid}/describe` (job) Describe a character or location image with the project's LLM (Gemma Describe) and save the description.

## refmods

- `GET /refmods` Saved RefMods in models/refmods with their type (folder), kind, frame count, tokens and description. Filter with `folder` (for example `identity`). A project switched to the RefMod pipeline (`pipeline: refmod` in the MiniMax H3 settings) renders from these. (query: folder)
- `POST /refmods` (job) Make a saved RefMod from pictures, like RefMods Studio's Create. Needs `name`, `type` (identity, clothing_men, clothing_women, background, generic, object, prop, vehicle or creature) and `paths` (picture files on this computer, one or more, in order). Optional: `crops` (one [x0, y0, x1, y1] or null per picture), `quality` (maximum, high, balanced by default, compact or draft), `mode` (`Full Reference` by default or `Compressed Reference`), `steps` (0 to 2000, default 500), `description` and `overwrite` (replace a RefMod of the same name). A bad request is a 400 with no job. Needs ComfyUI with ComfyUI-MiniMaxH3Mod and the MiniMax H3 video VAE. The job result has `path`, `folder`, `name`, `kind`, `tokens`, `latent_frames` and `canvas`. Then pick the RefMod on a card with PUT references/subjects/{rid}: `source: "refmod"` and `refmod: {"name": "<type>/<name>"}`. (body: crops, description, mode, name, overwrite, paths, quality, steps, type)

## scenes

- `GET /projects/{pid}/scenes` List scenes. Filter with `has_image`, `has_prompt`, `has_video` and `status`. (query: has_image, has_prompt, has_video, status)
- `POST /projects/{pid}/scenes` Insert a scene (`position`, `ref_scene_id`, `duration`, `label`, `notes`, prompts). Later scene files are renumbered. (body: duration, i2v_prompt, label, notes, position, ref_scene_id, t2i_prompt)
- `POST /projects/{pid}/scenes/bulk` Apply several scene operations in one atomic change. `operations` is a non-empty list of objects: `{"op": "move", "scene_id", "start", "ripple"}`, `{"op": "resize", "scene_id", "duration" or "end", "ripple"}` or `{"op": "patch", "scene_id", "fields": {...}}`. `scene_id` is a scene id or its number on the timeline. An operation with another `op`, a missing field or an unknown scene fails the whole batch with a 400 or 404 and saves nothing. (body: operations)
- `DELETE /projects/{pid}/scenes/{sid}` Delete a scene. `ripple` closes the gap. Later scene files are renumbered. (query: ripple)
- `GET /projects/{pid}/scenes/{sid}` One scene: timing, lyrics, story beat, prompts (including `minimax_h3_prompt`), approved image, rendered video with its thumbnail, and scene audio. Each file is `null` when it does not exist. `scene_card` is the complete Storyboard scene card with every field PATCH can edit (Director Notes `timeline_note`, Video Notes `motion_summary`, Planning Notes `notes`, camera, performance, references, triggers and prompts). Read-only, under the Builder's own names: `audio_mask` (the scene's Audio Mask stems, or `null`), `use_scene_minimax_h3_settings` and the scene's own `minimax_h3_settings` (its locked MiniMax H3 settings such as `continuity_mode`, or `null`), and the continuity its last render used, `minimax_h3_continuity_mode_used` and `minimax_h3_continuity_source_scene_id`. `minimax_h3_attention` is the attention the scene renders with, resolved like the render (scene settings when locked, else the project's): `source` (`scene` or `project`), `render_pass`, `sage_attention`, `use_memory_efficient_sage_attention`, `block_sparse_attention` and `block_sparse_attention_by_pass`. The scene list leaves these out.
- `PATCH /projects/{pid}/scenes/{sid}` Edit a scene card. Fields are saved on the timeline scene with the Builder's keys, and the open Video Builder shows the change (fields marked card only belong to a Storyboard card, which this app does not have): `timeline_note` (Director Notes; alias `director_note`), `i2v_notes` (Video Notes; saved as `motion_summary` on the card; aliases `video_notes`, `motion_summary`), `notes` (Planning Notes), `prompt_summary` (card only), `label`, `lyric_text` (alias `lyrics`; sets `lyric_no_lip_sync` from the text unless you send it), `lyric_section`, `story_beat`, `lyric_singers`, `lyric_cue_map`, `lyric_no_lip_sync`, `lyric_instrumental`, `lyric_performance_mode`, `lyric_shot_word_timing_enabled`, `speaker_assignments`, `shot_type`, `camera_motion`, `character_motion`, `no_character_present`, `performance_mode` (`singing`, `speaking`, `no_lip_sync`), `performance_style`, `facial_performance`, `facial_performance_custom`, `emotion_expression_tags` (up to 1200 characters; scene mood/expression direction interpreted by the LLM), `include_microphone`, `audio_direction`, `continuity`, `flf_start_state`, `flf_transformation`, `flf_end_state`, `flf_carry_forward`, `video_style`, `video_style_custom`, `temporal_world_effect_override`, `temporal_world_effect_custom`, `trigger_phrase`, `trigger_position` (card only), `video_prompt_type`, `subject_ids` (the scene's characters) and `location_id` (the Reference Builder scene maps), `image_prompt` and `video_prompt` (written like a Builder prompt edit for the project's image model and video engine; `video_prompt_origin`), `minimax_h3_pass2_prompt`. Also the single fields `t2i_prompt`, `i2v_prompt`, `enhance_prompt`, `minimax_h3_prompt`, `flux_prompt`, `nb_prompt`, `flow_gpt_prompt`, `ernie_t2i_prompt`, `minimax_h3_i2v_frame_mode` (`normal` or `flf`, per scene), `first_last_frame_end_image_path` (explicit last image for I2V FLF), `minimax_h3_continuation_direction` (what a scene continued with `latent_continuation_masked` does after its first moments, one smooth movement, no cut), `minimax_h3_continuation_start_seconds` (second of that scene where the direction starts: 0.5 by default, at most half the scene, `null` resets), `start`, `end`, or per-scene `use_scene_*` / `*_settings`. `use_scene_minimax_h3_settings` must be a boolean, and `minimax_h3_settings` is checked against GET /settings/minimax-h3/schema (for example `sage_attention`, `use_memory_efficient_sage_attention`, `use_block_sparse_attention`, `two_pass_use_block_sparse_attention`, `pass1_use_block_sparse_attention`, `pass2_use_block_sparse_attention`) and merged into the scene's saved settings; an invalid value returns 400 and nothing is saved. Only the keys sent change; an empty string, `false`, `0` or `[]` is saved as sent, so a field can be cleared or turned off. A field that cannot be patched returns a validation error that lists the supported fields, and nothing is saved. `storyboard_card` in the reply lists the Storyboard card fields that changed. The scene_defaults object stores per-scene Scene Defaults under the project setting names (camera_flow, camera_motion_speed, character_motion_speed, minimax_h3_cut_frequency, minimax_h3_cut_style, performance/facial fields and visual options); false, zero and empty values are explicit overrides. Story arc detail stays project-wide. Nonempty scene_defaults objects merge existing overrides; an empty object clears that record.
- `GET /projects/{pid}/scenes/{sid}/audio-settings` Speaking mode only: read scene overrides, effective/project settings, scene-owned dialogue clips, audio span, padded total duration, current scene duration, needs_render and the scene_dialogue editor draft. Uses the scene ID, not its position.
- `PATCH /projects/{pid}/scenes/{sid}/audio-settings` Speaking mode only: patch `settings` (`use_project_defaults`, `fit_duration`: booleans; `silence_before`, `silence_after`: 0–60 seconds). Optionally import base64/data-URL `audio_data` with `audio_name`, or remove owned dialogue with `clear_audio: true`. Also accepts a `dialogue` draft (speaker_id, text, delivery, allow_rewrite, model_id) saved on that scene as scene_dialogue. Generated audio and API keys are never stored in the draft. Fits duration to dialogue plus silence and ripples later scenes and attached audio; independent tracks stay fixed. Originals are preserved. Supports If-Match; frozen timelines and running scene renders reject edits. (body: audio_data, audio_name, clear_audio, dialogue, settings)
- `POST /projects/{pid}/scenes/{sid}/dialogue/craft` (job) Speaking only: create an LLM job using the saved runner, assigned character voice and elevenlabs_dialogue instruction preset. Optional dialogue object overrides the saved scene draft: speaker_id, text, delivery, allow_rewrite (default false), model_id (eleven_v4 default or eleven_v3). Returns editable tagged text; preserves spoken words unless rewriting is explicitly allowed. Does not call ElevenLabs or save the draft. Wait for the returned job ID. (body: dialogue)
- `POST /projects/{pid}/scenes/{sid}/dialogue/generate` Speaking only: explicitly generate one MP3 take with the saved ElevenLabs key and the chosen character's enabled voice. Optional dialogue overrides the saved scene_dialogue draft. Returns audio_data (MP3 data URL), audio_name, voice_id and dialogue. Uses credits; never retries automatically and does not write project state. Preview, then import via PATCH audio-settings with audio_data, audio_name, dialogue and If-Match to fit/ripple the scene. The project's own key is used; without one, the key saved in H3 Studio's App Settings. (body: dialogue)
- `POST /projects/{pid}/scenes/{sid}/merge` Merge a scene with its neighbour (`with_direction`: previous or next). Lyrics are joined. (body: with_direction)
- `GET /projects/{pid}/scenes/{sid}/minimax-references` What the Video Builder's Choose MiniMax References button shows for a scene: `available` (every character, extra, location and ingredients sheet with an image, each with its `key`, whether the scene mapping already picks it, and its `image_number` when selected), `selected` (the order sent to MiniMax), `custom` (chosen by hand or following the scene mappings), `automatic_keys` and the `limits` (`max_choices` is 8: a scene sends at most 8 references, and the scene image used as Image 1 is a ninth image) and `over_limit_by` (how many mapped references the render leaves out). `pipeline` says which view you got: `standard` (the view above) or, when the MiniMax H3 project renders with RefMods, `refmod`: `items` in render order, each with its prompt `label` (`<Video N>`, `<Picture N>`, `<Audio N>`), card, `category`, `reference_type` (props included), `strength`, `tokens` and `source` (`refmod` name, `found`, `kind`, `frames`, `canvas`, read from the RefMod file like the render), `labels`, `tokens` (`total`, `limit`, `over_limit`, `imbalance`), `warnings` and `editable: false`.
- `PUT /projects/{pid}/scenes/{sid}/minimax-references` Choose the ordered MiniMax references for a scene: `keys` is the list from `available` (for example `["subject:ava", "location:roof", "location:alley"]`, several locations are allowed), or `automatic: true` to follow the scene mappings again. Unknown or repeated keys and too many keys are refused with the allowed keys listed. In a RefMod project the render ignores this order, so the call returns 422 NOT_SUPPORTED_FOR_MODE and saves nothing; change the scene's references with PUT /references/scene-mapping instead. _(If-Match)_ (body: automatic, keys)
- `POST /projects/{pid}/scenes/{sid}/move` Move a scene to `start_time`. `ripple` shifts the others. (body: ripple, start_time)
- `GET /projects/{pid}/scenes/{sid}/prompts/context` The context brief an outside agent needs to write a prompt itself: cast, cut plan and character budget (`kind`). A scene continued with `latent_continuation_masked` also gets a `continuation` block: `hold_seconds`, the scene's `direction` and the `rules` for writing it as the next moment of the previous scene's take. (query: kind)
- `POST /projects/{pid}/scenes/{sid}/prompts/edit` (job) Rewrite an existing scene prompt following an instruction. (body: scene_id)
- `POST /projects/{pid}/scenes/{sid}/prompts/enhance` (job) Improve an existing scene prompt with the LLM. (body: scene_id)
- `POST /projects/{pid}/scenes/{sid}/prompts/image` (job) Write one scene's image prompt with the LLM, from the scene's notes, lyric and references (`user_notes` overrides them). (body: scene_id)
- `POST /projects/{pid}/scenes/{sid}/prompts/minimax/assemble` Build a MiniMax prompt from shot descriptions you provide (`shots`, `mode`). `save` stores it. A reference-to-video prompt gets the same subject definitions and soundscape as `minimax-prompts`. (body: mode, save, shots)
- `POST /projects/{pid}/scenes/{sid}/prompts/minimax/validate` Check a MiniMax prompt against the length and format rules. (body: mode, prompt)
- `POST /projects/{pid}/scenes/{sid}/prompts/video` (job) Write one scene's video prompt with the LLM (`mode`), from its image prompt, motion notes and references. (body: scene_id)
- `PUT /projects/{pid}/scenes/{sid}/prompts/{field}` Set a prompt field directly (`t2i_prompt`, `i2v_prompt`, `minimax_h3_prompt`, ...). Body: `prompt`, `origin`. (body: origin, prompt)
- `POST /projects/{pid}/scenes/{sid}/resize` Change a scene's length (`duration` or `end_time`). `ripple` shifts the others. (body: duration, end_time, ripple)
- `POST /projects/{pid}/scenes/{sid}/split` Split a scene at `at_time`. `clear_right_media` drops the media on the new right half. Lyrics follow their words. (body: at_time, clear_right_media)
- `DELETE /projects/{project_id}/scenes/{scene_id}/image` Remove the scene's image.
- `PUT /projects/{project_id}/scenes/{scene_id}/image` Set the scene image from a file (`source_path`) or upload (`image_data`). (body: image_data, source_path)
- `POST /projects/{project_id}/scenes/{scene_id}/image/approve` Approve one generated image as the scene's image (`image_path`). (body: image_path)
- `POST /projects/{project_id}/scenes/{scene_id}/image/from-video-frame` Take the scene image from a frame of a video (`source_video_path`). (body: source_video_path)
- `POST /projects/{project_id}/scenes/{scene_id}/image/generate` (job) Generate a scene image with ComfyUI.
- `POST /projects/{project_id}/scenes/{scene_id}/image/revert` Step through the scene's image history (`index` or `delta`). (body: delta, index)
- `DELETE /projects/{project_id}/scenes/{scene_id}/latent` Delete a scene's latent (`all`, `reindex`). (body: all, reindex; query: all, reindex)
- `GET /projects/{project_id}/scenes/{scene_id}/latent` One scene's latent file and status.
- `POST /projects/{project_id}/scenes/{scene_id}/post/lut` (job) Apply a LUT to a scene video. (body: project_id, scene_id)
- `DELETE /projects/{project_id}/scenes/{scene_id}/video` Clear the scene's active video. Files stay on disk.
- `POST /projects/{project_id}/scenes/{scene_id}/video/match-start-color` (job) Match the opening colors of a scene video to the previous scene's last frame.
- `POST /projects/{project_id}/scenes/{scene_id}/video/minimax-stage-recover` Recover a MiniMax pass-1 or pass-2 clip from the scratch folder.
- `POST /projects/{project_id}/scenes/{scene_id}/video/recover` Bring back a scene video from a backup or file (`source_path`). (body: source_path)
- `POST /projects/{project_id}/scenes/{scene_id}/video/render` (job) Render one scene's video with ComfyUI (`mode`, e.g. `minimax_h3`). To render against another sound than the project song, send `audio_path` (for example the `no_vocals_path` of POST audio/stems). Trims to the exact timeline length and saves it as `video_NNNN-audio.mp4`. Set `audio_mode: built_in_audio` in the MiniMax settings for H3 voices and sound (all MiniMax video modes and supported passes, including 2 Pass Advanced). With `continuity_mode: latent_continuation_masked` the previous scene must be rendered first (`PREDECESSOR_MISSING` otherwise); it works in Single and 2 Pass. When `continuity_prompt_from_last_frame` is also true, the scene's prompt is first written by the LLM from the previous scene's rendered final frame (the Video Builder's automatic prompt), saved on the scene, and then rendered. A `prompt` in `params` skips that.
- `POST /projects/{project_id}/scenes/{scene_id}/video/select` Choose which take is the scene's active video (`source_path`). (body: source_path)
- `GET /projects/{project_id}/scenes/{scene_id}/video/takes` List the scene's raw (untrimmed) renders, newest first, with length, frame count and whether the file still exists. Takes in a sibling scratch folder with the same project name are marked `other_folder`. Only renders made through the API are always kept: the Video Builder deletes its scratch renders after each render. Use `take` on the trim call.
- `POST /projects/{project_id}/scenes/{scene_id}/video/trim` (job) Trim a scene video as a job. Name the source with `source_path` (default: the scene's current video) or with `take` (`latest` or an index from `video/takes`, the raw render, so the clip can start earlier or end later than the current one). `start` is seconds into the source; give `duration` (and `frames`), or `to_end: true` to end on the source's last frame. Records the new clip on the scene and keeps the old one in its video history.

## service

- `GET /projects` List projects found in the allowed project roots. `root` limits the search to one root. (query: root)
- `POST /projects` Create a project: folder, empty session seeded with your saved model defaults. Body: `name`, optional `template_from`. (body: name, template_from)
- `DELETE /projects/{pid}` Delete a project folder from disk. Needs `confirm` equal to the project id. (query: confirm)
- `GET /projects/{pid}` The project with its settings, scenes, audio, story and references. `include` picks some of those groups and/or top-level session keys by name (e.g. `audio_path`, `detected_tempo_bpm`, `flux_reference_builder`; API keys come back blank). A Builder session key the project has not saved yet comes back empty (`{}` for `flux_reference_builder`, `builder_story_layer` and `builder_storyboard_defaults`, otherwise `null`). A name that is not a Builder session key is a 400 that lists it. (query: include)

## settings

- `GET /projects/{pid}/settings` Effective settings in every group, including all MiniMax H3 options.
- `PATCH /projects/{pid}/settings` Change settings by group, for example `{"minimax_h3": {"render_pass": "two_pass"}}` or the attention options `{"minimax_h3": {"two_pass_use_block_sparse_attention": false, "sage_attention": "auto", "use_memory_efficient_sage_attention": true}}`. Honors If-Match (409 on a stale revision); an invalid value returns 400 SETTINGS_INVALID. Saved under the same keys the Video Builder uses. The project group accepts `use_structured_outputs` and `omit_lyrics_from_video_prompts` (both default false); lyric omission applies to singing with supplied audio only.
- `POST /projects/{pid}/settings/preflight` Check the saved settings against the chosen video and image modes and report problems before rendering.
- `GET /settings/minimax-h3/schema` Every MiniMax H3 video setting that can be patched under the `minimax_h3` group: type, default, allowed values and limits. `continuity_mode` takes `latent_continuation_masked` (the previous scene's latent is protected at the start of the next, Single and 2 Pass), `latent_context_frames` takes 39, 90, 141 or 192 for it, and `location_transition_preset` takes `masked`.

## slideshow

- `POST /projects/{project_id}/slideshow` (job) Make a video from the scene images instead of rendered clips.

## speaking-audio

- `GET /projects/{pid}/speaking-audio/defaults` Speaking mode only: read project defaults for `silence_before`, `silence_after` (seconds, initially 0) and `fit_duration` (initially true).
- `PATCH /projects/{pid}/speaking-audio/defaults` Speaking mode only: patch `settings` (`silence_before`, `silence_after`: 0–60 seconds; `fit_duration`: boolean). Reposition scene-owned dialogue and resize/ripple scenes inheriting defaults. Independent music/effect tracks stay fixed. Source files and scene numbering are preserved. Supports If-Match; frozen timelines and running scene renders reject edits. (body: settings)

## stitch

- `POST /projects/{project_id}/stitch` (job) Join the scenes' videos into one video (`output_prefix`, default `FINAL_VIDEO`), frame-accurate to the timeline like the Builder's stitch: in a MiniMax H3 project every clip is synced to its scene's frames at 24 fps, so the output has exactly the timeline's frames. `scene_ids` picks scenes by number (1 = first scene) or id and stitches them in timeline order (default: every scene); an unknown scene or a selected scene without a video is a 400. `audio`: `auto` (default) uses the scenes' own audio when every selected MiniMax scene renders with built-in audio, the project song otherwise; `embedded` or `project` forces one. With the project song (or `audio_path`) a selection is cut to its window of the song, so it must be contiguous. `overlays` adds insert clips (`path`, `start`, `end`, `source_start`). The job result lists the stitched `scene_ids`, `audio`, the requested song window (`requested_audio_start`, `requested_audio_duration`; `audio_start` repeats the start) and `expected_frame_count`, and describes the written file from ffprobe: `output_width`, `output_height`, `output_duration` and `audio_duration` (= `output_audio_duration`, the audio actually in the file). An unknown body field is a 400. When the project has overlays (GET /projects/{pid}/overlay-layers), the stitch draws them onto the video and the job result has an `overlay_layers` section (applied, clips, skipped). Send `overlay_layers: false` to leave them out. The project's transitions (GET /projects/{pid}/transitions) are drawn on the video where their blocks lie, and the job result has `transitions` and `transition_notes` when there are any. Send `transitions` (a list of {transition, start, seconds, params}, start on the timeline in seconds) to use other ones for this stitch. The project's filters (GET /projects/{pid}/filters) and effects (GET /projects/{pid}/effects) are drawn on the joined video after the transitions, filters first, and the job result has `looks` and `look_notes` when there are any. Send `filters` or `effects` (a list of {look, start, seconds, intensity, params}) to use other ones for this stitch. (body: audio, audio_path, output_prefix, overlays, scene_ids)

## story

- `GET /projects/{pid}/story` The saved story layer: idea, arc, brief.
- `PUT /projects/{pid}/story` Replace the saved story layer.
- `PUT /projects/{pid}/story/settings` Save the Storyboard scene defaults (`defaults`: every key the Builder saves in `builder_storyboard_defaults`, e.g. video style, camera flow, motion speeds, cut frequency, short film planning, temporal and FX settings, plus `story_arc_detail`) and the story fields (`story`: every key of `builder_story_layer`, e.g. `enabled`, idea, strength, world style). Story Mode fields: `story_book_text` replaces the book (parsed into chapters and settings: `Chapter N: title`, then `Setting: name`), `story_settings` (setting looks: a list of `{id, name, look}`, ids as the book gave them), `story_acts` (a list of `{id, chapter_id, title, summary, cast_ids, scene_ids, location_note, direction}`, refused unless every scene is in exactly one act, in order), `story_scene_plan` (a list of `{scene_id, act_id, story_slice, spot, mood, performance_mode, cast_ids, continues_previous, continuation_direction}` with `performance_mode` singing, speaking, no_lip_sync or instrumental; it also sets each scene's performance, maps each planned scene's `cast_ids` onto the scene and makes a scene that continues its predecessor a masked continuation) `story_spot_min_scenes` (1-20), `story_cast_ids` (subject ids the story is about; empty means every card) `story_allow_new_cast` (true lets the story add people with no card) and `story_video_style` (`music_video`, `spoken` or `story_only`: how the story is told on screen).
- `POST /projects/{pid}/story/{step}` (job) Write a story step with the project's LLM. `step` is `arc` (from the story idea, lyrics and saved timed Timeline Notes), `brief` or `beats` (a beat per scene without one; `replace_existing`, `scene_ids`, `limit`). Story Mode steps: `book` (the story book from `story_idea` and `tone_notes`; lyrics set mood only), `split` (a story you wrote, sent as `text`, divided into chapters with a setting each, every word kept), `convert` (an older project's song-section story arc becomes the book the same way; the arc is kept), `remove_people` (rewrite the book without the people in `names`, so only the story cast is left; acts and plan are cleared), `chapter` (rewrite `chapter_id` following `direction`), `cast` (story characters with no card yet; nothing saved), `acts` (each chapter gets a run of scenes and is split into acts; `chapter_id` rebuilds one chapter; timed Timeline Notes stay inside one act) and `plan` (each scene of each act gets `story_slice`, `spot` inside the chapter's setting, `mood` and `performance_mode`; `act_id` rebuilds one act; `spot_min_scenes` overrides the saved scenes-per-spot). The plan also writes each scene's performance to the timeline (`performance_mode`, `lyric_no_lip_sync`). `locations` gives each chapter setting a look and each place a location card named `Setting - Spot`, and maps every scene to its spot (`flux_reference_builder.scene_map`); it uses the story and `style_theme` only, never the lyrics. Run it before the acts (no plan yet) and it finds the places inside each chapter setting, the Story Builder's Places tab, keeping the location cards you made by hand. Run it with `map_only: true` after the plan and it asks the LLM nothing: each scene is mapped to the card of its spot, and a card is linked to its setting and acts. `regenerate_looks` rewrites setting looks, `replace_descriptions` rewrites card text (images are never touched). Director, Video and Planning Notes and timed notes win over the story. The book is saved in the story layer as `story_book`, `story_book_text` and `story_settings`; edit the text with PUT /story/settings `story.story_book_text`. Each step also updates the Storyboard Builder's saved copy.

## summary

- `GET /projects/{pid}/summary` Compact status: scene counts, how many have images, prompts and videos, total duration.

## timeline

- `POST /projects/{pid}/timeline/bulk` Rebuild or extend the whole timeline from text. `mode` is `durations`, `ranges` or `markers`; `action` is `replace` or `append` (`append_start` sets where appended scenes begin). A line can end with the words for its scene, for example `12.5 --> 16.0 Hello darkness`. On `replace`, when no line carries words, the lyrics of the scenes being replaced move onto the new scenes by time, so they are not lost. Returns `scene_count`, `scenes_with_lyrics` and `lyrics` (`from_text`, `carried_over` or `none`). (body: action, append_start, clear_media, mode, text)
- `POST /projects/{pid}/timeline/calibrate` Shift all beat markers by `offset_seconds`. (body: offset_seconds)
- `POST /projects/{pid}/timeline/close-gaps` Remove gaps between scenes by moving later scenes earlier.
- `POST /projects/{pid}/timeline/enforce-length` Merge scenes shorter than `min_scene_seconds` and cut scenes longer than `max_scene_seconds`. `dry_run` shows the plan only. Scenes with rendered video are left alone. (body: dry_run, max_scene_seconds, min_scene_seconds)
- `POST /projects/{pid}/timeline/from-lines` (job) Make the timeline scenes from the lyrics, like Line Mapping: one scene per lyric line, short scenes merged and long ones split to fit `min_scene_seconds` and `max_scene_seconds`. Turns on the lyric lane.
- `GET /projects/{pid}/timeline/notes` The timed Timeline Notes (the Builder's `+ Timeline Note` markers, saved as `timeline_markers`): `id`, `start` and `end` seconds (`end` is `null` for a point note), `type`, `label`, `note`, and `scene_ids`, the scenes each note overlaps. Story Arc generation uses them as timed story events.
- `POST /projects/{pid}/timeline/notes` Add a timed Timeline Note. `start` (seconds, required), `end` (later than `start`; leave out or `null` for a point note), `type`, `label`, `note`, optional `id`. The open Video Builder shows it.
- `DELETE /projects/{pid}/timeline/notes/{nid}` Delete a timed Timeline Note.
- `PATCH /projects/{pid}/timeline/notes/{nid}` Change the sent fields of a timed Timeline Note (`start`, `end`, `type`, `label`, `note`). The id never changes; `end: null` makes it a point note.
- `POST /projects/{pid}/timeline/snap` Snap a scene edge to the nearest beat (`scene_id`, `edge`, `scope`). (body: edge, scene_id, scope)

## transitions

- `GET /projects/{pid}/transitions` The project's transitions in timeline order, as the stitch reads them: `scene_transitions`, each `{id, transition, start, end, seconds, params}` (a block of the timeline like an overlay: `start` and `end` are seconds on the timeline, `seconds` its length), `count`, and `revision`. On a cut between two scenes the block draws one scene into the other, inside a scene it is an effect on that scene's footage. Every stitch draws them, and so do the Video Builder's Render All and Export.
- `PUT /projects/{pid}/transitions` Replace all the project's transitions with `scene_transitions`, a list of `{transition, start, seconds, params, id}`. `transition` is an id from GET /transitions, `start` the time on the timeline in seconds, `seconds` the length 0.1 to 10 (default 0.5), `params` the settings to change from their defaults, `id` optional. Instead of `start`, `after_scene` (the number, 1 = first, or id of the scene before a cut) with `align` (center by default, end or start) puts the block on that cut. Blocks may not overlap: a later one starts where the one before it ends when the video is drawn. The reply is the saved list. (body: scene_transitions)
- `POST /projects/{pid}/transitions/set` Add one transition, change one, or remove one, and leave the others alone. Without an `id` a new block is added at `start`, or on the cut after `after_scene` (placed around the cut by `align`). With the `id` of an existing block only the fields sent change. A `transition` that is null, empty or `none` removes the block with that `id`, or the blocks that cover the cut after `after_scene`. `seconds` and `params` are as in PUT /projects/{pid}/transitions. The reply is the whole list. (body: id, transition, start, after_scene, seconds, align, params)
- `GET /transitions` The transitions the timeline's transition lane can hold: `transitions` (each with `id`, `origin` (`original` for the app's own, `library` for the gl-transitions collection), `author`, `license`, `description`, `tags` and `params`: name, type, default and range of each setting), and `count`. Use an `id` as `transition` in the project's transitions or in a stitch.

## validate

- `POST /projects/{pid}/validate` Check the project is consistent: files exist, scene timing, references, settings.

## video

- `POST /projects/{project_id}/video/graph` Build the ComfyUI graph for a video `mode` and the given parameters without running it, to inspect what would be sent.
- `POST /projects/{project_id}/video/render` (job) Render many scenes' videos one after another as a GPU job, and stitch them when asked.
- `POST /projects/{project_id}/video/scan` Scan the project folder for rendered scene videos and thumbnails and re-link them to scenes.
