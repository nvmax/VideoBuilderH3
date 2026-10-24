// Follow the agent. While an AI agent works on a project through the MCP server or the API, this panel lists what it does as it happens (a tool it calls,
// a job it starts and how far the job is), and with "Follow" on, the Builder goes where the agent goes: it opens the project the agent works in and selects the
// scene the agent is editing, scrolling to it and flashing it. The edits themselves appear in the open project on their own (the Builder's own merge of
// `vrgdg.project_changed`, see music_video_builder/external_changes.mjs), so what you see is the real project, not a replay.
//
// The server sends the entries over the event stream as `vrgdg.agent_activity` and keeps the last ones for a window that opens late.

import { api } from "/scripts/api.js";

const API = "/api/v1";
const FOLLOW = "h3studio.agent.follow";
const PANEL = "h3studio.agent.panel";
const SHOWN = 400;
const STYLE_ID = "h3-agent-style";

const DOT = { running: "#f59e0b", done: "#22c55e", failed: "#ef4444", cancelled: "#71717a" };

function remembered(key, fallback) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function remember(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not remembered: the next start uses the default.
  }
}

function element(tag, css, text) {
  const node = document.createElement(tag);
  if (css) node.style.cssText = css;
  if (text !== undefined) node.textContent = text;
  return node;
}

function addStyle() {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement("style");
  style.id = STYLE_ID;
  style.textContent = `
    @keyframes h3-agent-flash { 0% { box-shadow: 0 0 0 0 rgba(34,211,238,.95); } 100% { box-shadow: 0 0 0 16px rgba(34,211,238,0); } }
    .h3-agent-flash { animation: h3-agent-flash 1.3s ease-out 1; }
    @keyframes h3-agent-pulse { 0%, 100% { opacity: 1; } 50% { opacity: .35; } }
    .h3-agent-running { animation: h3-agent-pulse 1s ease-in-out infinite; }
  `;
  document.head.append(style);
}

