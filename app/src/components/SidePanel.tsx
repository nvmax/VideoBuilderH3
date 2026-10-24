import { createEffect, createMemo, createResource, createSignal, For, on, Show } from "solid-js";
import { call, encodePid, fileUrl, get } from "../api";
import { act, attachSong, edit, project, projectId, renderSelectedScene, say, selectedScene, submitJob } from "../store";
import { PromptEditor, SceneFields } from "./Inspector";
import { MODEL_KEYS, SettingRows, useSchema, VIDEO_GROUPS } from "./SettingsPanel";

type Tab = "Image" | "Video" | "Audio";
type Sub = "Models" | "Video Settings" | "Scene" | "Prompt";

const PASS_LABEL: Record<string, string> = { single: "Single pass", two_pass: "2 pass", three_pass: "2 pass advanced" };
const MODE_LABEL: Record<string, string> = {
  text_to_video: "T2V",
  image_to_video: "I2V",
  image_reference_to_video: "Image + Ref",
  reference_to_video: "Ref to Video",
  video_to_video: "V2V",
};
const PIPELINE_LABEL: Record<string, string> = { standard: "Standard pipeline", refmod: "RefMod pipeline" };

/** A row of buttons for a setting that has a few choices. The chosen one is lit. */
function Choice(props: { key: string; labels: Record<string, string>; columns: "c2" | "c3" | "c4" }) {
  const [schema] = useSchema();
  const setting = () => schema()?.settings.find((s) => s.key === props.key);
  const value = () => String(project()?.settings.minimax_h3[props.key] ?? setting()?.default ?? "");
  return (
    <Show when={setting()?.choices?.length}>
      <div class={`seg ${props.columns}`}>
        <For each={setting()!.choices}>
          {(choice) => (
            <button classList={{ on: value() === choice }} onClick={() => void edit("PATCH", "/settings", { minimax_h3: { [props.key]: choice } })}>
              {props.labels[choice] ?? choice.replace(/_/g, " ")}
            </button>
          )}
        </For>
      </div>
    </Show>
  );
}

function VideoTab() {
  const [sub, setSub] = createSignal<Sub>("Models");
  const [all, setAll] = createSignal(false);
  const [schema] = useSchema();
  const scene = selectedScene;
  return (
    <>
      <div class="infobox">
        <strong>MiniMax H3 Project</strong>
        <small>Mode, models and video settings apply to the whole project unless the active scene is locked to its own settings.</small>
      </div>
      <Choice key="pipeline" labels={PIPELINE_LABEL} columns="c2" />
      <Choice key="video_mode" labels={MODE_LABEL} columns="c2" />
      <Choice key="render_pass" labels={PASS_LABEL} columns="c3" />
      <div class="subtabs">
        <For each={["Models", "Video Settings", "Scene", "Prompt"] as Sub[]}>
          {(name) => (
            <button classList={{ on: sub() === name }} onClick={() => setSub(name)}>
              {name}
            </button>
          )}
        </For>
      </div>
      <div class="sidebody">
        <Show when={sub() === "Models"}>
          <SettingRows keys={MODEL_KEYS} />
        </Show>
        <Show when={sub() === "Video Settings"}>
          <For each={VIDEO_GROUPS}>
            {(g) => (
              <fieldset>
                <legend>{g.title}</legend>
                <SettingRows keys={g.keys.filter((k) => !["render_pass", "video_mode"].includes(k))} />
              </fieldset>
            )}
          </For>
          <button class="small" onClick={() => setAll(!all())}>{all() ? "Hide" : "Show"} every setting ({schema()?.settings.length ?? 0})</button>
          <Show when={all()}>
            <SettingRows keys={(schema()?.settings ?? []).map((s) => s.key)} />
          </Show>
        </Show>
        <Show when={sub() === "Scene"}>
          <SceneFields />
        </Show>
        <Show when={sub() === "Prompt"}>
          <PromptEditor />
        </Show>
      </div>
      <Show when={sub() !== "Prompt"}>
        <button
          class="create"
          disabled={!scene()?.minimax_h3_prompt?.trim()}
          title={scene()?.minimax_h3_prompt?.trim() ? "" : "This scene needs a prompt first (Prompt tab)"}
          onClick={() => void renderSelectedScene()}
        >
          Create MiniMax H3 Scene Video
        </button>
      </Show>
    </>
  );
}

