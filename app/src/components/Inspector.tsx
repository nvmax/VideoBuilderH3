import { createEffect, createMemo, createResource, createSignal, For, on, Show } from "solid-js";
import { call, encodePid, get } from "../api";
import { edit, projectId, renderSelectedScene, selectedScene, writeScenePrompt } from "../store";

const LIMIT = 7000;

function Mapping(props: { sceneId: string }) {
  const [refs, { refetch }] = createResource(() => projectId(), async (pid) => ({
    cards: await get<any>(`/projects/${encodePid(pid)}/references`),
    map: (await get<any>(`/projects/${encodePid(pid)}/references/scene-mapping`)).scene_mapping,
  }));
  const subjects = () => refs()?.cards.subjects ?? [];
  const locations = () => refs()?.cards.locations ?? [];
  const mappedSubjects = (): string[] => refs()?.map.subjects?.[props.sceneId] ?? [];
  const mappedLocation = (): string => refs()?.map.locations?.[props.sceneId] ?? "";

  const setSubjects = async (id: string, on: boolean) => {
    const next = on ? [...mappedSubjects(), id] : mappedSubjects().filter((s) => s !== id);
    await edit("PUT", "/references/scene-mapping", { subjects: { [props.sceneId]: next } });
    void refetch();
  };
  const setLocation = async (id: string) => {
    await edit("PUT", "/references/scene-mapping", { locations: { [props.sceneId]: id } });
    void refetch();
  };

  return (
    <Show when={subjects().length || locations().length}>
      <fieldset>
        <legend>References used by this scene</legend>
        <For each={subjects()}>
          {(card: any) => (
            <label class="check">
              <input type="checkbox" checked={mappedSubjects().includes(card.id)} onChange={(e) => void setSubjects(card.id, e.currentTarget.checked)} /> {card.name}
            </label>
          )}
        </For>
        <Show when={locations().length}>
          <label>
            Location{" "}
            <select value={mappedLocation()} onChange={(e) => void setLocation(e.currentTarget.value)}>
              <option value="">None</option>
              <For each={locations()}>{(card: any) => <option value={card.id}>{card.name}</option>}</For>
            </select>
          </label>
        </Show>
      </fieldset>
    </Show>
  );
}

/** The selected scene's label, lyrics and story beat, and the characters and location it uses. */
export function SceneFields() {
  const scene = selectedScene;
  const [label, setLabel] = createSignal("");
  const [lyrics, setLyrics] = createSignal("");
  const [beat, setBeat] = createSignal("");

  // Start from the saved scene when another scene is chosen or the saved value changes. A reload that changes nothing
  // leaves what is typed alone.
  const id = createMemo(() => scene()?.id);
  const savedFields = createMemo(() => JSON.stringify([scene()?.label ?? "", scene()?.lyrics ?? "", scene()?.story_beat ?? ""]));
  createEffect(on([id, savedFields], () => {
    const s = scene();
    if (!s) return;
    setLabel(s.label ?? "");
    setLyrics(s.lyrics ?? "");
    setBeat(s.story_beat ?? "");
  }));

  const saveFields = () => edit("PATCH", `/scenes/${encodeURIComponent(scene()!.id)}`, { label: label(), lyric_text: lyrics(), story_beat: beat() }, "Scene saved.");
  const dirty = () => scene() && (label() !== (scene()!.label ?? "") || lyrics() !== (scene()!.lyrics ?? "") || beat() !== (scene()!.story_beat ?? ""));

  return (
    <Show when={scene()} fallback={<p class="muted">Choose a scene on the left or on the timeline.</p>}>
      {(s) => (
        <div class="col">
          <p class="muted">
            Scene {s().number}: {s().start.toFixed(2)} to {s().end.toFixed(2)} s ({s().duration.toFixed(2)} s)
          </p>
          <label>Label<input value={label()} onInput={(e) => setLabel(e.currentTarget.value)} /></label>
          <label>Lyrics for this scene<textarea rows="4" value={lyrics()} onInput={(e) => setLyrics(e.currentTarget.value)} /></label>
          <label>What happens (story beat)<textarea rows="3" value={beat()} onInput={(e) => setBeat(e.currentTarget.value)} /></label>
          <div class="row">
            <button class="primary" disabled={!dirty()} onClick={() => void saveFields()}>Save scene</button>
          </div>
          <Mapping sceneId={s().id} />
        </div>
      )}
    </Show>
  );
}

/** The selected scene's MiniMax H3 prompt: edit, check, write with the LLM. */
export function PromptEditor() {
  const scene = selectedScene;
  const [prompt, setPrompt] = createSignal("");
  const [check, setCheck] = createSignal<{ valid: boolean; errors: { message: string }[] } | null>(null);
  const id = createMemo(() => scene()?.id);
  const savedPrompt = createMemo(() => scene()?.minimax_h3_prompt ?? "");
  createEffect(on([id, savedPrompt], () => {
    // Saving what is typed changes nothing on screen, so its check result stays.
    if (prompt() !== savedPrompt()) {
      setPrompt(savedPrompt());
      setCheck(null);
    }
  }));
  createEffect(on(id, () => setCheck(null)));

  const savePrompt = () => edit("PUT", `/scenes/${encodeURIComponent(scene()!.id)}/prompts/minimax_h3_prompt`, { prompt: prompt(), origin: "manual" }, "Prompt saved.");
  const validate = async () => {
    const r = await call("POST", `/projects/${encodePid(projectId()!)}/scenes/${encodeURIComponent(scene()!.id)}/prompts/minimax/validate`, { prompt: prompt() });
    setCheck(r.data);
  };

  return (
    <Show when={scene()} fallback={<p class="muted">Choose a scene on the left or on the timeline.</p>}>
      {(s) => (
        <div class="col">
          <label>
            MiniMax H3 prompt <span classList={{ muted: true, bad: prompt().length > LIMIT }}>{prompt().length} / {LIMIT}</span>
            <textarea class="prompt" rows="16" value={prompt()} onInput={(e) => { setPrompt(e.currentTarget.value); setCheck(null); }} />
          </label>
          <div class="row wrap">
            <button class="primary" disabled={prompt() === (s().minimax_h3_prompt ?? "")} onClick={() => void savePrompt()}>Save prompt</button>
            <button onClick={() => void validate()}>Check prompt</button>
            <button onClick={() => void writeScenePrompt()} title="Write this scene's prompt with the LLM from its lyrics, story beat and references">Write with LLM</button>
          </div>
          <Show when={check()}>
            {(c) => <p classList={{ good: c().valid, bad: !c().valid }}>{c().valid ? "The prompt passes the checks." : c().errors.map((e) => e.message).join(" ")}</p>}
          </Show>
          <button class="create" disabled={!(s().minimax_h3_prompt ?? "").trim()} title={(s().minimax_h3_prompt ?? "").trim() ? "" : "Save a prompt first"} onClick={() => void renderSelectedScene()}>
            Create MiniMax H3 Scene Video
          </button>
        </div>
      )}
    </Show>
  );
}
