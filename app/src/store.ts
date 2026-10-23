// Shared state of the editor. Everything the user sees comes from the API and is read again after each change.

import { createSignal } from "solid-js";
import { ApiError, call, encodePid, get, subscribe } from "./api";

export interface Asset {
  path: string;
  filename: string;
  mtime: number;
}

export interface Scene {
  id: string;
  number: number;
  start: number;
  end: number;
  duration: number;
  status: "empty" | "has_prompt" | "has_image" | "has_video";
  lyrics: string;
  story_beat: string;
  minimax_h3_prompt: string;
  t2i_prompt: string;
  approved_image: Asset | null;
  rendered_video: Asset | null;
  video_thumbnail: Asset | null;
  [key: string]: any;
}

export interface ProjectDetail {
  id: string;
  name: string;
  revision: number;
  scenes: Scene[];
  settings: { project: Record<string, any>; minimax_h3: Record<string, any> };
  audio: { attached: boolean; path: string; duration: number };
  [key: string]: any;
}

export interface Job {
  id: string;
  type: string;
  project_id: string | null;
  status: string;
  progress: { percent: number; stage: string; message: string; scene_id: string | null; scene_index: number; scene_count: number };
  result: any;
  error: { code: string; message: string } | null;
  created_at: number;
}

export const [projectId, setProjectId] = createSignal<string | null>(null);
export const [project, setProject] = createSignal<ProjectDetail | null>(null);
export const [selectedId, setSelectedId] = createSignal<string | null>(null);
export const [playhead, setPlayhead] = createSignal(0);
export const [jobs, setJobs] = createSignal<Job[]>([]);
export const [notice, setNotice] = createSignal<{ text: string; bad: boolean } | null>(null);
export const [busy, setBusy] = createSignal(false);

let noticeTimer: number | undefined;
export function say(text: string, bad = false) {
  setNotice({ text, bad });
  window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => setNotice(null), bad ? 8000 : 3000);
}

export const selectedScene = () => project()?.scenes.find((s) => s.id === selectedId()) ?? null;

export async function loadProject(pid = projectId()) {
  if (!pid) return;
  const detail = await get<ProjectDetail>(`/projects/${encodePid(pid)}`);
  setProject(detail);
  if (!detail.scenes.some((s) => s.id === selectedId())) setSelectedId(detail.scenes[0]?.id ?? null);
}

export async function loadJobs() {
  const pid = projectId();
  if (!pid) return setJobs([]);
  setJobs(await get<Job[]>(`/jobs?project_id=${encodePid(pid)}`));
}

export async function openProject(pid: string) {
  setProjectId(pid);
  setSelectedId(null);
  setPlayhead(0);
  await Promise.all([loadProject(pid), loadJobs()]);
}

export function closeProject() {
  setProjectId(null);
  setProject(null);
  setJobs([]);
}

let chain: Promise<unknown> = Promise.resolve();

/**
 * Run a change, then read the project again. Changes run one after another, so each one is sent with the revision the
 * one before it produced. A failure is shown and the project is read again as well.
 */
export function act<T>(work: () => Promise<T>, done?: string): Promise<T | undefined> {
  const run = chain.then(async () => {
    setBusy(true);
    try {
      const result = await work();
      if (done) say(done);
      return result;
    } catch (e) {
      say(e instanceof ApiError ? e.message : String(e), true);
    } finally {
      await loadProject().catch(() => undefined);
      setBusy(false);
    }
    return undefined;
  });
  chain = run.catch(() => undefined);
  return run;
}

/** `POST`, `PUT`, `PATCH` or `DELETE` on a project path, with the revision we last read. */
export function edit<T = any>(method: string, path: string, body?: unknown, done?: string) {
  const pid = projectId();
  if (!pid) return Promise.resolve(undefined);
  return act(() => call<T>(method, `/projects/${encodePid(pid)}${path}`, body, project()?.revision).then((r) => r.data), done);
}

let stop: (() => void) | undefined;

/** Keep the job list and the project in step with what the backend reports. */
export function startEvents() {
  stop?.();
  stop = subscribe(({ name, data }) => {
    if (name === "queue.updated") return;
    const job = data as Job;
    if (job.project_id !== projectId()) return;
    setJobs((list) => {
      const rest = list.filter((j) => j.id !== job.id);
      return [job, ...rest].sort((a, b) => b.created_at - a.created_at);
    });
    if (["succeeded", "failed", "cancelled", "interrupted"].includes(job.status)) {
      void loadProject();
      if (job.status === "failed") say(job.error?.message ?? "A job failed.", true);
    }
  });
}
