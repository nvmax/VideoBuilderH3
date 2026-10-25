// The app's own settings, which the pack's Builder does not have because ComfyUI keeps them: where ComfyUI runs, where its output and the projects are,
// and where ffmpeg is. Opened by the "App Settings" entry this adds to the Builder's Menu, and on its own the first time the app starts (or when
// ffmpeg cannot be found), with the same checks the setup of the agent API makes.

const API = "/api/v1";
const SKIPPED = "h3studio.setupSkipped";
const FIELDS = [
  { key: "comfy_url", label: "ComfyUI address", hint: "Where ComfyUI is running, for example http://127.0.0.1:8188.", pick: null },
  { key: "comfy_output_dir", label: "ComfyUI output folder", hint: "Leave empty to ask the running ComfyUI. The Video Builder keeps its projects here.", pick: "folder" },
  { key: "project_root", label: "Project folder", hint: "Where new projects are made when they are not in the ComfyUI output folder.", pick: "project_root" },
  { key: "ffmpeg_path", label: "ffmpeg", hint: "A name found on PATH, or the full path of ffmpeg.exe.", pick: "program" },
  { key: "ffprobe_path", label: "ffprobe", hint: "A name found on PATH, or the full path of ffprobe.exe.", pick: "program" },
];
const LEVEL = {
  ok: { mark: "✓", color: "#22c55e" },
  warn: { mark: "!", color: "#f59e0b" },
  fail: { mark: "✗", color: "#ef4444" },
  skip: { mark: "–", color: "#71717a" },
};

async function call(path, options = {}) {
  const reply = await fetch(`${API}${path}`, { headers: { "Content-Type": "application/json" }, ...options });
  const body = await reply.json().catch(() => ({}));
  if (!reply.ok || body.ok === false) throw new Error(body?.error?.message || body?.error || `The app answered ${reply.status}.`);
  return body.data;
}

export function element(tag, css, text) {
  const node = document.createElement(tag);
  if (css) node.style.cssText = css;
  if (text !== undefined) node.textContent = text;
  return node;
}

export function button(label, primary = false) {
  const node = element("button", `border:1px solid ${primary ? "#0891b2" : "#3f3f46"};border-radius:6px;background:${primary ? "#0e7490" : "#27272a"};color:#f4f4f5;padding:7px 12px;font-size:12px;font-weight:700;cursor:pointer;`, label);
  node.type = "button";
  return node;
}

function restartApp() {
  const internals = window.__TAURI_INTERNALS__;
  if (!internals?.invoke) return false;
  internals.invoke("restart_app").catch(() => {});
  return true;
}

// The ElevenLabs key is kept for every project, in the same place as the Builder's own settings, and used by any project that has no key of its own. It
// saves at once, with no restart. An agent can make speech with it through the app and never sees it.
function elevenLabsBlock() {
  const wrap = element("div", "margin:14px 0 10px;border-top:1px solid #1e293b;padding-top:12px;");
  wrap.append(element("div", "font-weight:800;color:#cbd5e1;", "ElevenLabs API key"));
  const row = element("div", "display:flex;gap:6px;margin-top:3px;flex-wrap:wrap;");
  const input = element("input", "flex:1;min-width:200px;box-sizing:border-box;border:1px solid #3f3f46;border-radius:6px;background:#18181b;color:#f4f4f5;padding:7px 9px;font-size:12px;");
  input.type = "password";
  input.autocomplete = "off";
  input.spellcheck = false;
  const save = button("Save key");
  const check = button("Test");
  const remove = button("Remove");
  row.append(input, save, check, remove);
  const hint = element("div", "color:#71717a;font-size:11px;margin-top:3px;", "For Speaking projects: voices and speech. It is saved at once and used by every project without a key of its own. AI agents can make speech with it through the app and never see it.");
  const state = element("div", "min-height:16px;margin-top:4px;font-size:12px;color:#94a3b8;");
  wrap.append(row, hint, state);
  const say = (text, color = "#94a3b8") => {
    state.textContent = text;
    state.style.color = color;
  };
  const show = (configured) => {
    input.placeholder = configured ? "A key is saved. Paste a new one to replace it." : "Paste your ElevenLabs API key";
    remove.style.display = configured ? "" : "none";
    say(configured ? "A key is saved." : "No key is saved.", configured ? "#86efac" : "#94a3b8");
  };
  show(false);
  call("/setup/elevenlabs").then((found) => show(Boolean(found.configured))).catch((error) => say(String(error?.message || error), "#fca5a5"));
  save.onclick = async () => {
    const key = input.value.trim();
    if (!key) return say("Paste a key first.", "#fbbf24");
    try {
      const found = await call("/setup/elevenlabs", { method: "PUT", body: JSON.stringify({ api_key: key }) });
      input.value = "";
      show(Boolean(found.configured));
    } catch (error) {
      say(String(error?.message || error), "#fca5a5");
    }
  };
  check.onclick = async () => {
    say("Testing...");
    try {
      await call("/setup/elevenlabs/test", { method: "POST", body: JSON.stringify({ api_key: input.value.trim() }) });
      say("Connected: ElevenLabs accepted the key.", "#86efac");
    } catch (error) {
      say(String(error?.message || error), "#fca5a5");
    }
  };
  remove.onclick = async () => {
    try {
      const found = await call("/setup/elevenlabs", { method: "PUT", body: JSON.stringify({ api_key: "" }) });
      input.value = "";
      show(Boolean(found.configured));
    } catch (error) {
      say(String(error?.message || error), "#fca5a5");
    }
  };
  return wrap;
}

