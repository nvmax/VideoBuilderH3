import { createResource, onCleanup, Show } from "solid-js";
import { apiBase, get } from "../api";

/** ComfyUI and ffmpeg status, read again every 15 seconds. */
export default function Health() {
  const [health, { refetch }] = createResource(() => get<any>("/health").catch(() => null));
  const timer = window.setInterval(() => void refetch(), 15000);
  onCleanup(() => window.clearInterval(timer));
  return (
    <span class="health" title={apiBase()}>
      <span class={health()?.comfy_up ? "dot ok" : "dot bad"} /> ComfyUI {health()?.comfy_up ? "connected" : "not reachable"}
      <Show when={health() && !health().ffmpeg_available}>
        <span class="bad"> · ffmpeg not found</span>
      </Show>
    </span>
  );
}
