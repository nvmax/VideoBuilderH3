import { createResource, createSignal, For, Show } from "solid-js";
import { call, encodePid, fileUrl, get } from "../api";
import { act, jobs, loadJobs, project, projectId, say, setJobs } from "../store";

const FINISHED = ["succeeded", "failed", "cancelled", "interrupted"];

function Finals() {
  const [finals, { refetch }] = createResource(
    () => [projectId(), jobs().filter((j) => j.type === "video.stitch" && j.status === "succeeded").length] as const,
    async ([pid]) => get<{ filename: string; path: string; bytes: number; mtime: number }[]>(`/projects/${encodePid(pid!)}/finals`),
  );
  const [shown, setShown] = createSignal<string | null>(null);
  const current = () => finals()?.find((f) => f.path === shown()) ?? finals()?.[0];
  return (
    <section class="panel">
      <div class="row">
        <h3>Final videos</h3>
        <button class="small" onClick={() => void refetch()}>Refresh</button>
      </div>
      <Show when={current()} fallback={<p class="muted">No final video yet. Render the scenes, then stitch them.</p>}>
        {(f) => (
          <>
            <video controls preload="metadata" src={fileUrl(f().path, f().mtime)} class="final" />
            <div class="row wrap">
              <For each={finals()}>
                {(item) => (
                  <button class="small" classList={{ active: item.path === f().path }} onClick={() => setShown(item.path)}>
                    {item.filename} ({(item.bytes / 1e6).toFixed(1)} MB)
                  </button>
                )}
              </For>
            </div>
          </>
        )}
      </Show>
    </section>
  );
}

export default function Queue() {
  const pid = () => encodePid(projectId()!);
  const scenes = () => project()?.scenes ?? [];
  const withPrompt = () => scenes().filter((s) => s.minimax_h3_prompt?.trim()).length;
  const withVideo = () => scenes().filter((s) => s.rendered_video).length;
  const running = () => jobs().some((j) => !FINISHED.includes(j.status) && j.type !== "image.generate");

  const submit = (path: string, body: unknown, done: string) =>
    act(async () => {
      const r = await call("POST", `/projects/${pid()}${path}`, body);
      setJobs((list) => [r.data.job, ...list]);
      say(done);
    });

  const cancel = (id: string) => act(async () => void (await call("POST", `/jobs/${id}/cancel`)).data).then(loadJobs);
  const retry = (id: string, resume: boolean) =>
    act(async () => {
      const r = await call("POST", `/jobs/${id}/retry`, { resume });
      setJobs((list) => [r.data, ...list]);
    });

  return (
    <div class="queue">
      <section class="panel">
        <div class="row wrap">
          <h3>Render</h3>
          <span class="muted">{withPrompt()} of {scenes().length} scenes have a prompt, {withVideo()} have a video.</span>
          <span class="spacer" />
          <button
            title="Times the song's lyrics with ComfyUI and builds the scenes from them"
            onClick={() => {
              if (scenes().length > 0 && !window.confirm("Replace the current scenes with scenes made from the lyrics?")) return;
              void submit("/timeline/from-lines", { replace_existing: scenes().length > 0 }, "Timing the lyrics and making scenes. This can take a few minutes.");
            }}
          >
            Make scenes from lyrics
          </button>
          <button disabled={withPrompt() === scenes().length} onClick={() => void submit("/minimax-prompts", {}, "Writing prompts for the scenes that have none.")}>
            Write missing prompts
          </button>
          <button class="primary" disabled={!withPrompt()} onClick={() => void submit("/video/render", { scope: "missing", skip_final_stitch: true }, "Rendering the scenes without a video.")}>
            Render scenes without a video
          </button>
          <button disabled={!withPrompt()} onClick={() => window.confirm("Render every scene again? Existing clips are kept as backups.") && void submit("/video/render", { scope: "all", skip_final_stitch: true }, "Rendering every scene.")}>
            Render all again
          </button>
          <button class="primary" disabled={!withVideo()} onClick={() => void submit("/stitch", {}, "Stitching the final video.")}>
            Stitch final video
          </button>
        </div>
        <Show when={running()}>
          <p class="muted">One render runs at a time. The rest wait their turn.</p>
        </Show>
      </section>

      <section class="panel">
        <h3>Jobs</h3>
        <For each={jobs()} fallback={<p class="muted">Nothing has run yet.</p>}>
          {(job) => (
            <div class="job">
              <div class="row wrap">
                <strong>{job.type}</strong>
                <span classList={{ muted: true, good: job.status === "succeeded", bad: job.status === "failed" }}>{job.status}</span>
                <span class="muted">{job.progress.stage}</span>
                <span class="spacer" />
                <Show when={!FINISHED.includes(job.status)}>
                  <button class="small" onClick={() => void cancel(job.id)}>Cancel</button>
                </Show>
                <Show when={job.status === "failed" || job.status === "interrupted"}>
                  <button class="small" onClick={() => void retry(job.id, true)}>Resume</button>
                  <button class="small" onClick={() => void retry(job.id, false)}>Retry all</button>
                </Show>
              </div>
              <Show when={!FINISHED.includes(job.status)}>
                <progress max="100" value={job.progress.percent} />
              </Show>
              <p class="muted">{job.error ? job.error.message : job.progress.message}</p>
            </div>
          )}
        </For>
      </section>
      <Finals />
    </div>
  );
}