export function openAppSettings({ welcome = false } = {}) {
  if (document.getElementById("h3-app-settings")) return;
  const shade = element("div", "position:fixed;inset:0;z-index:100010;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.6);");
  shade.id = "h3-app-settings";
  const panel = element("div", "width:min(720px,94vw);max-height:92vh;overflow:auto;box-sizing:border-box;border:1px solid #155e75;border-radius:10px;background:#0b1220;color:#e5e7eb;padding:18px;font:13px 'Segoe UI',Inter,Arial,sans-serif;box-shadow:0 18px 60px rgba(0,0,0,.6);");
  shade.append(panel);
  panel.append(element("div", "font-size:17px;font-weight:900;color:#f0fdfa;", welcome ? "Welcome to VRGDG H3 Studio" : "App Settings"));
  panel.append(element("div", "color:#94a3b8;margin:4px 0 14px;line-height:1.45;", welcome ? "Tell the app where ComfyUI runs and where its tools are. You can change this later in the Menu under App Settings." : "Where ComfyUI runs and where the app finds its folders and tools. Saved settings are used after the app restarts."));

  const inputs = {};
  for (const field of FIELDS) {
    const wrap = element("label", "display:flex;flex-direction:column;gap:3px;margin-bottom:10px;");
    wrap.append(element("span", "font-weight:800;color:#cbd5e1;", field.label));
    const row = element("div", "display:flex;gap:6px;");
    const input = element("input", "flex:1;min-width:0;box-sizing:border-box;border:1px solid #3f3f46;border-radius:6px;background:#18181b;color:#f4f4f5;padding:7px 9px;font-size:12px;");
    input.type = "text";
    input.spellcheck = false;
    inputs[field.key] = input;
    row.append(input);
    if (field.pick) {
      const browse = button("Browse…");
      browse.onclick = async (event) => {
        event.preventDefault();
        try {
          const found = await fetch("/vrgdg/music_builder/pick_path", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ kind: field.pick }) }).then((r) => r.json());
          if (found.path) input.value = found.path;
        } catch {
          // The dialog is optional. The path can be typed.
        }
      };
      row.append(browse);
    }
    wrap.append(row, element("span", "color:#71717a;font-size:11px;", field.hint));
    panel.append(wrap);
  }
  panel.append(elevenLabsBlock());
  const suggestion = element("div", "display:none;margin:-2px 0 10px;color:#94a3b8;font-size:12px;");
  panel.append(suggestion);

  const results = element("div", "margin:6px 0 12px;border:1px solid #1e293b;border-radius:8px;padding:8px 10px;display:none;");
  panel.append(results);
  const status = element("div", "min-height:18px;margin-bottom:10px;color:#94a3b8;");
  panel.append(status);

  function showChecks(report) {
    results.replaceChildren();
    results.style.display = report.checks?.length ? "block" : "none";
    for (const item of report.checks || []) {
      const level = LEVEL[item.level] || LEVEL.skip;
      const line = element("div", "display:flex;gap:8px;padding:3px 0;line-height:1.4;");
      line.append(element("span", `flex:0 0 14px;font-weight:900;color:${level.color};`, level.mark));
      const text = element("div", "min-width:0;");
      text.append(element("span", "font-weight:800;", `${item.label} `), element("span", "color:#94a3b8;overflow-wrap:anywhere;", item.detail || ""));
      if (item.fix && item.level !== "ok") text.append(element("div", "color:#fbbf24;font-size:12px;", item.fix));
      line.append(text);
      results.append(line);
    }
    if (report.suggested_project_root && report.suggested_project_root !== inputs.comfy_output_dir.value.trim()) {
      suggestion.style.display = "block";
      suggestion.replaceChildren(document.createTextNode(`ComfyUI says its output folder is ${report.suggested_project_root}. `));
      const use = button("Use it");
      use.onclick = () => {
        inputs.comfy_output_dir.value = report.suggested_project_root;
        suggestion.style.display = "none";
      };
      suggestion.append(use);
    }
  }

  const current = () => Object.fromEntries(FIELDS.map((field) => [field.key, inputs[field.key].value.trim()]).filter(([key, value]) => value || key === "comfy_output_dir" || key.startsWith("ff")));

  const buttons = element("div", "display:flex;gap:8px;justify-content:flex-end;flex-wrap:wrap;");
  const checkButton = button("Check");
  const saveButton = button("Save and restart", true);
  const closeButton = button(welcome ? "Skip for now" : "Close");
  buttons.append(checkButton, saveButton, closeButton);
  panel.append(buttons);

  const close = () => shade.remove();
  closeButton.onclick = () => {
    if (welcome) {
      try {
        localStorage.setItem(SKIPPED, "1");
      } catch {
        // Not remembered: the welcome shows again next time.
      }
    }
    close();
  };
  checkButton.onclick = async () => {
    status.textContent = "Checking…";
    try {
      showChecks(await call("/setup/check", { method: "POST", body: JSON.stringify(current()) }));
      status.textContent = "";
    } catch (error) {
      status.textContent = String(error?.message || error);
    }
  };
  saveButton.onclick = async () => {
    status.textContent = "Saving…";
    try {
      await call("/setup/config", { method: "PUT", body: JSON.stringify(current()) });
      status.textContent = restartApp() ? "Saved. The app restarts." : "Saved. Close and open the app again to use the new settings.";
    } catch (error) {
      status.textContent = String(error?.message || error);
    }
  };
  shade.addEventListener("keydown", (event) => event.key === "Escape" && closeButton.click());
  document.body.append(shade);

  call("/setup/status").then((report) => {
    for (const field of FIELDS) inputs[field.key].value = report.config?.[field.key] ?? "";
    showChecks(report);
  }).catch((error) => {
    status.textContent = String(error?.message || error);
  });
}

