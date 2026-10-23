import { createResource, createSignal, For, Show } from "solid-js";
import { call, get } from "../api";
import { say } from "../store";

const RUNNERS: [string, string][] = [
  ["lm_studio", "LM Studio (the model that is loaded)"],
  ["own_server", "Own server (OpenAI compatible)"],
  ["openai", "OpenAI"],
  ["anthropic", "Anthropic"],
  ["google", "Google"],
];

export default function LlmSettings() {
  const [cfg, { refetch }] = createResource(() => get<any>("/llm/settings"));
  const [changes, setChanges] = createSignal<Record<string, unknown>>({});
  const [result, setResult] = createSignal<{ ok: boolean; text: string } | null>(null);
  const value = (key: string) => (key in changes() ? changes()[key] : cfg()?.[key]);
  const set = (key: string, v: unknown) => setChanges({ ...changes(), [key]: v });
  const runner = () => String(value("runner") ?? "lm_studio");

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

  return (
    <fieldset>
      <legend>LLM for writing prompts and story steps</legend>
      <Show when={cfg()}>
        <label class="setting">
          <span>Runner</span>
          <select value={runner()} onChange={(e) => set("runner", e.currentTarget.value)}>
            <For each={RUNNERS}>{([id, name]) => <option value={id}>{name}</option>}</For>
          </select>
        </label>
        <Show when={runner() === "lm_studio"}>
          <p class="muted">The app uses the model that is already loaded in LM Studio. It never loads or switches a model.</p>
          {text("lm_studio_base_url", "Address")}
          {text("lm_studio_context_limit", "Context limit", "number")}
          {text("lm_studio_output_limit", "Reply limit", "number")}
        </Show>
        <Show when={runner() === "own_server"}>
          {text("own_server_url", "Address")}
          {text("own_server_model", "Model name")}
          {text("own_server_api_key", "API key", "password")}
        </Show>
        <Show when={["openai", "anthropic", "google"].includes(runner())}>
          {text("api_model", "Model name")}
          {text("api_key", "API key", "password")}
        </Show>
        <div class="row wrap">
          <button class="primary" disabled={!Object.keys(changes()).length} onClick={() => void save()}>Save</button>
          <button onClick={() => void test()} disabled={Object.keys(changes()).length > 0} title={Object.keys(changes()).length ? "Save first" : ""}>Test</button>
          <Show when={result()}>{(r) => <span classList={{ good: r().ok, bad: !r().ok }}>{r().text}</span>}</Show>
        </div>
      </Show>
    </fieldset>
  );
}
