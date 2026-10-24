import { createEffect, createSignal, Show } from "solid-js";
import { fileUrl } from "../api";
import { FINISHED, fmtTime, jobs, playhead, project, selectedScene, setPlayhead } from "../store";

/** The preview of the selected scene and the song's scrub bar. The song plays here and moves the timeline's playhead. */
export default function Stage() {
  let audio: HTMLAudioElement | undefined;
  const [playing, setPlaying] = createSignal(false);
  const duration = () => Math.max(project()?.audio.duration ?? 0, ...(project()?.scenes.map((s) => s.end) ?? [0]));
  const scene = selectedScene;
  const running = () => jobs().find((j) => !FINISHED.includes(j.status));
  const status = () => {
    const job = running();
    return job ? `${job.type}: ${job.progress.stage} ${Math.round(job.progress.percent)}%` : "Render status: idle";
  };

  // A click on the timeline or a scene moves the song to that time.
  createEffect(() => {
    const t = playhead();
    if (audio && Math.abs(audio.currentTime - t) > 0.25) audio.currentTime = t;
  });

  const toggle = () => {
    if (!audio) return;
    if (audio.paused) void audio.play();
    else audio.pause();
  };

  return (
    <section class="stage">
      <div class="preview">
        <span class="chip" classList={{ busy: !!running() }}>{status()}</span>
        <Show
          when={scene()?.rendered_video}
          fallback={
            <Show
              when={scene()?.approved_image}
              fallback={<div class="empty">{scene() ? "This scene has no video or picture yet. Write its prompt and render it from the panel on the right." : "Choose a scene to see it here."}</div>}
            >
              <img class="still" src={fileUrl(scene()!.approved_image!.path, scene()!.approved_image!.mtime)} alt="Scene picture" />
            </Show>
          }
        >
          <video controls preload="metadata" src={fileUrl(scene()!.rendered_video!.path, scene()!.rendered_video!.mtime)} />
        </Show>
      </div>
      <div class="scrub">
        <Show when={project()?.audio.attached} fallback={<span class="muted">No song yet. Add it in the Audio tab.</span>}>
          <audio
            ref={audio}
            preload="metadata"
            src={fileUrl(project()!.audio.path)}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onTimeUpdate={() => playing() && audio && setPlayhead(audio.currentTime)}
          />
          <button class="small" onClick={toggle}>{playing() ? "Pause" : "Play"}</button>
          <span class="muted">Global audio scrub</span>
          <input type="range" min="0" max={duration()} step="0.01" value={playhead()} onInput={(e) => setPlayhead(Number(e.currentTarget.value))} />
          <span class="time">{fmtTime(playhead())} / {fmtTime(duration())}</span>
        </Show>
      </div>
    </section>
  );
}