export function startAgentFollow() {
  if (document.getElementById("h3-agent-dock")) return;
  addStyle();

  const entries = new Map(); // id -> entry, in the order they arrived
  const tried = new Map(); // project id -> when the panel last tried to open it
  let follow = remembered(FOLLOW, "1") !== "0";
  let open = false;
  let lastInput = 0;
  let selectedAt = 0;
  let opening = false;

  // -- the Builder, through the handle the Builder keeps (see tools/builder_ui_patches.json) -----------------------------------

  const builder = () => window.__h3Builder || null;
  const openProjectId = () => String(builder()?.state?.projectFolder || "").split(/[\\/]/).filter(Boolean).pop() || "";
  const inOpenProject = (id) => Boolean(id) && openProjectId().toLowerCase() === String(id).toLowerCase();
  const sceneNumber = (sceneId) => {
    const all = builder()?.state?.segments || [];
    const index = all.findIndex((segment) => segment?.id === sceneId);
    return index >= 0 ? index + 1 : 0;
  };
  const userIsTyping = () => {
    const active = document.activeElement;
    return Boolean(active && (active.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(active.tagName)) && !dock.contains(active));
  };
  const userIsBusy = () => userIsTyping() || Date.now() - lastInput < 2500;

  document.addEventListener("pointerdown", (event) => {
    if (!dock.contains(event.target)) lastInput = Date.now();
  }, true);
  document.addEventListener("keydown", (event) => {
    if (!dock.contains(event.target)) lastInput = Date.now();
  }, true);

  async function openProject(id, onFound = () => {}) {
    if (!builder() || !id) return false;
    try {
      const reply = await fetch(`${API}/projects/${encodeURIComponent(id)}`);
      const body = await reply.json();
      const folder = body?.data?.project_folder;
      // A project the agent has not made yet is not there to open. That is tried again with the next entry.
      if (!reply.ok || !folder) return false;
      onFound();
      return Boolean(await builder().loadProject(folder));
    } catch {
      return false;
    }
  }

  function reveal(sceneId) {
    requestAnimationFrame(() => {
      const card = [...document.querySelectorAll("[data-h3-scene-id]")].find((node) => node.dataset.h3SceneId === sceneId);
      if (!card) return;
      card.scrollIntoView({ block: "nearest", behavior: "smooth" });
      card.classList.remove("h3-agent-flash");
      void card.offsetWidth; // restart the animation
      card.classList.add("h3-agent-flash");
    });
  }

  function goToScene(sceneId) {
    if (!builder()?.selectScene(sceneId)) return false;
    selectedAt = Date.now();
    reveal(sceneId);
    return true;
  }

  /** Go where the agent is working: its project first, then its scene. */
  function followEntry(entry) {
    if (!follow || !builder() || entry.status === "cancelled") return;
    if (entry.project_id && !inOpenProject(entry.project_id)) {
      // A project with edits that are not saved, a text field in use or a render running is not left. A window with no project open has nothing to lose.
      if (entry.status === "failed" || opening || userIsBusy() || (openProjectId() && builder().busy()) || Date.now() - (tried.get(entry.project_id) || 0) < 3000) return;
      opening = true;
      openProject(entry.project_id, () => tried.set(entry.project_id, Date.now()))
        .then((opened) => {
          if (opened) note(`Opened ${entry.project_id} to follow the agent.`);
        })
        .finally(() => {
          opening = false;
        });
      return;
    }
    if (!entry.scene_id || userIsBusy() || Date.now() - selectedAt < 500) return;
    if (builder().state?.activeId === entry.scene_id) reveal(entry.scene_id);
    else goToScene(entry.scene_id);
  }

  // -- the entries -------------------------------------------------------------------------------------------------------------

  let localCount = 0;
  function note(text) {
    localCount += 1;
    upsert({ id: `local-${localCount}`, time: Date.now(), source: "follow", title: text, status: "done" }, false);
  }

  function upsert(entry, andFollow = true) {
    const known = entries.get(entry.id);
    entries.set(entry.id, { ...known, ...entry });
    while (entries.size > SHOWN) entries.delete(entries.keys().next().value);
    paint();
    if (andFollow && entry.source !== "follow" && (!known || known.status !== entry.status || entry.scene_id !== known.scene_id || entry.message !== known.message)) followEntry({ ...known, ...entry });
  }

  // -- the panel ---------------------------------------------------------------------------------------------------------------

  const dock = element("div", "position:fixed;right:16px;bottom:92px;z-index:100002;display:none;flex-direction:column;align-items:flex-end;gap:8px;font:12px 'Segoe UI',Inter,Arial,sans-serif;color:#e5e7eb;");
  dock.id = "h3-agent-dock";

  const panel = element("div", "display:none;width:min(420px,92vw);max-height:46vh;flex-direction:column;box-sizing:border-box;border:1px solid #155e75;border-radius:10px;background:#0b1220;box-shadow:0 12px 40px rgba(0,0,0,.55);overflow:hidden;");
  const header = element("div", "display:flex;align-items:center;gap:8px;padding:8px 10px;border-bottom:1px solid #1e293b;background:#0f172a;");
  header.append(element("div", "flex:1;font-weight:900;color:#f0fdfa;", "Agent activity"));
  const followLabel = element("label", "display:flex;align-items:center;gap:5px;color:#cbd5e1;font-weight:700;cursor:pointer;");
  followLabel.title = "Open the project the agent works in and select the scene it edits.";
  const followBox = element("input");
  followBox.type = "checkbox";
  followBox.checked = follow;
  followBox.onchange = () => {
    follow = followBox.checked;
    remember(FOLLOW, follow ? "1" : "0");
  };
  followLabel.append(followBox, document.createTextNode("Follow"));
  const clear = element("button", "border:1px solid #3f3f46;border-radius:6px;background:#27272a;color:#f4f4f5;padding:3px 8px;font-size:11px;font-weight:700;cursor:pointer;", "Clear");
  clear.type = "button";
  clear.onclick = () => {
    entries.clear();
    paint();
  };
  const hide = element("button", "border:1px solid #3f3f46;border-radius:6px;background:#27272a;color:#f4f4f5;padding:3px 8px;font-size:11px;font-weight:700;cursor:pointer;", "Hide");
  hide.type = "button";
  hide.onclick = () => setOpen(false);
  header.append(followLabel, clear, hide);
  const list = element("div", "overflow:auto;padding:4px 0;min-height:60px;");
  panel.append(header, list);

  const pill = element("button", "display:flex;align-items:center;gap:7px;border:1px solid #155e75;border-radius:999px;background:#0b1220;color:#e0f2fe;padding:6px 12px;font:700 12px 'Segoe UI',Inter,Arial,sans-serif;cursor:pointer;box-shadow:0 6px 20px rgba(0,0,0,.45);");
  pill.type = "button";
  const pillDot = element("span", "width:9px;height:9px;border-radius:50%;background:#71717a;");
  const pillText = element("span", "", "Agent");
  pill.append(pillDot, pillText);
  pill.onclick = () => setOpen(!open);
  dock.append(panel, pill);
  document.body.append(dock);

  function setOpen(value) {
    open = value;
    panel.style.display = open ? "flex" : "none";
    remember(PANEL, open ? "open" : "closed");
    if (open) list.scrollTop = list.scrollHeight;
  }

  function line(entry) {
    const row = element("div", "display:flex;gap:8px;padding:6px 10px;border-bottom:1px solid #111827;");
    const dot = element("span", `flex:0 0 9px;height:9px;margin-top:4px;border-radius:50%;background:${DOT[entry.status] || DOT.done};`);
    if (entry.status === "running") dot.className = "h3-agent-running";
    const body = element("div", "flex:1;min-width:0;");
    const top = element("div", "display:flex;gap:6px;align-items:baseline;");
    const title = element("span", "font-weight:800;color:#f0f9ff;overflow-wrap:anywhere;", entry.title || entry.tool || "Agent");
    top.append(title);
    const scene = entry.scene_id ? sceneNumber(entry.scene_id) : 0;
    if (scene && inOpenProject(entry.project_id)) top.append(element("span", "color:#67e8f9;font-weight:800;white-space:nowrap;", `Scene ${scene}`));
    else if (entry.project_id && !inOpenProject(entry.project_id)) top.append(element("span", "color:#fcd34d;white-space:nowrap;max-width:140px;overflow:hidden;text-overflow:ellipsis;", entry.project_id));
    top.append(element("span", "margin-left:auto;color:#64748b;font-size:11px;white-space:nowrap;", new Date(entry.time || Date.now()).toLocaleTimeString([], { hour12: false })));
    body.append(top);
    const said = [entry.detail, entry.status === "running" && Number.isFinite(entry.percent) && entry.percent > 0 ? `${Math.round(entry.percent)}%` : ""].filter(Boolean).join("  ");
    if (said) body.append(element("div", "color:#94a3b8;overflow-wrap:anywhere;line-height:1.35;", said));
    if (entry.message) body.append(element("div", `color:${entry.status === "failed" ? "#fca5a5" : "#a7f3d0"};overflow-wrap:anywhere;line-height:1.35;`, entry.message));
    row.append(dot, body);
    if (entry.project_id && !inOpenProject(entry.project_id) && entry.source !== "follow" && entry.status !== "failed") {
      const go = element("button", "align-self:center;border:1px solid #0891b2;border-radius:6px;background:#0e7490;color:#f4f4f5;padding:3px 8px;font-size:11px;font-weight:700;cursor:pointer;", "Open");
      go.type = "button";
      go.onclick = (event) => {
        event.stopPropagation();
        openProject(entry.project_id);
      };
      row.append(go);
    } else if (entry.scene_id && inOpenProject(entry.project_id)) {
      row.style.cursor = "pointer";
      row.title = "Go to this scene";
      row.onclick = () => goToScene(entry.scene_id);
    }
    return row;
  }

  function paint() {
    const all = [...entries.values()];
    dock.style.display = all.length ? "flex" : "none";
    const running = all.filter((entry) => entry.status === "running").length;
    pillDot.style.background = running ? DOT.running : all.some((entry) => entry.status === "failed" && Date.now() - (entry.time || 0) < 60000) ? DOT.failed : all.length ? DOT.done : "#71717a";
    pillDot.className = running ? "h3-agent-running" : "";
    pillText.textContent = running ? `Agent  ${running} running` : "Agent";
    const stuck = list.scrollTop + list.clientHeight >= list.scrollHeight - 24;
    list.replaceChildren(...all.map(line));
    if (!all.length) list.append(element("div", "padding:14px;color:#64748b;", "Nothing yet. What an agent does through the MCP server shows here."));
    if (stuck) list.scrollTop = list.scrollHeight;
  }

  // -- the stream ---------------------------------------------------------------------------------------------------------------

  let first = true;
  api.addEventListener("vrgdg.agent_activity", (event) => {
    const entry = event.detail || {};
    if (entry.id === undefined) return;
    if (first) {
      first = false;
      if (remembered(PANEL, "open") !== "closed") setOpen(true);
    }
    upsert(entry);
  });

  // What happened before this window opened.
  fetch(`${API}/agent/activity?after=0`)
    .then((reply) => reply.json())
    .then((body) => {
      for (const entry of body?.data?.items || []) entries.set(entry.id, { ...entry, ...entries.get(entry.id) });
      paint();
    })
    .catch(() => {
      // No backlog: the feed starts with what happens from now on.
    });

  // The scene names change when the open project does.
  window.addEventListener("vrgdg:builder-session-loaded", paint);
}
