// Follow the agent. While an AI agent works on a project through the MCP server or the API, this panel lists what it does as it happens: that it connected, each tool
// it calls with the API call behind it and what it changed, and each job it starts with how far the job is. With "Follow" on, the Builder goes where the agent goes: it
// opens the project the agent works in and selects the scene the agent is working on, scrolling to it and flashing it. A scene an agent renders glows as rendering in the
// timeline, as one the user renders does, and its card in the scene list says how far the render is. The edits themselves appear in the open project on their own (the
// Builder's own merge of `vrgdg.project_changed`, see music_video_builder/external_changes.mjs), so what you see is the real project, not a replay.
//
// The server sends the entries over the event stream as `vrgdg.agent_activity` and the scene a job works on as `vrgdg.scene_work`. It keeps the last ones for a window that
// opens late (`GET /api/v1/agent/activity`).

import { api } from "/scripts/api.js";

const API = "/api/v1";
const FOLLOW = "h3studio.agent.follow";
const PANEL = "h3studio.agent.panel";
const READS = "h3studio.agent.reads";
const SHOWN = 400;
const STYLE_ID = "h3-agent-style";
const CONNECTED_SHOWN_MS = 15000;

const DOT = { running: "#f59e0b", done: "#22c55e", failed: "#ef4444", cancelled: "#71717a", connect: "#22d3ee" };

// Scene fields an agent changes, in words.
const FIELD_NAMES = {
  label: "label", notes: "notes", timeline_note: "timeline note", lyric_text: "lyrics", minimax_h3_prompt: "MiniMax prompt", minimax_h3_pass2_prompt: "pass 2 prompt",
  i2v_notes: "video notes", i2v_prompt: "video prompt", start: "timing", end: "timing", video_path: "video", approved_image_path: "image", characters: "characters", location: "location",
};

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

