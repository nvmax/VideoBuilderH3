// Starts the Video Builder (the pack's own front end, in /extensions/comfyui-vrgamedevgirl) as the app's one screen.
// ComfyUI normally opens it from a node button. Here a stand-in node keeps the few values the Builder keeps on its node
// (the project folder, the session file, the audio and SRT paths) in this browser's storage.

const EXT = "/extensions/comfyui-vrgamedevgirl";
const STORE = "h3studio.node.";

async function apiRoot() {
  const tauri = window.__TAURI_INTERNALS__;
  if (tauri?.invoke) {
    const base = await tauri.invoke("api_base");
    return String(base).replace(/\/api\/v1\/?$/, "");
  }
  return null; // api.js falls back to ?api= or the default address
}

function standInNode() {
  const widget = (name) => ({
    name,
    type: "text",
    get value() {
      try {
        return localStorage.getItem(STORE + name) ?? "";
      } catch {
        return "";
      }
    },
    set value(v) {
      try {
        localStorage.setItem(STORE + name, v ?? "");
      } catch {
        /* storage is not available: the value just is not remembered */
      }
    },
  });
  return {
    id: 1,
    type: "VRGDG_MusicVideoBuilderUI",
    comfyClass: "VRGDG_MusicVideoBuilderUI",
    widgets: ["audio_path", "project_folder", "session_path", "srt_path"].map(widget),
    widgets_values: [],
    setSize() {},
    addWidget() {
      return null;
    },
  };
}

function showFailure(error) {
  console.error("H3 Studio could not start the Builder:", error);
  const box = document.createElement("pre");
  box.style.cssText = "color:#fca5a5;background:#18181b;padding:16px;margin:16px;border:1px solid #7f1d1d;white-space:pre-wrap;font:12px Consolas,monospace";
  box.textContent = `The Builder could not start.\n\n${error?.stack || error}`;
  document.body.appendChild(box);
}

async function start() {
  const root = await apiRoot();
  if (root) window.__H3_API_ROOT__ = root;
  // The pack's top level scripts the Builder talks to (themes, the Storyboard opener, comparison preview, LUT picker).
  for (const file of ["VRGDG_UIThemes.js", "VRGDG_StoryboardBuilderUI.js", "VRGDG_OverlayTrack.js", "VRGDG_PostProcessComparePreview.js", "VRGDG_MusicVideoBuilderLUTs.js"]) {
    try {
      await import(/* @vite-ignore */ `${EXT}/${file}`);
    } catch (error) {
      console.warn(`H3 Studio: ${file} did not load`, error);
    }
  }
  const { openBuilder } = await import(/* @vite-ignore */ `${EXT}/music_video_builder/builder.mjs`);
  openBuilder(standInNode(), {});
}

start().catch(showFailure);
