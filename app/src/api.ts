// The one place the UI talks to the backend: the app's own /api/v1, the same one agents use.

export interface ApiFailure {
  code: string;
  message: string;
  details: Record<string, unknown>;
  retryable: boolean;
}

export class ApiError extends Error {
  code: string;
  status: number;
  details: Record<string, unknown>;
  constructor(status: number, failure: ApiFailure) {
    super(failure.message);
    this.status = status;
    this.code = failure.code;
    this.details = failure.details ?? {};
  }
}

let base = new URLSearchParams(location.search).get("api") ?? "http://127.0.0.1:8190/api/v1";

/** Inside the desktop app the address comes from the backend. In a browser it is the default or `?api=`. */
export async function initApi(): Promise<string> {
  if ("__TAURI_INTERNALS__" in window) {
    const { invoke } = await import("@tauri-apps/api/core");
    base = await invoke<string>("api_base");
  }
  return base;
}

export const apiBase = () => base;

export interface Reply<T> {
  data: T;
  revision?: number;
}

export async function call<T = any>(method: string, path: string, body?: unknown, revision?: number): Promise<Reply<T>> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (revision !== undefined) headers["If-Match"] = String(revision);
  let response: Response;
  try {
    response = await fetch(base + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  } catch {
    throw new ApiError(0, { code: "API_UNREACHABLE", message: `The app's API is not reachable at ${base}.`, details: {}, retryable: true });
  }
  const json = await response.json().catch(() => null);
  if (!json || json.ok === false) {
    throw new ApiError(response.status, json?.error ?? { code: "BAD_RESPONSE", message: `The API answered ${response.status}.`, details: {}, retryable: false });
  }
  return { data: json.data as T, revision: json.revision };
}

export const get = <T = any>(path: string) => call<T>("GET", path).then((r) => r.data);

/** URL of a project file the UI can show: a video, an image or the song. */
export const fileUrl = (path: string, bust?: number | string) => `${base}/files?path=${encodeURIComponent(path)}${bust ? `&v=${bust}` : ""}`;

export type JobEvent = { name: string; data: any };

/** Job and queue events. Returns a function that stops listening. */
export function subscribe(onEvent: (event: JobEvent) => void): () => void {
  const source = new EventSource(`${base}/events`);
  for (const name of ["job.created", "job.updated", "queue.updated"]) {
    source.addEventListener(name, (e) => {
      try {
        onEvent({ name, data: JSON.parse((e as MessageEvent).data) });
      } catch {
        /* a malformed event is ignored */
      }
    });
  }
  return () => source.close();
}

export const encodePid = (pid: string) => encodeURIComponent(pid);

export function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1] ?? "");
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}
