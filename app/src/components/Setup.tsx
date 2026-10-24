import { createSignal, For, onMount, Show } from "solid-js";
import { call, get } from "../api";
import { say } from "../store";
import LlmSettings from "./LlmSettings";

interface Check {
  id: string;
  label: string;
  level: "ok" | "warn" | "fail" | "skip";
  detail: string;
  fix: string;
}

interface Report {
  first_run: boolean;
  ready: boolean;
  suggested_project_root: string | null;
  config: Record<string, string>;
  config_path: string;
  checks: Check[];
}

const FIELDS: [string, string, string][] = [
  ["comfy_url", "ComfyUI address", "http://127.0.0.1:8188"],
  ["project_root", "Project folder", "Where your projects are saved"],
  ["comfy_output_dir", "ComfyUI output folder (optional)", "Leave empty to ask ComfyUI"],
  ["ffmpeg_path", "ffmpeg (optional)", "Leave empty to use the one that ships with the app or is on PATH"],
  ["ffprobe_path", "ffprobe (optional)", "Leave empty to use the one next to ffmpeg"],
];

const MARK: Record<Check["level"], string> = { ok: "OK", warn: "!", fail: "X", skip: "-" };

/** First-run setup and the same screen later from the header: what is ready, what is not, and the paths to change. */
export default function Setup(props: { onClose: () => void }) {
  const [report, setReport] = createSignal<Report | null>(null);
  const [values, setValues] = createSignal<Record<string, string>>({});
  const [busy, setBusy] = createSignal(false);
  const [dirty, setDirty] = createSignal(false);

  const load = async () => {
    const data = await get<Report>("/setup/status");
    setReport(data);
    setValues({ ...data.config });
    // The Video Builder keeps its projects in ComfyUI's output folder. On the first run that folder is suggested, so
    // the projects already made there show up.
    if (data.first_run && data.suggested_project_root && data.config.project_root.includes("VRGDG-H3 Projects")) {
      setValues({ ...data.config, project_root: data.suggested_project_root });
      setDirty(true);
    }
  };
  onMount(() => void load().catch((e) => say((e as Error).message, true)));

  const set = (key: string, v: string) => {
    setValues({ ...values(), [key]: v });
    setDirty(true);
  };
  const sendable = () => Object.fromEntries(Object.entries(values()).filter(([k]) => FIELDS.some((f) => f[0] === k)));

  const checkNow = async () => {
    setBusy(true);
    try {
      const data = await call<Report>("POST", "/setup/check", sendable());
      setReport({ ...data.data, config: { ...values() } });
    } catch (e) {
      say((e as Error).message, true);
    } finally {
      setBusy(false);
    }
  };

  const saveAndRestart = async () => {
    setBusy(true);
    try {
      await call("PUT", "/setup/config", sendable());
      if ("__TAURI_INTERNALS__" in window) {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("restart_app");
      } else {
        say("Saved. Restart the app to use the new settings.");
        setDirty(false);
      }
    } catch (e) {
      say((e as Error).message, true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section class="panel narrow setup">
      <h2>{report()?.first_run ? "Welcome. Let's set up VRGDG H3 Studio" : "Setup"}</h2>
      <p class="muted">The app needs a running ComfyUI with the H3 nodes, a folder for projects and ffmpeg. Everything is checked below.</p>
      <For each={FIELDS}>
        {([key, label, hint]) => (
          <label class="setting">
            <span>{label}</span>
            <input placeholder={hint} value={values()[key] ?? ""} onInput={(e) => set(key, e.currentTarget.value)} />
          </label>
        )}
      </For>
      <div class="row wrap">
        <button disabled={busy()} onClick={() => void checkNow()}>
          Check
        </button>
        <button class="primary" disabled={busy() || !dirty()} onClick={() => void saveAndRestart()} title={dirty() ? "" : "Change a setting first"}>
          Save and restart
        </button>
        <button onClick={props.onClose}>{report()?.ready ? "Done" : "Close"}</button>
      </div>
      <Show when={report()}>
        {(r) => (
          <>
            <ul class="checks">
              <For each={r().checks}>
                {(c) => (
                  <li>
                    <span class={`mark ${c.level}`}>{MARK[c.level]}</span>
                    <div>
                      <strong>{c.label}</strong>
                      <div class="muted">{c.detail}</div>
                      <Show when={c.fix && c.level !== "ok"}>
                        <div class={c.level === "fail" ? "bad" : "warnnote"}>{c.fix}</div>
                      </Show>
                    </div>
                  </li>
                )}
              </For>
            </ul>
            <p class={r().ready ? "good" : "bad"}>{r().ready ? "Ready to make music videos." : "Fix the items marked X, then check again."}</p>
            <p class="muted">Settings file: {r().config_path}</p>
          </>
        )}
      </Show>
      <LlmSettings />
    </section>
  );
}