function ImageTab() {
  const scene = selectedScene;
  const [presets] = createResource(() => get<{ id: string; name: string }[]>("/image-presets").catch(() => []));
  const [preset, setPreset] = createSignal("");
  const [write, setWrite] = createSignal(true);
  const [prompt, setPrompt] = createSignal("");
  const id = createMemo(() => scene()?.id);
  const saved = createMemo(() => scene()?.t2i_prompt ?? "");
  createEffect(on([id, saved], () => prompt() !== saved() && setPrompt(saved())));
  const chosen = () => preset() || presets()?.[0]?.id || "zimage";
  const base = () => `/scenes/${encodeURIComponent(scene()!.id)}`;
  return (
    <Show when={scene()} fallback={<p class="muted">Choose a scene first.</p>}>
      {(s) => (
        <>
          <Show when={s().approved_image}>
            <img class="still" style={{ "max-width": "100%", "border-radius": "4px" }} src={fileUrl(s().approved_image!.path, s().approved_image!.mtime)} alt="Scene picture" />
          </Show>
          <label>
            Image prompt (T2I)
            <textarea class="prompt" rows="9" value={prompt()} onInput={(e) => setPrompt(e.currentTarget.value)} />
          </label>
          <div class="row wrap">
            <button class="primary" disabled={prompt() === saved()} onClick={() => void edit("PUT", `${base()}/prompts/t2i_prompt`, { prompt: prompt(), origin: "manual" }, "Image prompt saved.")}>Save prompt</button>
            <button onClick={() => void submitJob(`${base()}/prompts/image`, { mode: chosen() }, "Writing the image prompt.")}>Write with LLM</button>
          </div>
          <label>
            Image model (workflow preset)
            <select value={chosen()} onChange={(e) => setPreset(e.currentTarget.value)}>
              <For each={presets() ?? []}>{(p) => <option value={p.id}>{p.name}</option>}</For>
            </select>
          </label>
          <label class="check">
            <input type="checkbox" checked={write()} onChange={(e) => setWrite(e.currentTarget.checked)} /> Write the prompt with the LLM first when it is empty
          </label>
          <button class="create" onClick={() => void submitJob(`${base()}/image/generate`, { preset_id: chosen(), write_prompt: write() }, "Image queued. Watch it in the render queue.")}>
            Generate image
          </button>
          <button disabled={!s().approved_image} onClick={() => void edit("POST", `${base()}/image/approve`, {}, "Image approved.")}>Approve the newest image</button>
          <button onClick={() => void submitJob("/images/generate", { scope: "all", run_mode: "resume_missing", preset_id: chosen(), write_prompt: write() }, "Generating the pictures that are missing.")}>
            Generate every missing image
          </button>
        </>
      )}
    </Show>
  );
}

function AudioTab() {
  const pid = () => encodePid(projectId()!);
  const [lyrics, { refetch }] = createResource(() => projectId(), () => get<{ lyrics_text: string }>(`/projects/${pid()}/lyrics`).catch(() => ({ lyrics_text: "" })));
  const [text, setText] = createSignal<string | null>(null);
  const current = () => text() ?? lyrics()?.lyrics_text ?? "";
  const audio = () => project()?.audio;
  return (
    <>
      <div class="infobox">
        <strong>Song</strong>
        <small>
          <Show when={audio()?.attached} fallback="No song on this project yet.">
            {audio()!.path.split(/[\\/]/).pop()} ({audio()!.duration.toFixed(1)} s)
          </Show>
        </small>
      </div>
      <label class="file">
        {audio()?.attached ? "Replace the song" : "Add a song"}
        <input type="file" accept="audio/*" onChange={(e) => void attachSong(e.currentTarget.files?.[0])} />
      </label>
      <label>
        Lyrics
        <textarea rows="12" value={current()} onInput={(e) => setText(e.currentTarget.value)} placeholder="One line per sung line. They are used to time the scenes." />
      </label>
      <div class="row wrap">
        <button
          class="primary"
          disabled={text() === null}
          onClick={() => void act(async () => { await call("PUT", `/projects/${pid()}/lyrics`, { lyrics_text: text() }); setText(null); await refetch(); say("Lyrics saved."); })}
        >
          Save lyrics
        </button>
        <button
          disabled={!audio()?.attached}
          title="Times the lyrics against the song with ComfyUI and makes the scenes"
          onClick={() => window.confirm("Make scenes from the lyrics? This replaces the current scenes.") && void submitJob("/timeline/from-lines", { replace_existing: (project()?.scenes.length ?? 0) > 0 }, "Timing the lyrics and making scenes. This can take a few minutes.")}
        >
          Make scenes from lyrics
        </button>
      </div>
    </>
  );
}

export default function SidePanel() {
  const [tab, setTab] = createSignal<Tab>("Video");
  return (
    <aside class="side">
      <div class="tabs">
        <For each={["Image", "Video", "Audio"] as Tab[]}>
          {(name) => (
            <button classList={{ active: tab() === name }} onClick={() => setTab(name)}>
              {name}
            </button>
          )}
        </For>
      </div>
      <Show when={tab() === "Image"}>
        <ImageTab />
      </Show>
      <Show when={tab() === "Video"}>
        <VideoTab />
      </Show>
      <Show when={tab() === "Audio"}>
        <AudioTab />
      </Show>
    </aside>
  );
}
