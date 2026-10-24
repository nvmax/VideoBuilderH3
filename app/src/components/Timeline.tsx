import { createEffect, createResource, createSignal, For, onCleanup, Show } from "solid-js";
import { encodePid, fileUrl, get } from "../api";
import { edit, fmtTime, playhead, project, projectId, say, selectedId, setPlayhead, setSelectedId } from "../store";

const WAVE_HEIGHT = 96;
const CLIP_TOP = 30;
const CLIP_HEIGHT = 66;

function Waveform(props: { width: number; top: number }) {
  // `width` is the length of the song in pixels, so the peaks line up with the scene clips.
  let canvas: HTMLCanvasElement | undefined;
  const [wave] = createResource(
    () => (project()?.audio.attached ? [projectId(), project()!.audio.path] : null),
    async () => get<{ peaks: number[]; duration: number }>(`/projects/${encodePid(projectId()!)}/audio/waveform?peaks=1600`),
  );
  createEffect(() => {
    const data = wave();
    if (!canvas || !data || !data.peaks.length) return;
    const width = Math.max(1, Math.floor(props.width));
    canvas.width = width;
    canvas.height = WAVE_HEIGHT;
    const g = canvas.getContext("2d")!;
    g.clearRect(0, 0, width, WAVE_HEIGHT);
    g.fillStyle = "#0e7490";
    const max = Math.max(...data.peaks, 0.001);
    for (let x = 0; x < width; x++) {
      const peak = data.peaks[Math.min(data.peaks.length - 1, Math.floor((x / width) * data.peaks.length))] / max;
      const h = Math.max(1, peak * (WAVE_HEIGHT - 8));
      g.fillRect(x, (WAVE_HEIGHT - h) / 2, 1, h);
    }
  });
  return <canvas ref={canvas} class="wave" style={{ top: `${props.top}px`, width: `${props.width}px`, height: `${WAVE_HEIGHT}px` }} />;
}

