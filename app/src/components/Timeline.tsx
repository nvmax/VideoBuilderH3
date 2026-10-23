import { createEffect, createResource, createSignal, For, onCleanup, Show } from "solid-js";
import { call, encodePid, get, readFileAsBase64 } from "../api";
import { act, edit, playhead, project, projectId, say, selectedId, setPlayhead, setSelectedId } from "../store";

const HEIGHT = 84;

function Waveform(props: { width: number }) {
  // `width` is the length of the song in pixels, so the peaks line up with the scene blocks.
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
    canvas.height = HEIGHT;
    const g = canvas.getContext("2d")!;
    g.clearRect(0, 0, width, HEIGHT);
    g.fillStyle = "#4b5d7a";
    const max = Math.max(...data.peaks, 0.001);
    for (let x = 0; x < width; x++) {
      const peak = data.peaks[Math.min(data.peaks.length - 1, Math.floor((x / width) * data.peaks.length))] / max;
      const h = Math.max(1, peak * (HEIGHT - 8));
      g.fillRect(x, (HEIGHT - h) / 2, 1, h);
    }
  });
  return <canvas ref={canvas} class="wave" style={{ width: `${props.width}px`, height: `${HEIGHT}px` }} />;
}

export default function Timeline() {
  const [zoom, setZoom] = createSignal(40); // pixels per second
  const [bulk, setBulk] = createSignal(false);
  const [bulkText, setBulkText] = createSignal("");
  const [drag, setDrag] = createSignal<{ id: string; end: number } | null>(null);

  const total = () => Math.max(project()?.audio.duration ?? 0, ...(project()?.scenes.map((s) => s.end) ?? [0]), 10);
  const width = () => Math.ceil(total() * zoom()) + 40;
  const selected = () => project()?.scenes.find((s) => s.id === selectedId()) ?? null;

  const attachSong = async (file: File | undefined, path?: string) => {
    await act(async () => {
      const body = file ? { audio_data: await readFileAsBase64(file), audio_name: file.name } : { audio_path: path };
      await call("PUT", `/projects/${encodePid(projectId()!)}/audio`, body);
    }, "Song added.");
  };

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
    const scene = project()?.scenes.find((s) => playhead() > s.start + 0.05 && playhead() < s.end - 0.05);
    if (!scene) return say("Put the playhead inside a scene first: click the timeline.", true);
    await edit("POST", `/scenes/${encodeURIComponent(scene.id)}/split`, { at_time: Math.round(playhead() * 100) / 100 });
  };

  const applyBulk = async (action: "replace" | "append") => {
    const mode = /-->|\s-\s|\sto\s/.test(bulkText()) ? "ranges" : "durations";
    const done = await edit("POST", "/timeline/bulk", { text: bulkText(), mode, action }, "Timeline updated.");
    if (done) setBulk(false);
  };

  return (
    <section class="panel timeline">
      <div class="row wrap">
        <strong>Timeline</strong>
        <Show
          when={project()?.audio.attached}
          fallback={
            <label class="file">
              Add song
              <input type="file" accept="audio/*" onChange={(e) => void attachSong(e.currentTarget.files?.[0])} />
            </label>
          }
        >
          <span class="muted">{project()!.audio.path.split(/[\\/]/).pop()} · {project()!.audio.duration.toFixed(1)} s</span>
          <label class="file small">
            Replace song
            <input type="file" accept="audio/*" onChange={(e) => void attachSong(e.currentTarget.files?.[0])} />
          </label>
        </Show>
        <span class="spacer" />
        <button class="small" onClick={() => void split()}>Split at playhead ({playhead().toFixed(2)} s)</button>
        <button class="small" disabled={!selected()} onClick={() => selected() && void edit("POST", `/scenes/${encodeURIComponent(selected()!.id)}/merge`, { with_direction: "next" })}>Merge with next</button>
        <button class="small" onClick={() => void edit("POST", "/scenes", { position: selected() ? "after" : "append", ref_scene_id: selected()?.id, duration: 4 })}>Add scene</button>
        <button
          class="small danger"
          disabled={!selected()}
          onClick={() => selected() && window.confirm(`Delete scene ${selected()!.number}? Its files move to the removed scenes folder.`) && void edit("DELETE", `/scenes/${encodeURIComponent(selected()!.id)}?ripple=true`)}
        >
          Delete
        </button>
        <button class="small" onClick={() => void edit("POST", "/timeline/close-gaps", {}, "Gaps closed.")}>Close gaps</button>
        <button class="small" onClick={() => setBulk(!bulk())}>Bulk text</button>
        <label class="muted">
          Zoom <input type="range" min="10" max="200" value={zoom()} onInput={(e) => setZoom(Number(e.currentTarget.value))} />
        </label>
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

      <div class="scroll">
        <div class="track" style={{ width: `${width()}px`, height: `${HEIGHT + 40}px` }} onClick={seek}>
          <Waveform width={(project()?.audio.duration ?? 0) * zoom()} />
          <For each={project()?.scenes ?? []}>
            {(scene) => {
              const end = () => (drag()?.id === scene.id ? drag()!.end : scene.end);
              return (
                <div
                  class={`block ${scene.status}`}
                  classList={{ selected: scene.id === selectedId() }}
                  style={{ left: `${scene.start * zoom()}px`, width: `${Math.max(6, (end() - scene.start) * zoom())}px`, top: `${HEIGHT + 4}px` }}
                  title={`${scene.number}. ${scene.lyrics}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    setSelectedId(scene.id);
                    setPlayhead(scene.start);
                  }}
                >
                  <span>{scene.number}</span>
                  <small>{scene.lyrics.split("\n")[0]}</small>
                  <i class="handle" onPointerDown={(e) => startResize(e, scene.id, scene.end)} />
                </div>
              );
            }}
          </For>
          <div class="playhead" style={{ left: `${playhead() * zoom()}px`, height: `${HEIGHT + 40}px` }} />
        </div>
      </div>
      <p class="muted legend">
        <span class="swatch empty" /> empty <span class="swatch has_prompt" /> prompt <span class="swatch has_image" /> image <span class="swatch has_video" /> video. Drag a scene's right edge to resize it.
      </p>
    </section>
  );
}