/** 4:21 or 1:02:03. */
function clock(seconds) {
  const total = Math.max(0, Math.round(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}` : `${m}:${String(s).padStart(2, "0")}`;
}

// A render or a new picture changes a dozen fields of a scene (paths, history, thumbnails, flags). They are one change in words.
const FIELD_GROUPS = [
  [/^(raw_video|rendered_video|video|thumbnail|preview_mode|scene_audio_render|minimax_h3_continuity)/, "video"],
  [/^(approved_image|custom_image|image)/, "image"],
];

function fieldName(key) {
  if (FIELD_NAMES[key]) return FIELD_NAMES[key];
  const group = FIELD_GROUPS.find(([pattern]) => pattern.test(String(key)));
  return group ? group[1] : String(key).replace(/_/g, " ");
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
    @keyframes h3-agent-outline { 0%, 100% { outline-color: rgba(59,130,246,.95); } 50% { outline-color: rgba(59,130,246,.3); } }
    .h3-agent-working { outline: 2px solid rgba(59,130,246,.95); outline-offset: -2px; animation: h3-agent-outline 1.4s ease-in-out infinite; }
  `;
  document.head.append(style);
}

const BADGE_STYLE = "position:absolute;left:6px;right:6px;bottom:3px;padding:1px 6px;border-radius:5px;background:rgba(8,47,73,.92);border:1px solid #0e7490;color:#e0f2fe;"
  + "font:700 10px 'Segoe UI',Inter,Arial,sans-serif;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;pointer-events:none;z-index:2;";

export function startAgentFollow() {
  if (document.getElementById("h3-agent-dock")) return;
  addStyle();

  const entries = new Map(); // id -> entry, in the order they arrived
  const expanded = new Set(); // ids of the entries whose details are open
  const working = new Map(); // "project|scene" -> what a job works on now (`vrgdg.scene_work`)
  const tried = new Map(); // project id -> when the panel last tried to open it
  let follow = remembered(FOLLOW, "1") !== "0";
  let showReads = remembered(READS, "1") !== "0";
  let open = false;
  let lastInput = 0;
  let selectedAt = 0;
  let opening = false;
  let connectedName = "";
  let connectedUntil = 0;

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

  // -- scenes in work: the glow in the timeline and the badge on the card -------------------------------------------------------

  const workKey = (project, scene) => `${String(project || "").toLowerCase()}|${scene}`;

  /** The scenes of the open project that jobs work on now. */
  function scenesInWork() {
    const wanted = new Map();
    for (const work of working.values()) if (inOpenProject(work.project_id)) wanted.set(work.scene_id, work);
    return wanted;
  }

  /** Make the Builder's timeline draw the scenes an agent renders as rendering, and only those. */
  function syncRendering() {
    const handle = builder();
    if (!handle?.agentRendering || !handle.markRendering) return;
    try {
      const wanted = new Set([...scenesInWork().values()].filter((work) => work.what === "video").map((work) => work.scene_id));
      for (const id of [...handle.agentRendering]) if (!wanted.has(id)) handle.markRendering(id, false);
      for (const id of wanted) handle.markRendering(id, true);
    } catch (error) {
      console.warn("[VRGDG H3 Studio] Showing the render the agent started failed", error);
    }
  }

  function badgeText(work) {
    const what = work.what === "image" ? "Agent making image" : "Agent rendering";
    const percent = Number.isFinite(work.percent) ? ` ${Math.round(work.percent)}%` : "";
    // The message reads "Rendering with ComfyUI - Pass 1 sampler step 12 of 30 - about 2:10 left - 4:21 elapsed": the card shows what follows the first part.
    const tail = String(work.message || "").split(" - ").slice(1).join(" - ");
    return `${what}${percent}${tail ? ` - ${tail}` : ""}`;
  }

  /** The badge on the card of each scene in work. The Builder draws the scene list again at any change, so this is done again every second. */
  function decorate() {
    const wanted = scenesInWork();
    for (const badge of document.querySelectorAll(".h3-agent-badge")) {
      const card = badge.parentElement;
      if (wanted.has(card?.dataset?.h3SceneId)) continue;
      card?.classList.remove("h3-agent-working");
      badge.remove();
    }
    if (!wanted.size) return;
    for (const card of document.querySelectorAll("[data-h3-scene-id]")) {
      const work = wanted.get(card.dataset.h3SceneId);
      if (!work) continue;
      let badge = card.querySelector(":scope > .h3-agent-badge");
      if (!badge) {
        badge = element("div", BADGE_STYLE);
        badge.className = "h3-agent-badge";
        card.append(badge);
      }
      card.classList.add("h3-agent-working");
      const text = badgeText(work);
      if (badge.textContent !== text) badge.textContent = text;
      badge.title = work.message || "";
    }
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
    if (entry.kind === "connect" && !known) {
      connectedName = entry.client || "an agent";
      connectedUntil = Date.now() + CONNECTED_SHOWN_MS;
      setTimeout(paint, CONNECTED_SHOWN_MS + 100);
    }
    paint();
    if (andFollow && entry.source !== "follow" && (!known || known.status !== entry.status || entry.scene_id !== known.scene_id || entry.message !== known.message)) followEntry({ ...known, ...entry });
  }

  // -- the panel ---------------------------------------------------------------------------------------------------------------

  const dock = element("div", "position:fixed;right:16px;bottom:92px;z-index:100010;display:none;flex-direction:column;align-items:flex-end;gap:8px;font:12px 'Segoe UI',Inter,Arial,sans-serif;color:#e5e7eb;");
  dock.id = "h3-agent-dock";

  const panel = element("div", "display:none;width:min(460px,92vw);max-height:52vh;flex-direction:column;box-sizing:border-box;border:1px solid #155e75;border-radius:10px;background:#0b1220;box-shadow:0 12px 40px rgba(0,0,0,.55);overflow:hidden;");
  const header = element("div", "display:flex;align-items:center;gap:8px;padding:8px 10px;border-bottom:1px solid #1e293b;background:#0f172a;");
  header.append(element("div", "flex:1;font-weight:900;color:#f0fdfa;", "Agent activity"));
  const checkbox = (label, title, checked, onchange) => {
    const wrap = element("label", "display:flex;align-items:center;gap:5px;color:#cbd5e1;font-weight:700;cursor:pointer;");
    wrap.title = title;
    const box = element("input");
    box.type = "checkbox";
    box.checked = checked;
    box.onchange = () => onchange(box.checked);
    wrap.append(box, document.createTextNode(label));
    return wrap;
  };
  const followLabel = checkbox("Follow", "Open the project the agent works in and select the scene it works on.", follow, (value) => {
    follow = value;
    remember(FOLLOW, follow ? "1" : "0");
  });
  const readsLabel = checkbox("Reads", "List what the agent only looks at (dimmed), not just what it changes.", showReads, (value) => {
    showReads = value;
    remember(READS, showReads ? "1" : "0");
    paint();
  });
  const button = (text, onclick) => {
    const node = element("button", "border:1px solid #3f3f46;border-radius:6px;background:#27272a;color:#f4f4f5;padding:3px 8px;font-size:11px;font-weight:700;cursor:pointer;", text);
    node.type = "button";
    node.onclick = onclick;
    return node;
  };
  const clear = button("Clear", () => {
    entries.clear();
    expanded.clear();
    paint();
  });
  const hide = button("Hide", () => setOpen(false));
  header.append(followLabel, readsLabel, clear, hide);
  const list = element("div", "overflow:auto;padding:4px 0;min-height:60px;");
  panel.append(header, list);

  const pill = element("button", "display:flex;align-items:center;gap:7px;border:1px solid #155e75;border-radius:999px;background:#0b1220;color:#e0f2fe;padding:6px 12px;font:700 12px 'Segoe UI',Inter,Arial,sans-serif;cursor:pointer;box-shadow:0 4px 18px rgba(0,0,0,.45);max-width:min(460px,92vw);");
  pill.type = "button";
  const pillDot = element("span", "flex:0 0 9px;width:9px;height:9px;border-radius:50%;background:#71717a;");
  const pillText = element("span", "overflow:hidden;text-overflow:ellipsis;white-space:nowrap;", "Agent");
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

  /** "Scene 3: label, MiniMax prompt" for each scene an entry changed, one text per scene. `most` keeps the first fields of each and counts the rest. */
  function touchedLines(entry, most = Infinity) {
    const lines = [];
    for (const item of entry.touched || []) {
      if (item.project) {
        lines.push("The project");
      } else if (item.notes) {
        lines.push("Timeline notes");
      } else if (item.transitions) {
        lines.push("Transitions");
      } else if (item.effects) {
        lines.push("Effects");
      } else if (item.filters) {
        lines.push("Filters");
      } else {
        const number = inOpenProject(entry.project_id) ? sceneNumber(item.scene_id) : 0;
        const fields = [...new Set((item.fields || []).map(fieldName))];
        const shown = fields.slice(0, most).join(", ") + (fields.length > most ? ` and ${fields.length - most} more` : "");
        lines.push(`${number ? `Scene ${number}` : item.scene_id}: ${shown || "its data"}`);
      }
    }
    return lines;
  }

  function summaryOf(entry) {
    if (entry.kind === "connect") return entry.detail || "";
    if (entry.kind === "read") return entry.result || entry.detail || "";
    const lines = touchedLines(entry, 4);
    if (lines.length) return `Changed ${lines.length > 2 ? `${lines.slice(0, 2).join("; ")} and ${lines.length - 2} more` : lines.join("; ")}`;
    // A job says what it does in its message, and a call what it was told.
    return entry.source === "job" ? "" : entry.detail || "";
  }

  function details(entry) {
    const box = element("div", "margin-top:6px;padding:6px 8px;border-radius:6px;background:#0f172a;border:1px solid #1e293b;display:grid;grid-template-columns:auto 1fr;gap:3px 10px;font-size:11px;cursor:text;");
    box.onclick = (event) => event.stopPropagation();
    const add = (label, value, css = "") => {
      if (value === undefined || value === null || value === "") return;
      box.append(element("div", "color:#64748b;white-space:nowrap;", label), element("div", `color:#cbd5e1;overflow-wrap:anywhere;${css}`, String(value)));
    };
    const mono = "font-family:Consolas,'Cascadia Mono',monospace;";
    add("API", entry.api, mono);
    add("What it does", entry.about);
    add("Tool", entry.source === "mcp" && entry.kind !== "connect" ? entry.tool : "", mono);
    add("Told", entry.detail);
    add("Answered", entry.result);
    add("Changed", touchedLines(entry).join("\n"), "white-space:pre-line;");
    add("Took", Number.isFinite(entry.duration_ms) ? `${(entry.duration_ms / 1000).toFixed(entry.duration_ms < 10000 ? 1 : 0)} s` : "");
    add("HTTP", entry.http_status);
    add("Client", entry.client);
    add("Job", entry.job_id, mono);
    add("Stage", entry.stage);
    if (!box.children.length) add("Nothing more", "This entry has no further details.");
    return box;
  }

  function line(entry, count = 1) {
    const isRead = entry.kind === "read";
    const isOpen = expanded.has(entry.id);
    const row = element("div", `display:flex;gap:8px;padding:6px 10px;border-bottom:1px solid #111827;cursor:pointer;${isRead ? "opacity:.7;" : ""}`);
    row.title = isOpen ? "Hide the details" : "Show the details";
    const dot = element("span", `flex:0 0 9px;height:9px;margin-top:4px;border-radius:50%;background:${entry.kind === "connect" ? DOT.connect : DOT[entry.status] || DOT.done};`);
    if (entry.status === "running") dot.className = "h3-agent-running";
    const body = element("div", "flex:1;min-width:0;");
    const top = element("div", "display:flex;gap:6px;align-items:baseline;");
    top.append(element("span", `font-weight:${isRead ? 600 : 800};color:#f0f9ff;overflow-wrap:anywhere;`, entry.title || entry.tool || "Agent"));
    if (count > 1) top.append(element("span", "color:#94a3b8;font-weight:700;", `x${count}`));
    const scene = entry.scene_id ? sceneNumber(entry.scene_id) : 0;
    if (scene && inOpenProject(entry.project_id)) {
      const chip = element("span", "color:#67e8f9;font-weight:800;white-space:nowrap;text-decoration:underline dotted;", `Scene ${scene}`);
      chip.title = "Go to this scene";
      chip.onclick = (event) => {
        event.stopPropagation();
        goToScene(entry.scene_id);
      };
      top.append(chip);
    } else if (entry.project_id && !inOpenProject(entry.project_id)) {
      top.append(element("span", "color:#fcd34d;white-space:nowrap;max-width:140px;overflow:hidden;text-overflow:ellipsis;", entry.project_id));
    }
    if (entry.scene_count > 1) top.append(element("span", "color:#94a3b8;white-space:nowrap;", `${entry.scene_index} of ${entry.scene_count}`));
    const when = element("span", "margin-left:auto;color:#64748b;font-size:11px;white-space:nowrap;");
    if (entry.status === "running") {
      // The clock of a running entry moves every second (see the timer below).
      const elapsed = element("span", "color:#fbbf24;", clock((Date.now() - (entry.time || Date.now())) / 1000));
      elapsed.dataset.since = String(entry.time || Date.now());
      when.append(elapsed);
    } else {
      when.textContent = new Date(entry.time || Date.now()).toLocaleTimeString([], { hour12: false });
    }
    top.append(when);
    body.append(top);
    const said = summaryOf(entry);
    if (said) body.append(element("div", "color:#94a3b8;overflow-wrap:anywhere;line-height:1.35;", said));
    if (entry.message) body.append(element("div", `color:${entry.status === "failed" ? "#fca5a5" : entry.status === "running" ? "#cbd5e1" : "#a7f3d0"};overflow-wrap:anywhere;line-height:1.35;`, entry.message));
    // Only a job that measures its work has a percent. The rest show their message and clock.
    if (entry.status === "running" && Number.isFinite(entry.percent)) {
      const bar = element("div", "display:flex;align-items:center;gap:6px;margin-top:3px;");
      const track = element("div", "flex:1;height:5px;border-radius:3px;background:#1e293b;overflow:hidden;");
      track.append(element("div", `height:100%;width:${Math.max(2, Math.min(100, entry.percent))}%;background:#22d3ee;`));
      bar.append(track, element("span", "color:#67e8f9;font-weight:800;min-width:30px;text-align:right;", `${Math.round(entry.percent)}%`));
      body.append(bar);
    }
    if (isOpen) body.append(details(entry));
    row.append(dot, body);
    if (entry.project_id && !inOpenProject(entry.project_id) && entry.source !== "follow" && entry.status !== "failed" && entry.kind !== "connect") {
      const go = element("button", "align-self:center;border:1px solid #0891b2;border-radius:6px;background:#0e7490;color:#f4f4f5;padding:3px 8px;font-size:11px;font-weight:700;cursor:pointer;", "Open");
      go.type = "button";
      go.onclick = (event) => {
        event.stopPropagation();
        openProject(entry.project_id);
      };
      row.append(go);
    }
    row.onclick = () => {
      if (expanded.has(entry.id)) expanded.delete(entry.id);
      else expanded.add(entry.id);
      paint();
    };
    return row;
  }

  const sameRead = (a, b) => a.title === b.title && a.detail === b.detail && a.scene_id === b.scene_id && a.project_id === b.project_id && a.status === "done" && b.status === "done";

  function paint() {
    const all = [...entries.values()];
    dock.style.display = all.length ? "flex" : "none";
    const running = all.filter((entry) => entry.status === "running").length;
    const connected = Date.now() < connectedUntil;
    pillDot.style.background = connected ? DOT.connect : running ? DOT.running : all.some((entry) => entry.status === "failed" && Date.now() - (entry.time || 0) < 60000) ? DOT.failed : all.length ? DOT.done : "#71717a";
    pillDot.className = running || connected ? "h3-agent-running" : "";
    pillText.textContent = connected ? `Agent connected: ${connectedName}` : running ? `Agent  ${running} running` : "Agent";
    // A read that is repeated is one row with a count.
    const rows = [];
    for (const entry of all) {
      if (entry.kind === "read" && !showReads) continue;
      const last = rows[rows.length - 1];
      if (entry.kind === "read" && last?.entry.kind === "read" && sameRead(last.entry, entry)) {
        last.count += 1;
        continue;
      }
      rows.push({ entry, count: 1 });
    }
    const stuck = list.scrollTop + list.clientHeight >= list.scrollHeight - 24;
    const kept = list.scrollTop;
    list.replaceChildren(...rows.map((row) => line(row.entry, row.count)));
    if (!rows.length) list.append(element("div", "padding:14px;color:#64748b;", "Nothing yet. What an agent does through the MCP server shows here."));
    list.scrollTop = stuck ? list.scrollHeight : kept;
  }

  // The clock of a running entry, and the badges (the Builder draws the scene list again at any change). The panel stands above the Builder's own dialogs
  // (the welcome dialog is at 100006), so an agent that connects while one is open can still be seen and used.
  setInterval(() => {
    for (const node of list.querySelectorAll("[data-since]")) node.textContent = clock((Date.now() - Number(node.dataset.since)) / 1000);
    if (working.size) decorate();
  }, 500);

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

  api.addEventListener("vrgdg.scene_work", (event) => {
    const work = event.detail || {};
    if (!work.scene_id) return;
    if (work.state === "on") working.set(workKey(work.project_id, work.scene_id), work);
    else working.delete(workKey(work.project_id, work.scene_id));
    syncRendering();
    decorate();
  });

  // What happened before this window opened, and the scenes jobs work on now.
  fetch(`${API}/agent/activity?after=0`)
    .then((reply) => reply.json())
    .then((body) => {
      for (const entry of body?.data?.items || []) entries.set(entry.id, { ...entry, ...entries.get(entry.id) });
      for (const work of body?.data?.scenes_in_work || []) working.set(workKey(work.project_id, work.scene_id), { ...work, state: "on" });
      paint();
      syncRendering();
      decorate();
    })
    .catch(() => {
      // No backlog: the feed starts with what happens from now on.
    });

  // The scene names, and which scenes are in work, change when the open project does.
  window.addEventListener("vrgdg:builder-session-loaded", () => {
    paint();
    syncRendering();
    decorate();
  });
}
