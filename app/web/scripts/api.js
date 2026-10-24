// Stand-in for ComfyUI's `scripts/api.js`. The Video Builder's front end (copied unchanged from the pack) imports it the
// way it does inside ComfyUI. Here the calls go to the H3 Studio server, which answers the pack's /vrgdg routes itself
// and forwards the few ComfyUI routes the Builder uses (/prompt, /history, /queue, /view, /interrupt) to ComfyUI.

const fromQuery = new URLSearchParams(location.search).get("api");

function resolveRoot() {
  if (window.__H3_API_ROOT__) return window.__H3_API_ROOT__;
  if (fromQuery) return fromQuery.replace(/\/api\/v1\/?$/, "").replace(/\/$/, "");
  return "http://127.0.0.1:8190";
}

class StudioApi extends EventTarget {
  constructor() {
    super();
    this.clientId = (crypto.randomUUID && crypto.randomUUID()) || String(Math.random()).slice(2);
    this.user = "";
    this.api_base = "";
    this.api_host = "";
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

  fetchApi(route, options = {}) {
    const headers = { ...(options.headers || {}) };
    return fetch(this.apiURL(route), { ...options, headers });
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
export default api;
