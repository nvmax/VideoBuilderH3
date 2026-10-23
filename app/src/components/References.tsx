import { createResource, createSignal, For, Show } from "solid-js";
import { call, encodePid, fileUrl, get, readFileAsBase64 } from "../api";
import { act, edit, project, projectId, say, setJobs } from "../store";

interface Card {
  id: string;
  name: string;
  description: string;
  image?: { path?: string; name?: string };
}

interface Preset {
  id: string;
  name: string;
}

function CardView(props: { kind: "subjects" | "locations"; card: Card; presets: Preset[]; scenes: number; refresh: () => void }) {
  const [name, setName] = createSignal(props.card.name);
  const [description, setDescription] = createSignal(props.card.description ?? "");
  const [preset, setPreset] = createSignal(props.presets[0]?.id ?? "zimage");
  const [prompt, setPrompt] = createSignal("");
  const base = () => `/references/${props.kind}/${encodeURIComponent(props.card.id)}`;
  const image = () => props.card.image?.path;

  const save = async () => {
    await edit("PUT", base(), { name: name(), description: description() }, "Saved.");
    props.refresh();
  };

  const upload = async (file: File | undefined) => {
    if (!file) return;
    await edit("PUT", `${base()}/image`, { image_data: await readFileAsBase64(file) }, "Picture set.");
    props.refresh();
  };

  const generate = async () => {
    const pid = projectId()!;
    await act(async () => {
      const r = await call("POST", `/projects/${encodePid(pid)}${base()}/image/generate`, { preset_id: preset(), prompt: prompt().trim() || undefined });
      setJobs((list) => [r.data.job, ...list]);
      say("Picture queued. Watch it in the Render tab.");
    });
  };

  const remove = async () => {
    if (!window.confirm(`Delete ${props.card.name}? It is taken off every scene.`)) return;
    await edit("DELETE", base());
    props.refresh();
  };

  const useEverywhere = async () => {
    const scenes = project()?.scenes ?? [];
    const key = props.kind;
    const map = Object.fromEntries(scenes.map((s) => [s.id, key === "subjects" ? [props.card.id] : props.card.id]));
    await edit("PUT", "/references/scene-mapping", { [key]: map }, `Used in all ${scenes.length} scenes.`);
  };

  return (
    <div class="card">
      <div class="pic">
        <Show when={image()} fallback={<span class="muted">No picture</span>}>
          <img src={fileUrl(image()!)} alt={props.card.name} />
        </Show>
      </div>
      <div class="col">
        <input value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <textarea rows="3" placeholder="Describe how it looks. This text also feeds the prompt." value={description()} onInput={(e) => setDescription(e.currentTarget.value)} />
        <div class="row wrap">
          <button class="primary" disabled={name() === props.card.name && description() === (props.card.description ?? "")} onClick={() => void save()}>Save</button>
          <label class="file small">
            Choose picture
            <input type="file" accept="image/*" onChange={(e) => void upload(e.currentTarget.files?.[0])} />
          </label>
          <button class="small" onClick={() => void useEverywhere()}>Use in every scene</button>
          <button class="small danger" onClick={() => void remove()}>Delete</button>
        </div>
        <details>
          <summary>Make a picture with ComfyUI</summary>
          <div class="row wrap">
            <select value={preset()} onChange={(e) => setPreset(e.currentTarget.value)}>
              <For each={props.presets}>{(p) => <option value={p.id}>{p.name}</option>}</For>
            </select>
            <input class="grow" placeholder="Prompt (the description is used when this is empty)" value={prompt()} onInput={(e) => setPrompt(e.currentTarget.value)} />
            <button onClick={() => void generate()}>Generate</button>
          </div>
        </details>
      </div>
    </div>
  );
}

export default function References() {
  const [data, { refetch }] = createResource(
    () => [projectId(), project()?.revision] as const,
    async ([pid]) => ({
      cards: await get<{ subjects: Card[]; locations: Card[] }>(`/projects/${encodePid(pid!)}/references`),
      presets: await get<Preset[]>("/image-presets"),
    }),
  );
  const add = async (kind: "subjects" | "locations") => {
    const id = `${kind === "subjects" ? "subj" : "loc"}_${Date.now()}_${Math.floor(Math.random() * 10000)}`;
    await edit("PUT", `/references/${kind}/${id}`, { name: kind === "subjects" ? "New character" : "New location" });
    void refetch();
  };
  const sceneCount = () => project()?.scenes.length ?? 0;

  return (
    <div class="refs">
      <p class="muted">Characters and locations give the model faces and places to keep consistent. Give each a picture, then choose which scenes use it.</p>
      <For each={["subjects", "locations"] as const}>
        {(kind) => (
          <section>
            <div class="row">
              <h3>{kind === "subjects" ? "Characters" : "Locations"}</h3>
              <button class="small" onClick={() => void add(kind)}>Add</button>
            </div>
            <For each={data()?.cards[kind] ?? []} fallback={<p class="muted">None yet.</p>}>
              {(card) => <CardView kind={kind} card={card} presets={data()?.presets ?? []} scenes={sceneCount()} refresh={() => void refetch()} />}
            </For>
          </section>
        )}
      </For>
    </div>
  );
}
