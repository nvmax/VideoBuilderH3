import { Match, Show, Switch, type JSX } from "solid-js";
import { call, encodePid } from "../api";
import { act, closeProject, jobs, FINISHED, overlay, project, projectId, say, setJobs, setOverlay, stopAll, type Overlay } from "../store";
import Health from "./Health";
import LlmSettings from "./LlmSettings";
import Queue from "./Queue";
import References from "./References";
import SceneList from "./SceneList";
import SettingsPanel from "./SettingsPanel";
import SidePanel from "./SidePanel";
import Setup from "./Setup";
import Stage from "./Stage";
import Timeline from "./Timeline";

const TITLES: Record<Overlay, string> = {
  refs: "Ref Builder: characters and locations",
  queue: "Render queue and final videos",
  llm: "LLM Runner",
  settings: "All MiniMax H3 settings",
  setup: "Setup",
};

function ToolButton(props: { children: JSX.Element; onClick: () => void; cyan?: boolean; title?: string; disabled?: boolean }) {
  return (
    <button class="tb" classList={{ cyan: props.cyan }} title={props.title} disabled={props.disabled} onClick={props.onClick}>
      {props.children}
    </button>
  );
}

/** The Video Builder's layout: a button bar on top, scenes on the left, the preview in the middle, settings on the right
 * and the timeline along the bottom. The Reference Builder, queue, LLM and settings open over the middle. */
export default function Editor() {
  const running = () => jobs().some((j) => !FINISHED.includes(j.status));
  const autoBuild = () => {
    if (!window.confirm("Auto Build makes images and prompts where they are missing, renders every scene without a video and stitches the final video. It uses the GPU for a long time. Start it?")) return;
    void act(async () => {
      const r = await call("POST", `/projects/${encodePid(projectId()!)}/pipelines/build-full-video`, {});
      setJobs((list) => [r.data.job, ...list]);
      say("Auto Build started. Watch it in the render queue.");
    });
  };
  const toggle = (name: Overlay) => setOverlay(overlay() === name ? null : name);

  return (
    <div class="builder">
      <header class="bar">
        <div class="group">
          <ToolButton onClick={closeProject} title="Back to the project list">Menu</ToolButton>
          <div>
            <span class="label">Project</span>
            <strong>{project()?.name}</strong>
          </div>
          <div>
            <span class="label">Video Type</span>
            <span>Singing (music video)</span>
          </div>
        </div>
        <span class="spacer" />
        <div class="group">
          <ToolButton cyan onClick={autoBuild} title="Images, prompts, videos and the final stitch for the whole project">Auto<br />Build</ToolButton>
          <ToolButton onClick={() => toggle("refs")}>Ref<br />Builder</ToolButton>
          <ToolButton onClick={() => toggle("llm")}>LLM<br />Runner</ToolButton>
          <ToolButton onClick={() => toggle("queue")}>Render<br />Queue</ToolButton>
          <ToolButton onClick={() => toggle("settings")}>All<br />Settings</ToolButton>
        </div>
        <span class="spacer" />
        <div class="group">
          <Health />
          <span class="pill">MiniMax H3</span>
          <button class="tb stop" disabled={!running()} onClick={() => void stopAll()}>Stop</button>
          <ToolButton onClick={() => toggle("setup")}>Setup</ToolButton>
        </div>
      </header>
      <div class="main">
        <SceneList />
        <Stage />
        <SidePanel />
        <Show when={overlay()}>
          {(name) => (
            <div class="overlay">
              <div class="head">
                <h2>{TITLES[name()]}</h2>
                <span class="spacer" />
                <button onClick={() => setOverlay(null)}>Close</button>
              </div>
              <Switch>
                <Match when={name() === "refs"}>
                  <References />
                </Match>
                <Match when={name() === "queue"}>
                  <Queue />
                </Match>
                <Match when={name() === "llm"}>
                  <LlmSettings />
                </Match>
                <Match when={name() === "settings"}>
                  <SettingsPanel />
                </Match>
                <Match when={name() === "setup"}>
                  <Setup onClose={() => setOverlay(null)} />
                </Match>
              </Switch>
            </div>
          )}
        </Show>
      </div>
      <Timeline />
    </div>
  );
}
