import { createSignal, For, Match, Switch } from "solid-js";
import Timeline from "./Timeline";
import Inspector from "./Inspector";
import SettingsPanel from "./SettingsPanel";
import References from "./References";
import Queue from "./Queue";

const TABS = ["Scene", "Settings", "References", "Render"] as const;
type Tab = (typeof TABS)[number];

export default function Editor() {
  const [tab, setTab] = createSignal<Tab>("Scene");
  return (
    <main class="editor">
      <Timeline />
      <nav class="tabs">
        <For each={TABS}>
          {(name) => (
            <button classList={{ active: tab() === name }} onClick={() => setTab(name)}>
              {name}
            </button>
          )}
        </For>
      </nav>
      <div class="tab-body">
        <Switch>
          <Match when={tab() === "Scene"}>
            <Inspector />
          </Match>
          <Match when={tab() === "Settings"}>
            <SettingsPanel />
          </Match>
          <Match when={tab() === "References"}>
            <References />
          </Match>
          <Match when={tab() === "Render"}>
            <Queue />
          </Match>
        </Switch>
      </div>
    </main>
  );
}
