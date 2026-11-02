// Stand-in for ComfyUI's `scripts/api.js`. The Video Builder's front end (copied unchanged from the pack) imports it the
// way it does inside ComfyUI. Here the calls go to the H3 Studio server, which answers the pack's /vrgdg routes itself
// and forwards the few ComfyUI routes the Builder uses (/prompt, /history, /queue, /view, /interrupt) to ComfyUI.

const fromQuery = new URLSearchParams(location.search).get("api");

// What the page sends in place of the ComfyUI API key. The server (`COMFY_KEY_PLACEHOLDER` in vrgdg/mod.rs) swaps it for the saved key.
const COMFY_KEY_PLACEHOLDER = "h3-studio:saved-comfy-api-key";

function resolveRoot() {
  if (window.__H3_API_ROOT__) return window.__H3_API_ROOT__;
  if (fromQuery) return fromQuery.replace(/\/api\/v1\/?$/, "").replace(/\/$/, "");
  // The screens are served by the API, so the API is where the page came from.
  return location.protocol.startsWith("http") ? location.origin : "http://127.0.0.1:8190";
}

// A request body this large (a project save is a few MB of JSON) is sent gzipped: over a slow link the Builder waits for
// the upload, not for the server. The server unpacks it.
const GZIP_BODY_MIN = 256 * 1024;

async function gzipLargeBody(options) {
  const { body } = options;
  if (typeof body !== "string" || body.length < GZIP_BODY_MIN || typeof CompressionStream === "undefined") return options;
  const headers = new Headers(options.headers);
  if (headers.has("Content-Encoding")) return options;
  headers.set("Content-Encoding", "gzip");
  const packed = new Blob([body]).stream().pipeThrough(new CompressionStream("gzip"));
  return { ...options, headers, body: await new Response(packed).blob() };
}

class StudioApi extends EventTarget {
  constructor() {
    super();
    this.clientId = (crypto.randomUUID && crypto.randomUUID()) || String(Math.random()).slice(2);
    this.user = "";
    this.api_base = "";
    this.api_host = "";
    // The Builder reads these when it sends a job to a partner node (API Nodes video). ComfyUI's page signs in with an account. This app has no
    // such page, and it keeps the ComfyUI API key on its server: the page only gets a placeholder when a key is saved, and the server swaps it
    // for the key on the way to ComfyUI.
    this.authToken = undefined;
    this.apiKey = undefined;
  }

  // Ask the app whether a ComfyUI API key is saved. Run at start and after the key is saved or removed in App Settings.
  async refreshCredentials() {
    try {
      const reply = await fetch(this.apiURL("/api/v1/setup/comfy_api_key"));
      const body = await reply.json();
      this.apiKey = body?.data?.configured ? COMFY_KEY_PLACEHOLDER : undefined;
    } catch {
      this.apiKey = undefined;
    }
  }

  get root() {
    return resolveRoot();
  }

  apiURL(route) {
    return `${this.root}${route}`;
  }

  fileURL(route) {
    return `${this.root}${route}`;
  }

  async fetchApi(route, options = {}) {
    const headers = { ...(options.headers || {}) };
    return fetch(this.apiURL(route), await gzipLargeBody({ ...options, headers }));
  }

  // ComfyUI sends server events over a WebSocket. The H3 Studio server sends the same kind of events over one
  // event stream. Listeners added with addEventListener receive them as CustomEvent.detail.
  connectEvents() {
    if (this._source) return;
    try {
      this._source = new EventSource(`${this.root}/vrgdg/events`);
      this._source.onmessage = (message) => {
        try {
          const { type, data } = JSON.parse(message.data);
          this.dispatchEvent(new CustomEvent(type, { detail: data }));
        } catch {
          /* a malformed event is ignored */
        }
      };
    } catch {
      /* no event stream: the Builder still works, it just does not hear about changes made elsewhere */
    }
  }
}

export const api = new StudioApi();
api.connectEvents();
api.refreshCredentials();
export default api;
