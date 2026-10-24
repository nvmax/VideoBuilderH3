import { createResource, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { apiBase, call, get, initApi } from "./api";
import { closeProject, notice, openProject, project, projectId, say, startEvents } from "./store";
import Editor from "./components/Editor";
import Setup from "./components/Setup";
import "./App.css";

interface ProjectRow {
  id: string;
  name: string;
  scene_count: number;
  has_audio: boolean;
  revision: number;
  updated: number;
}

function Health() {
  const [health, { refetch }] = createResource(() => get<any>("/health").catch(() => null));
  const timer = window.setInterval(() => void refetch(), 15000);
  onCleanup(() => window.clearInterval(timer));
  return (
    <span class="health" title={apiBase()}>
      <span class={health()?.comfy_up ? "dot ok" : "dot bad"} /> ComfyUI {health()?.comfy_up ? "connected" : "not reachable"}
      <Show when={health() && !health().ffmpeg_available}>
        <span class="bad"> · ffmpeg not found</span>
      </Show>
    </span>
  );
}

function ProjectList() {
  const [rows, { refetch }] = createResource(() => get<ProjectRow[]>("/projects"));
  const [name, setName] = createSignal("");

  const create = async () => {
    try {
      const made = await call<any>("POST", "/projects", { name: name() });
      await openProject(made.data.project_id);
    } catch (e) {
      say(String((e as Error).message), true);
    }
  };

  const remove = async (row: ProjectRow) => {
    if (!window.confirm(`Delete "${row.name}" and all its files? This cannot be undone.`)) return;
    try {
      await call("DELETE", `/projects/${encodeURIComponent(row.id)}?confirm=${encodeURIComponent(row.id)}`);
      void refetch();
    } catch (e) {
      say(String((e as Error).message), true);
    }
  };

  return (
    <section class="panel narrow">
      <h2>Projects</h2>
      <div class="row">
        <input placeholder="New project name" value={name()} onInput={(e) => setName(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && name().trim() && void create()} />
        <button class="primary" disabled={!name().trim()} onClick={create}>
          Create
        </button>
      </div>
      <Show when={rows.error}>
        <p class="bad">{String(rows.error?.message ?? rows.error)}. Start the app or run `h3 serve`.</p>
      </Show>
      <ul class="list">
        <For each={rows() ?? []} fallback={<li class="muted">No projects yet.</li>}>
          {(row) => (
            <li>
              <button class="link" onClick={() => void openProject(row.id)}>
                {row.name}
              </button>
              <span class="muted">
                {row.scene_count} scenes{row.has_audio ? "" : ", no song"}
              </span>
              <button class="small danger" onClick={() => void remove(row)}>
                Delete
              </button>
            </li>
          )}
        </For>
      </ul>
    </section>
  );
}

export default function App() {
  const [ready, setReady] = createSignal(false);
  const [setup, setSetup] = createSignal(false);
  onMount(async () => {
    await initApi();
    startEvents();
    // The first time the app runs, or while something it needs is not ready, it opens on the setup screen.
    try {
      const status = await get<{ first_run: boolean; ready: boolean }>("/setup/status");
      if (status.first_run || !status.ready) setSetup(true);
    } catch {
      /* the project list shows that the API is not reachable */
    }
    setReady(true);
  });
  return (
    <div class="app">
      <header class="top">
        <h1>VRGDG H3 Studio</h1>
        <Show when={project()}>
          <span class="muted">{project()!.name}</span>
          <button class="small" onClick={closeProject}>
            All projects
          </button>
        </Show>
        <span class="spacer" />
        <button class="small" onClick={() => setSetup(!setup())}>
          Setup
        </button>
        <Health />
      </header>
      <Show when={notice()}>
        <div class={notice()!.bad ? "toast bad" : "toast"}>{notice()!.text}</div>
      </Show>
      <Show when={ready() && setup()}>
        <Setup onClose={() => setSetup(false)} />
      </Show>
      <Show when={ready() && !setup()}>
        <Show when={projectId()} fallback={<ProjectList />}>
          <Show when={project()} fallback={<p class="muted pad">Opening...</p>}>
            <Editor />
          </Show>
        </Show>
      </Show>
    </div>
  );
}
