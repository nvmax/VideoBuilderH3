import { createResource, createSignal, For, onMount, Show } from "solid-js";
import { call, get } from "../api";
import { say } from "../store";

const RUNNERS: [string, string][] = [
  ["lm_studio", "LM Studio (the model that is loaded)"],
  ["own_server", "Own server (OpenAI compatible)"],
  ["openai", "OpenAI"],
  ["anthropic", "Anthropic"],
  ["google", "Google"],
];

/** The fields that decide where the model list comes from. A key is only sent when it was typed in this session. */
const LOOKUP_FIELDS = ["runner", "lm_studio_base_url", "own_server_url"];
const LOOKUP_KEYS = ["lm_studio_api_key", "own_server_api_key", "api_key"];

interface ModelRow {
  id: string;
  loaded?: boolean;
}

export default function LlmSettings() {
  const [cfg, { refetch }] = createResource(() => get<any>("/llm/settings"));
  const [changes, setChanges] = createSignal<Record<string, unknown>>({});
  const [result, setResult] = createSignal<{ ok: boolean; text: string } | null>(null);
  const [models, setModels] = createSignal<ModelRow[]>([]);
  const [lookup, setLookup] = createSignal<{ ok: boolean; text: string } | null>(null);
  const [looking, setLooking] = createSignal(false);
  const value = (key: string) => (key in changes() ? changes()[key] : cfg()?.[key]);
  const set = (key: string, v: unknown) => setChanges({ ...changes(), [key]: v });
  const runner = () => String(value("runner") ?? "lm_studio");

  /** Ask the runner which models it has, with the values on screen (saved or not). Nothing is loaded or changed. */
  const findModels = async () => {
    setLooking(true);
    setLookup(null);
    try {
      const body: Record<string, unknown> = {};
      for (const key of LOOKUP_FIELDS) body[key] = value(key);
      for (const key of LOOKUP_KEYS) if (changes()[key]) body[key] = changes()[key];
      const reply = await call<{ models: ModelRow[]; note: string }>("POST", "/llm/models", body);
      setModels(reply.data.models);
      const n = reply.data.models.length;
      setLookup({ ok: n > 0, text: n > 0 ? `Found ${n} model${n === 1 ? "" : "s"}.` : reply.data.note || (runner() === "lm_studio" ? "LM Studio has no model loaded. Load one in LM Studio." : "The server listed no models.") });
    } catch (e) {
      setModels([]);
      setLookup({ ok: false, text: (e as Error).message });
    } finally {
      setLooking(false);
    }
  };

  onMount(() => {
    // Show the models of the saved settings as soon as the panel opens. A failure is shown quietly next to the button.
    const wait = window.setInterval(() => {
      if (cfg()) {
        window.clearInterval(wait);
        void findModels();
      }
    }, 100);
    window.setTimeout(() => window.clearInterval(wait), 5000);
  });

  const changeRunner = (id: string) => {
    set("runner", id);
    setModels([]);
    setLookup(null);
    void findModels();
  };

  const save = async () => {
    try {
      await call("PUT", "/llm/settings", changes());
      setChanges({});
      await refetch();
      say("LLM settings saved.");
    } catch (e) {
      say((e as Error).message, true);
    }
  };

  const test = async () => {
    setResult(null);
    try {
      const reply = await call<any>("POST", "/llm/test");
      setResult({ ok: true, text: `Answered by ${reply.data.model} (${reply.data.runner}): ${reply.data.reply}` });
    } catch (e) {
      setResult({ ok: false, text: (e as Error).message });
    }
  };

  const text = (key: string, label: string, kind = "text") => (
    <label class="setting">
      <span>{label}</span>
      <input type={kind} placeholder={kind === "password" && cfg()?.[`${key}_set`] ? "saved, type to replace" : ""} value={kind === "password" ? String(changes()[key] ?? "") : String(value(key) ?? "")} onInput={(e) => set(key, kind === "number" ? Number(e.currentTarget.value) : e.currentTarget.value)} />
    </label>
  );

  /** A model field: pick from what the runner reported, or type a name when the list is empty. */
  const model = (key: string, label: string) => (
    <label class="setting">
      <span>{label}</span>
      <div class="row">
        <input list="llm-models" placeholder={models().length ? "Pick a model" : "Press Find models, or type a name"} value={String(value(key) ?? "")} onInput={(e) => set(key, e.currentTarget.value)} />
        <button type="button" disabled={looking()} onClick={() => void findModels()}>
          {looking() ? "Looking..." : "Find models"}
        </button>
      </div>
    </label>
  );

  return (
    <fieldset>
      <legend>LLM for writing prompts and story steps</legend>
      <Show when={cfg()}>
        <datalist id="llm-models">
          <For each={models()}>{(m) => <option value={m.id} />}</For>
        </datalist>
        <label class="setting">
          <span>Runner</span>
          <select value={runner()} onChange={(e) => changeRunner(e.currentTarget.value)}>
            <For each={RUNNERS}>{([id, name]) => <option value={id}>{name}</option>}</For>
          </select>
        </label>
        <Show when={runner() === "lm_studio"}>
          <p class="muted">The app uses the model that is already loaded in LM Studio. It never loads or switches a model.</p>
          {text("lm_studio_base_url", "Address")}
          {model("lm_studio_model", "Preferred model (optional, when several are loaded)")}
          {text("lm_studio_context_limit", "Context limit", "number")}
          {text("lm_studio_output_limit", "Reply limit", "number")}
        </Show>
        <Show when={runner() === "own_server"}>
          {text("own_server_url", "Address")}
          {text("own_server_api_key", "API key", "password")}
          {model("own_server_model", "Model")}
        </Show>
        <Show when={["openai", "anthropic", "google"].includes(runner())}>
          {text("api_key", "API key", "password")}
          {model("api_model", "Model")}
        </Show>
        <Show when={lookup()}>{(l) => <p classList={{ good: l().ok, bad: !l().ok }}>{l().text}</p>}</Show>
        <div class="row wrap">
          <button class="primary" disabled={!Object.keys(changes()).length} onClick={() => void save()}>Save</button>
          <button onClick={() => void test()} disabled={Object.keys(changes()).length > 0} title={Object.keys(changes()).length ? "Save first" : ""}>Test</button>
          <Show when={result()}>{(r) => <span classList={{ good: r().ok, bad: !r().ok }}>{r().text}</span>}</Show>
        </div>
      </Show>
    </fieldset>
  );
}