export default function Timeline() {
  const [zoom, setZoom] = createSignal(12); // pixels per second
  const [bulk, setBulk] = createSignal(false);
  const [bulkText, setBulkText] = createSignal("");
  const [drag, setDrag] = createSignal<{ id: string; end: number } | null>(null);

  const scenes = () => project()?.scenes ?? [];
  const total = () => Math.max(project()?.audio.duration ?? 0, ...scenes().map((s) => s.end), 10);
  const width = () => Math.ceil(total() * zoom()) + 60;
  const selected = () => scenes().find((s) => s.id === selectedId()) ?? null;

  const startResize = (e: PointerEvent, id: string, end: number) => {
    e.stopPropagation();
    e.preventDefault();
    const startX = e.clientX;
    const move = (m: PointerEvent) => setDrag({ id, end: Math.max(0.1, end + (m.clientX - startX) / zoom()) });
    const up = async () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const d = drag();
      setDrag(null);
      if (d && Math.abs(d.end - end) > 0.02) await edit("POST", `/scenes/${encodeURIComponent(id)}/resize`, { end_time: Math.round(d.end * 100) / 100 });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    onCleanup(() => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    });
  };

  const seek = (e: MouseEvent) => {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setPlayhead(Math.max(0, (e.clientX - rect.left) / zoom()));
  };

  const split = async () => {
    const scene = scenes().find((s) => playhead() > s.start + 0.05 && playhead() < s.end - 0.05);
    if (!scene) return say("Put the playhead inside a scene first: click the timeline.", true);
    await edit("POST", `/scenes/${encodeURIComponent(scene.id)}/split`, { at_time: Math.round(playhead() * 100) / 100 });
  };

  const applyBulk = async (action: "replace" | "append") => {
    const mode = /-->|\s-\s|\sto\s/.test(bulkText()) ? "ranges" : "durations";
    const done = await edit("POST", "/timeline/bulk", { text: bulkText(), mode, action }, "Timeline updated.");
    if (done) setBulk(false);
  };

  // A mark every second, a label every ten (every thirty when zoomed far out).
  const labelEvery = () => (zoom() < 6 ? 30 : 10);
  const labels = () => Array.from({ length: Math.floor(total() / labelEvery()) + 1 }, (_, i) => i * labelEvery());

  return (
    <section class="timeline">
      <div class="tltools">
        <strong>Timeline Tools</strong>
        <button class="small" onClick={() => void split()} title="Cut the scene under the playhead in two">Split at playhead</button>
        <button class="small" onClick={() => void edit("POST", "/timeline/snap", { scope: "all" }, "Scenes snapped to the beats.")}>Snap to beats</button>
        <span class="muted">Zoom</span>
        <button class="small" onClick={() => setZoom(Math.max(3, zoom() / 1.4))}>-</button>
        <input type="range" min="3" max="120" value={zoom()} onInput={(e) => setZoom(Number(e.currentTarget.value))} style={{ width: "110px", padding: 0 }} />
        <button class="small" onClick={() => setZoom(Math.min(120, zoom() * 1.4))}>+</button>
        <span class="stats">
          {scenes().length} scenes | Timeline {fmtTime(total())} | Audio {fmtTime(project()?.audio.duration ?? 0)}
        </span>
        <span class="spacer" />
        <span class="muted">Playhead {fmtTime(playhead())}</span>
      </div>
      <div class="tlbody">
        <div class="tlbuttons">
          <button onClick={() => setBulk(!bulk())}>Bulk</button>
          <button class="cyan" onClick={() => void edit("POST", "/scenes", { position: selected() ? "after" : "append", ref_scene_id: selected()?.id, duration: 4 })}>+ Segment</button>
          <button onClick={() => void split()}>Split</button>
          <button disabled={!selected()} onClick={() => selected() && void edit("POST", `/scenes/${encodeURIComponent(selected()!.id)}/merge`, { with_direction: "next" })}>Merge next</button>
          <button class="danger" disabled={!selected()} onClick={() => selected() && window.confirm(`Delete scene ${selected()!.number}? Its files move to the removed scenes folder.`) && void edit("DELETE", `/scenes/${encodeURIComponent(selected()!.id)}?ripple=true`)}>Delete</button>
          <button onClick={() => void edit("POST", "/timeline/close-gaps", {}, "Gaps closed.")}>Close gaps</button>
        </div>
        <div class="tlscroll">
          <div class="tltrack" style={{ width: `${width()}px`, height: `${CLIP_TOP + CLIP_HEIGHT + WAVE_HEIGHT + 8}px` }} onClick={seek}>
            <div class="ruler" style={{ "background-image": `repeating-linear-gradient(to right, #52525b 0, #52525b 1px, transparent 1px, transparent ${zoom()}px)`, "background-size": "100% 8px", "background-repeat": "no-repeat", "background-position": "0 18px" }}>
              <For each={labels()}>{(t) => <span style={{ left: `${t * zoom()}px` }}>{fmtTime(t)}</span>}</For>
            </div>
            <For each={scenes()}>
              {(scene) => {
                const end = () => (drag()?.id === scene.id ? drag()!.end : scene.end);
                const w = () => Math.max(6, (end() - scene.start) * zoom());
                const thumb = () => scene.video_thumbnail ?? scene.approved_image;
                return (
                  <div
                    class={`clip ${scene.status}`}
                    classList={{ selected: scene.id === selectedId() }}
                    style={{
                      left: `${scene.start * zoom()}px`,
                      width: `${w()}px`,
                      top: `${CLIP_TOP}px`,
                      "background-image": thumb() ? `url("${fileUrl(thumb()!.path, thumb()!.mtime)}")` : undefined,
                    }}
                    title={`${scene.number}. ${scene.lyrics}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      setSelectedId(scene.id);
                      setPlayhead(scene.start);
                    }}
                  >
                    <Show when={scene.rendered_video}>
                      <span class="tag v">V</span>
                    </Show>
                    <Show when={w() > 56}>
                      <span class="cap">
                        <b>Scene {scene.number}</b>
                        <Show when={w() > 120}>
                          {fmtTime(scene.start)} - {fmtTime(end())} | {(end() - scene.start).toFixed(2)}s
                        </Show>
                      </span>
                    </Show>
                    <i class="handle" onPointerDown={(e) => startResize(e, scene.id, scene.end)} />
                  </div>
                );
              }}
            </For>
            <Waveform width={(project()?.audio.duration ?? 0) * zoom()} top={CLIP_TOP + CLIP_HEIGHT + 4} />
            <div class="playhead" style={{ left: `${playhead() * zoom()}px`, height: "100%" }} />
          </div>
        </div>
      </div>
      <Show when={bulk()}>
        <div class="bulk">
          <p class="muted">One scene per line. Seconds each (`4 words for the scene`) or ranges (`0:10 to 0:18 words`). Words become the scene's lyrics.</p>
          <textarea rows="6" value={bulkText()} onInput={(e) => setBulkText(e.currentTarget.value)} placeholder={"4 first line\n4.5 second line\n3"} />
          <div class="row">
            <button class="primary" disabled={!bulkText().trim()} onClick={() => void applyBulk("replace")}>Replace timeline</button>
            <button disabled={!bulkText().trim()} onClick={() => void applyBulk("append")}>Add to the end</button>
            <button onClick={() => setBulk(false)}>Close</button>
          </div>
        </div>
      </Show>
    </section>
  );
}