// Puts "App Settings" in the Builder's Menu, above its own "Settings". The Menu is built by the Builder's script, so a watcher adds the entry when it appears.
function addMenuEntry(root) {
  for (const found of root.querySelectorAll ? root.querySelectorAll("button") : []) {
    if (found.textContent.trim() !== "Settings" || found.dataset.h3AppSettings) continue;
    const menu = found.parentElement;
    if (!menu || ![...menu.children].some((child) => /New Project/.test(child.textContent || ""))) continue;
    const entry = found.cloneNode(true);
    entry.textContent = "App Settings";
    entry.dataset.h3AppSettings = "1";
    entry.title = "Where ComfyUI runs and where the app finds its folders and tools.";
    entry.onclick = () => openAppSettings();
    found.dataset.h3AppSettings = "1";
    menu.insertBefore(entry, found);
    // MCP Setup sits right under it: how to connect an AI agent to this app.
    const mcp = found.cloneNode(true);
    mcp.textContent = "MCP Setup";
    mcp.dataset.h3AppSettings = "1";
    mcp.title = "Connect an AI agent (Claude, Codex, Cursor, LM Studio and others) to the MCP server built into this app.";
    mcp.onclick = () => import("/host/mcp_setup.js").then((module) => module.openMcpSetup());
    menu.insertBefore(mcp, found);
    // Updates are installed by the app window, so a browser tab that shows the app has no entry for them.
    if (window.__TAURI_INTERNALS__?.invoke) {
      const updates = found.cloneNode(true);
      updates.textContent = "Check for Updates";
      updates.dataset.h3AppSettings = "1";
      updates.title = "Look for a newer version of this app and install it.";
      updates.onclick = () => import("/host/updates.js").then((module) => module.openUpdates());
      menu.insertBefore(updates, found);
    }
  }
}

// The first time the app starts, or when ffmpeg cannot be found, the settings open on their own.
async function welcomeIfNeeded() {
  try {
    if (localStorage.getItem(SKIPPED)) return;
  } catch {
    // Storage is not available: ask anyway.
  }
  try {
    const report = await call("/setup/status");
    const noFfmpeg = (report.checks || []).some((item) => item.id === "ffmpeg" && item.level === "fail");
    if (report.first_run || noFfmpeg) openAppSettings({ welcome: true });
  } catch {
    // Without an answer there is nothing to show.
  }
}

export function startAppSettings() {
  addMenuEntry(document);
  new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node.nodeType === 1) addMenuEntry(node);
      }
    }
  }).observe(document.documentElement, { childList: true, subtree: true });
  welcomeIfNeeded();
}
