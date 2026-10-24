import { createResource, createSignal, For, Show } from "solid-js";
import { get } from "../api";
import { edit, project } from "../store";

export interface Setting {
  key: string;
  type: "boolean" | "integer" | "number" | "string" | "array" | "object" | "null";
  default: unknown;
  choices?: string[];
  minimum?: number;
  maximum?: number;
}

const GROUPS: { title: string; keys: string[] }[] = [
  { title: "Quality", keys: ["render_pass", "resolution_preset", "aspect_ratio", "megapixels", "steps", "seed", "sampler_name", "scheduler"] },
  { title: "Models", keys: ["diffusion_model_name", "clip_name", "video_vae_name", "audio_vae_name"] },
  { title: "Continuity and audio", keys: ["continuity_mode", "latent_context_frames", "audio_mode", "video_mode"] },
  { title: "Speed", keys: ["sage_attention", "enable_fp16_accumulation", "use_turbo_lora", "use_loras"] },
];

const MODEL_SOURCE: Record<string, string> = {
  diffusion_model_name: "diffusion_models",
  clip_name: "text_encoders",
  video_vae_name: "vae",
  audio_vae_name: "vae",
};

function Control(props: { setting: Setting; value: unknown; models: Record<string, string[]>; onChange: (v: unknown) => void }) {
  const s = () => props.setting;
  const options = () => {
    const source = MODEL_SOURCE[s().key];
    if (source) {
      const list = props.models[source] ?? [];
      const current = String(props.value ?? "");
      return current && !list.includes(current) ? [current, ...list] : list;
    }
    return s().choices;
  };
  return (
    <>
      {s().type === "boolean" ? (
        <input type="checkbox" checked={Boolean(props.value)} onChange={(e) => props.onChange(e.currentTarget.checked)} />
      ) : options()?.length ? (
        <select value={String(props.value ?? "")} onChange={(e) => props.onChange(e.currentTarget.value)}>
          <For each={options()}>{(o) => <option value={o}>{o}</option>}</For>
        </select>
      ) : s().type === "integer" || s().type === "number" ? (
        <span class="row tight">
          <input
            type="number"
            step={s().type === "integer" ? 1 : "any"}
            min={s().minimum}
            max={s().maximum}
            value={Number(props.value ?? 0)}
            onChange={(e) => props.onChange(Number(e.currentTarget.value))}
          />
          <Show when={s().key === "seed"}>
            <button class="small" onClick={() => props.onChange(Math.floor(Math.random() * 2147483646) + 1)}>New</button>
          </Show>
        </span>
      ) : s().type === "string" ? (
        <input value={String(props.value ?? "")} onChange={(e) => props.onChange(e.currentTarget.value)} />
      ) : (
        <textarea
          rows="3"
          value={JSON.stringify(props.value ?? s().default)}
          onChange={(e) => {
            try {
              props.onChange(JSON.parse(e.currentTarget.value));
            } catch {
              /* not valid JSON yet: keep what is saved */
            }
          }}
        />
      )}
    </>
  );
}

// The schema and the model lists are read once and shared by every place that shows settings.
let schemaCache: Promise<{ settings: Setting[] }> | undefined;
let modelsCache: Promise<Record<string, string[]>> | undefined;
const loadSchema = () => (schemaCache ??= get<{ settings: Setting[] }>("/settings/minimax-h3/schema").catch((e) => { schemaCache = undefined; throw e; }));
const loadModels = () => (modelsCache ??= get<Record<string, string[]>>("/models").catch(() => { modelsCache = undefined; return {} as Record<string, string[]>; }));

/** The settings schema, shared. */
export const useSchema = () => createResource(loadSchema);
const byKeyOf = (list: Setting[] | undefined) => Object.fromEntries((list ?? []).map((s) => [s.key, s]));

/** One row per setting key, saved at once when changed. */
export function SettingRows(props: { keys: string[] }) {
  const [schema] = createResource(loadSchema);
  const [models] = createResource(loadModels);
  const byKey = () => byKeyOf(schema()?.settings);
  const values = () => project()?.settings.minimax_h3 ?? {};
  const change = (key: string, value: unknown) => edit("PATCH", "/settings", { minimax_h3: { [key]: value } });
  return (
    <For each={props.keys}>
      {(key) => (
        <Show when={byKey()[key]}>
          <label class="setting">
            <span>{key.replace(/_/g, " ")}</span>
            <Control setting={byKey()[key]} value={values()[key]} models={models() ?? {}} onChange={(v) => void change(key, v)} />
          </label>
        </Show>
      )}
    </For>
  );
}

export const MODEL_KEYS = GROUPS[1].keys;
export const VIDEO_GROUPS = [GROUPS[0], GROUPS[2], GROUPS[3]];

export default function SettingsPanel() {
  const [schema] = createResource(loadSchema);
  const [models] = createResource(loadModels);
  const [all, setAll] = createSignal(false);

  return (
    <div class="settings">
      <p class="muted">These are the project's MiniMax H3 settings. A change is saved at once and applies to the next render. Scenes with their own settings keep them.</p>
      <Show when={models() && !models()?.diffusion_models?.length}>
        <p class="bad">ComfyUI did not list any models, so model names cannot be chosen. Start ComfyUI and reopen this tab.</p>
      </Show>
      <div class="groups">
        <For each={GROUPS}>
          {(g) => (
            <fieldset>
              <legend>{g.title}</legend>
              <SettingRows keys={g.keys} />
            </fieldset>
          )}
        </For>
      </div>
      <button class="small" onClick={() => setAll(!all())}>{all() ? "Hide" : "Show"} every setting ({schema()?.settings.length ?? 0})</button>
      <Show when={all()}>
        <fieldset class="all">
          <SettingRows keys={(schema()?.settings ?? []).map((s) => s.key)} />
        </fieldset>
      </Show>
    </div>
  );
}
