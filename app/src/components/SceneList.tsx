import { createResource, createSignal, For, Show } from "solid-js";
import { call, encodePid, fileUrl, get } from "../api";
import { act, edit, fmtTime, project, projectId, say, selectedId, setPlayhead, setSelectedId, setOverlay, submitJob, writeScenePrompt, renderSelectedScene } from "../store";

type LeftTab = "Scenes" | "Tools" | "Post Process";

function Cards() {
  const [options, setOptions] = createSignal<string | null>(null);
  const select = (id: string, start: number) => {
    setSelectedId(id);
    setPlayhead(start);
  };
  return (
    <div class="scenelist">
      <Show when={!project()?.scenes.length}>
        <p class="muted">No scenes yet. Add the song, then make scenes from the lyrics or add them by hand on the timeline.</p>
      </Show>
      <For each={project()?.scenes ?? []}>
        {(scene) => {
          const thumb = () => scene.video_thumbnail ?? scene.approved_image;
          const flag = (on: boolean) => (on ? "badge ok" : "badge off");
          return (
            <div class="scene-card" classList={{ selected: scene.id === selectedId() }} onClick={() => select(scene.id, scene.start)}>
              <div class="t">{scene.number}. {scene.label || `Scene ${scene.number}`}</div>
              <div class="m">Duration in seconds: {scene.duration.toFixed(2)}</div>
              <div class="m dim">{fmtTime(scene.start)} - {fmtTime(scene.end)}</div>
              <div class="badges">
                <span class={flag(!!scene.t2i_prompt)}>T2I {scene.t2i_prompt ? "OK" : "--"}</span>
                <span class={flag(!!scene.minimax_h3_prompt)}>H3 {scene.minimax_h3_prompt ? "OK" : "--"}</span>
                <span class={flag(!!scene.rendered_video)}>VID {scene.rendered_video ? "OK" : "--"}</span>
                <span class={flag(!!scene.approved_image)}>IMG {scene.approved_image ? "OK" : "--"}</span>
              </div>
              <div class="thumb">
                <Show when={thumb()} fallback={<span>No picture yet</span>}>
                  <img loading="lazy" src={fileUrl(thumb()!.path, thumb()!.mtime)} alt="" />
                </Show>
              </div>
              <button
                class="opt"
                onClick={(e) => {
                  e.stopPropagation();
                  select(scene.id, scene.start);
                  setOptions(options() === scene.id ? null : scene.id);
                }}
              >
                Options
              </button>
              <Show when={options() === scene.id}>
                <div class="optmenu" onClick={(e) => e.stopPropagation()}>
                  <button disabled={!scene.minimax_h3_prompt} onClick={() => void renderSelectedScene()}>Render this scene</button>
                  <button onClick={() => void writeScenePrompt(scene.id)}>Write the H3 prompt (LLM)</button>
                  <button onClick={() => void submitJob(`/scenes/${encodeURIComponent(scene.id)}/prompts/image`, {}, "Writing the image prompt.")}>Write the image prompt (LLM)</button>
                  <button
                    class="danger"
                    onClick={() => window.confirm(`Delete scene ${scene.number}? Its files move to the removed scenes folder.`) && void edit("DELETE", `/scenes/${encodeURIComponent(scene.id)}?ripple=true`)}
                  >
                    Delete scene
                  </button>
                </div>
              </Show>
            </div>
          );
        }}
      </For>
    </div>
  );
}

function Tools() {
  const scenes = () => project()?.scenes ?? [];
  const exportZip = async () => {
    const r = await call<{ zip_path: string; filename: string; bytes: number }>("POST", `/projects/${encodePid(projectId()!)}/export`, {});
    say(`Exported ${r.data.filename} (${(r.data.bytes / 1e6).toFixed(1)} MB) to ${r.data.zip_path}`);
  };
  return (
    <div class="toolslist">
      <button onClick={() => void edit("POST", "/scenes", { position: selectedId() ? "after" : "append", ref_scene_id: selectedId(), duration: 4 })}>Add a scene after the selected one</button>
      <button onClick={() => void edit("POST", "/timeline/close-gaps", {}, "Gaps closed.")}>Close gaps between scenes</button>
      <button onClick={() => void edit("POST", "/timeline/snap", { scope: "all" }, "Scenes snapped to the beats.")}>Snap scenes to the beats</button>
      <button
        onClick={() => {
          if (scenes().length > 0 && !window.confirm("Replace the current scenes with scenes made from the lyrics?")) return;
          void submitJob("/timeline/from-lines", { replace_existing: scenes().length > 0 }, "Timing the lyrics and making scenes. This can take a few minutes.");
        }}
      >
        Make scenes from the lyrics
      </button>
      <button onClick={() => void submitJob("/minimax-prompts", {}, "Writing prompts for the scenes that have none.")}>Write every missing H3 prompt (LLM)</button>
      <button onClick={() => void submitJob("/prompts/batch", { kind: "image", run_mode: "resume_missing" }, "Writing image prompts for the scenes that have none.")}>Write every missing image prompt (LLM)</button>
      <button onClick={() => setOverlay("queue")}>Render all, stitch, final videos</button>
      <button onClick={() => void act(exportZip)}>Export the project as a zip</button>
    </div>
  );
}

function PostProcess() {
  const [luts] = createResource(() => get<{ luts: { name: string; label: string }[] }>("/post/luts").catch(() => ({ luts: [] })));
  const [lut, setLut] = createSignal("");
  const chosen = () => lut() || luts()?.luts[0]?.name || "";
  const scene = () => project()?.scenes.find((s) => s.id === selectedId());
  return (
    <div class="toolslist">
      <p class="muted">Color grade the rendered clips with a LUT (.cube). The original clip is kept as a backup.</p>
      <Show when={luts()?.luts.length} fallback={<p class="muted">No LUTs yet. Add .cube files to the LUT folder.</p>}>
        <select value={chosen()} onChange={(e) => setLut(e.currentTarget.value)}>
          <For each={luts()?.luts}>{(l) => <option value={l.name}>{l.label}</option>}</For>
        </select>
        <button disabled={!scene()?.rendered_video} onClick={() => void submitJob(`/scenes/${encodeURIComponent(selectedId()!)}/post/lut`, { lut_name: chosen() }, "Applying the LUT to the selected scene.")}>
          Apply to the selected scene
        </button>
        <button onClick={() => window.confirm("Apply the LUT to every rendered scene?") && void submitJob("/post/apply-all", { stack: { lut: { lut_name: chosen() } } }, "Applying the LUT to every rendered scene.")}>
          Apply to every rendered scene
        </button>
      </Show>
    </div>
  );
}

export default function SceneList() {
  const [tab, setTab] = createSignal<LeftTab>("Scenes");
  return (
    <aside class="scenes">
      <div class="lefttabs">
        <For each={["Scenes", "Tools", "Post Process"] as LeftTab[]}>
          {(name) => (
            <button classList={{ active: tab() === name }} onClick={() => setTab(name)}>
              {name}
            </button>
          )}
        </For>
      </div>
      <Show when={tab() === "Scenes"}>
        <Cards />
      </Show>
      <Show when={tab() === "Tools"}>
        <Tools />
      </Show>
      <Show when={tab() === "Post Process"}>
        <PostProcess />
      </Show>
    </aside>
  );
}
