import { createResource, For, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type MissingNode = { class_type: string; provider: string; feature: string };
type ComfyStatus = {
  url: string;
  reachable: boolean;
  error: string | null;
  comfyui_version: string;
  gpus: string[];
  core_ready: boolean;
  missing: MissingNode[];
};

function App() {
  const [status, { refetch }] = createResource(() => invoke<ComfyStatus>("comfy_status"));

  return (
    <main class="container">
      <h1>VRGDG H3 Studio</h1>
      <section class="panel">
        <div class="row-between">
          <h2>ComfyUI</h2>
          <button onClick={() => refetch()} disabled={status.loading}>
            {status.loading ? "Checking..." : "Check again"}
          </button>
        </div>
        <Show when={status.error}>
          <p class="bad">{String(status.error)}</p>
        </Show>
        <Show when={status()}>
          {(s) => (
            <>
              <p>
                {s().url}: {" "}
                <span class={s().reachable ? "good" : "bad"}>
                  {s().reachable ? `connected, version ${s().comfyui_version}` : "not reachable"}
                </span>
              </p>
              <Show when={s().error}>
                <p class="bad">{s().error}</p>
              </Show>
              <For each={s().gpus}>{(g) => <p class="muted">{g}</p>}</For>
              <Show when={s().reachable}>
                <p class={s().core_ready ? "good" : "bad"}>
                  {s().core_ready ? "All core H3 nodes are installed." : "Core H3 nodes are missing."}
                </p>
                <Show when={s().missing.length > 0}>
                  <table>
                    <thead>
                      <tr>
                        <th>Missing node</th>
                        <th>Install</th>
                        <th>Needed for</th>
                      </tr>
                    </thead>
                    <tbody>
                      <For each={s().missing}>
                        {(m) => (
                          <tr>
                            <td>{m.class_type}</td>
                            <td>{m.provider}</td>
                            <td>{m.feature}</td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </table>
                </Show>
              </Show>
            </>
          )}
        </Show>
      </section>
    </main>
  );
}

export default App;
