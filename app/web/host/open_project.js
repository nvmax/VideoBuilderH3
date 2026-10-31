// The project this window has open. It does two things: it tells the server (`PUT /api/v1/app/open-project`, see crates/h3-api/src/open_project.rs), so an agent
// that connects is told which project the user means by "this project", and it shows the project's name in a bar at the top of the Builder.
//
// The Builder keeps the open project's folder in its state (`window.__h3Builder.state.projectFolder`, see tools/builder_ui_patches.json). It is read when a
// project has loaded and every second after that, which also catches New Project, Save Project As and Branch Project.

const API = "/api/v1/app/open-project";
const BAR_ID = "h3-project-bar";
const CHECK_MS = 1000;

const builder = () => window.__h3Builder || null;

/** The last part of a folder path: the project's id. */
export function projectIdOf(folder) {
  return String(folder || "").split(/[\\/]/).filter(Boolean).pop() || "";
}

function element(tag, css, text) {
  const node = document.createElement(tag);
  if (css) node.style.cssText = css;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Tell the server which folder is open (empty for none). Returns what it answered, or null when it could not be reached. */
async function send(folder, keepalive = false) {
  try {
    const reply = await fetch(API, { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ project_folder: folder }), keepalive });
    const body = await reply.json();
    return reply.ok ? body?.data || null : null;
  } catch {
    return null;
  }
}

export function startOpenProject() {
  if (document.getElementById(BAR_ID)) return;

  const bar = element("div", "display:flex;align-items:baseline;gap:10px;min-width:0;padding:7px 14px;border-bottom:1px solid #155e75;background:#0f172a;color:#fafafa;font-family:'Segoe UI',Inter,Arial,sans-serif;");
  bar.id = BAR_ID;
  const label = element("span", "flex:0 0 auto;color:#22d3ee;font-size:11px;font-weight:800;letter-spacing:.08em;text-transform:uppercase;", "Project");
  const name = element("span", "min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:17px;font-weight:800;");
  const hint = element("span", "flex:0 0 auto;margin-left:auto;color:#fcd34d;font-size:11px;font-weight:600;");
  bar.append(label, name, hint);

  let reported; // the folder the server was last told
  let findable = true; // whether the server could find that project
  let sending = false;
  let painted = "";

  function paint(folder) {
    const id = projectIdOf(folder);
    // Nothing to warn about until the server has answered for this folder.
    const lost = Boolean(id) && reported === folder && !findable;
    const key = `${folder}|${lost}`;
    if (key === painted) return;
    painted = key;
    name.textContent = id || "No project open";
    name.style.color = id ? "#f4f4f5" : "#71717a";
    name.title = folder || "";
    hint.textContent = lost ? "Agents cannot see this project" : "";
    hint.title = lost ? "It is not saved yet, or its folder is not one of the project folders the app lists." : "";
  }

  async function check() {
    const handle = builder();
    if (!handle) return;
    const folder = String(handle.state?.projectFolder || "").trim();
    const header = handle.header?.();
    if (header && !header.contains(bar)) header.prepend(bar);
    if (folder !== reported && !sending) {
      sending = true;
      const answer = await send(folder);
      sending = false;
      if (answer) {
        reported = folder;
        findable = answer.open === true;
      }
    }
    paint(folder);
  }

  setInterval(check, CHECK_MS);
  window.addEventListener("vrgdg:builder-session-loaded", check);
  // A closed window has no project open. A reload reports again as soon as the Builder is back.
  window.addEventListener("pagehide", () => {
    send("", true);
  });
  check();
}
