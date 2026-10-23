import { createEffect, createResource, createSignal, For, Show } from "solid-js";
import { call, encodePid, fileUrl, get } from "../api";
import { act, edit, projectId, say, selectedScene, setJobs } from "../store";

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

export default function Inspector() {
  const scene = selectedScene;
  const [label, setLabel] = createSignal("");
  const [lyrics, setLyrics] = createSignal("");
  const [beat, setBeat] = createSignal("");
  const [prompt, setPrompt] = createSignal("");
  const [check, setCheck] = createSignal<{ valid: boolean; errors: { message: string }[] } | null>(null);

  // Start the form from the saved scene whenever another scene is chosen or the scene is saved elsewhere.
  createEffect(() => {
    const s = scene();
    if (!s) return;
    setLabel(s.label ?? "");
    setLyrics(s.lyrics ?? "");
    setBeat(s.story_beat ?? "");
    setPrompt(s.minimax_h3_prompt ?? "");
    setCheck(null);
  });

  const saveFields = () => edit("PATCH", `/scenes/${encodeURIComponent(scene()!.id)}`, { label: label(), lyric_text: lyrics(), story_beat: beat() }, "Scene saved.");
  const savePrompt = () => edit("PUT", `/scenes/${encodeURIComponent(scene()!.id)}/prompts/minimax_h3_prompt`, { prompt: prompt(), origin: "manual" }, "Prompt saved.");
  const validate = async () => {
    const pid = projectId()!;
    const r = await call("POST", `/projects/${encodePid(pid)}/scenes/${encodeURIComponent(scene()!.id)}/prompts/minimax/validate`, { prompt: prompt() });
    setCheck(r.data);
  };
  const render = async () => {
    const pid = projectId()!;
    await act(async () => {
      const r = await call("POST", `/projects/${encodePid(pid)}/scenes/${encodeURIComponent(scene()!.id)}/video/render`, {});
      setJobs((list) => [r.data.job, ...list]);
      say("Render queued. Watch it in the Render tab.");
    });
  };

  const dirty = () => scene() && (label() !== (scene()!.label ?? "") || lyrics() !== (scene()!.lyrics ?? "") || beat() !== (scene()!.story_beat ?? ""));

  return (
    <Show when={scene()} fallback={<p class="muted pad">Choose a scene on the timeline, or add one.</p>}>
      {(s) => (
        <div class="inspector">
          <div class="col">
            <h3>
              Scene {s().number} <small class="muted">{s().start.toFixed(2)} to {s().end.toFixed(2)} s, {s().duration.toFixed(2)} s</small>
            </h3>
            <label>Label<input value={label()} onInput={(e) => setLabel(e.currentTarget.value)} /></label>
            <label>Lyrics for this scene<textarea rows="4" value={lyrics()} onInput={(e) => setLyrics(e.currentTarget.value)} /></label>
            <label>What happens (story beat)<textarea rows="3" value={beat()} onInput={(e) => setBeat(e.currentTarget.value)} /></label>
            <div class="row">
              <button class="primary" disabled={!dirty()} onClick={() => void saveFields()}>Save scene</button>
            </div>
            <Mapping sceneId={s().id} />
          </div>
          <div class="col">
            <label>
              MiniMax H3 prompt{" "}
              <span classList={{ muted: true, bad: prompt().length > LIMIT }}>{prompt().length} / {LIMIT}</span>
              <textarea class="prompt" rows="14" value={prompt()} onInput={(e) => { setPrompt(e.currentTarget.value); setCheck(null); }} />
            </label>
            <div class="row">
              <button class="primary" disabled={prompt() === (s().minimax_h3_prompt ?? "")} onClick={() => void savePrompt()}>Save prompt</button>
              <button onClick={() => void validate()}>Check prompt</button>
              <span class="spacer" />
              <button class="primary" disabled={!(s().minimax_h3_prompt ?? "").trim()} title={(s().minimax_h3_prompt ?? "").trim() ? "" : "Save a prompt first"} onClick={() => void render()}>
                Render this scene
              </button>
            </div>
            <Show when={check()}>
              {(c) => (
                <p classList={{ good: c().valid, bad: !c().valid }}>{c().valid ? "The prompt passes the checks." : c().errors.map((e) => e.message).join(" ")}</p>
              )}
            </Show>
          </div>
          <div class="col media">
            <Show when={s().rendered_video} fallback={<p class="muted">No video yet.</p>}>
              <video controls preload="metadata" src={fileUrl(s().rendered_video!.path, s().rendered_video!.mtime)} poster={s().video_thumbnail ? fileUrl(s().video_thumbnail!.path, s().video_thumbnail!.mtime) : undefined} />
              <small class="muted">{s().rendered_video!.filename}</small>
            </Show>
            <Show when={s().approved_image}>
              <img src={fileUrl(s().approved_image!.path, s().approved_image!.mtime)} alt="Scene image" />
            </Show>
          </div>
        </div>
      )}
    </Show>
  );
}
