// Updates: the Menu's "Check for Updates" and the note the app shows at start when a newer version is out. The app window does the work through
// the update commands of the Tauri shell (see src-tauri/src/updates.rs). An update is only installed after the user clicks Install and restart,
// never while a job runs, and only when its signature matches the key built into the app.

import { button, element } from "/host/settings.js";

const API = "/api/v1";
const SKIPPED = "h3studio.updateSkipped";
const REPO = "github.com/nvmax/VideoBuilderH3";
const START_DELAY_MS = 8000;

/** The shell of the app window. A browser tab that shows the app has none, so it cannot install anything. */
export function shell() {
  const internals = window.__TAURI_INTERNALS__;
  return internals?.invoke ? internals : null;
}

const ask = (command) => shell().invoke(command);

async function config() {
  const reply = await fetch(`${API}/setup/config`);
  const body = await reply.json().catch(() => ({}));
  return body?.data || {};
}

async function saveCheckSetting(on) {
  const reply = await fetch(`${API}/setup/config`, { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ check_for_updates: on }) });
  const body = await reply.json().catch(() => ({}));
  if (!reply.ok || body.ok === false) throw new Error(body?.error?.message || `The app answered ${reply.status}.`);
}

function megabytes(bytes) {
  return `${(bytes / 1048576).toFixed(1)} MiB`;
}

function skippedVersion() {
  try {
    return localStorage.getItem(SKIPPED) || "";
  } catch {
    return "";
  }
}

function skipVersion(version) {
  try {
    localStorage.setItem(SKIPPED, version);
  } catch {
    // Not remembered: the note shows again next time.
  }
}

export function openUpdates(found = null) {
  if (document.getElementById("h3-updates")) return;
  const shade = element("div", "position:fixed;inset:0;z-index:100010;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.6);");
  shade.id = "h3-updates";
  const panel = element("div", "width:min(560px,94vw);max-height:92vh;overflow:auto;box-sizing:border-box;border:1px solid #155e75;border-radius:10px;background:#0b1220;color:#e5e7eb;padding:18px;font:13px 'Segoe UI',Inter,Arial,sans-serif;box-shadow:0 18px 60px rgba(0,0,0,.6);");
  shade.append(panel);
  panel.append(element("div", "font-size:17px;font-weight:900;color:#f0fdfa;", "Updates"));
  const installed = element("div", "color:#94a3b8;margin:4px 0 12px;", "");
  const status = element("div", "min-height:18px;margin:0 0 10px;color:#cbd5e1;line-height:1.45;");
  const notes = element("div", "display:none;max-height:220px;overflow:auto;white-space:pre-wrap;margin:0 0 12px;padding:8px 10px;border:1px solid #1e293b;border-radius:8px;background:#0f172a;color:#cbd5e1;font-size:12px;line-height:1.45;");
  const meter = element("div", "display:none;height:6px;margin:0 0 12px;border-radius:3px;background:#1e293b;overflow:hidden;");
  const bar = element("div", "height:100%;width:0;background:#0891b2;");
  meter.append(bar);
  panel.append(installed, status, notes, meter);

  const startup = element("label", "display:flex;gap:8px;align-items:center;margin:0 0 10px;color:#cbd5e1;");
  const startupBox = element("input");
  startupBox.type = "checkbox";
  startupBox.checked = true;
  startup.append(startupBox, element("span", "", "Look for updates when the app starts"));
  panel.append(startup);
  panel.append(element("div", "color:#71717a;font-size:11px;line-height:1.45;margin:0 0 12px;", `Updates come from ${REPO}. The check reads one public file there and sends nothing about you or your projects. An update only installs when its signature matches the key built into this app, and never while a job runs.`));

  const buttons = element("div", "display:flex;gap:8px;justify-content:flex-end;flex-wrap:wrap;");
  const checkButton = button("Check now");
  const skipButton = button("Skip this version");
  const installButton = button("Install and restart", true);
  const closeButton = button("Close");
  skipButton.style.display = "none";
  installButton.style.display = "none";
  buttons.append(checkButton, skipButton, installButton, closeButton);
  panel.append(buttons);

  const say = (text, color = "#cbd5e1") => {
    status.textContent = text;
    status.style.color = color;
  };
  let offered = null;
  const show = (info) => {
    offered = info;
    notes.style.display = info?.notes ? "block" : "none";
    notes.textContent = info?.notes || "";
    installButton.style.display = info ? "" : "none";
    skipButton.style.display = info ? "" : "none";
    if (info) say(`Version ${info.version} is available.`, "#86efac");
  };

  const check = async () => {
    checkButton.disabled = true;
    say("Looking for a new version…");
    try {
      const info = await ask("update_check");
      show(info);
      if (!info) say("This is the newest version.", "#86efac");
    } catch (error) {
      show(null);
      say(String(error?.message || error), "#fca5a5");
    } finally {
      checkButton.disabled = false;
    }
  };

  installButton.onclick = async () => {
    installButton.disabled = checkButton.disabled = skipButton.disabled = true;
    meter.style.display = "block";
    say("Downloading the update…");
    const poll = setInterval(async () => {
      try {
        const progress = await ask("update_progress");
        if (progress.phase === "downloading" && progress.total) {
          bar.style.width = `${Math.min(100, (progress.downloaded / progress.total) * 100)}%`;
          say(`Downloading the update: ${megabytes(progress.downloaded)} of ${megabytes(progress.total)}`);
        } else if (progress.phase === "installing") {
          bar.style.width = "100%";
          say("Installing. The app closes and opens again by itself.");
        }
      } catch {
        // The app is closing for the installer.
      }
    }, 400);
    try {
      await ask("update_install");
    } catch (error) {
      say(String(error?.message || error), "#fca5a5");
      meter.style.display = "none";
      installButton.disabled = checkButton.disabled = skipButton.disabled = false;
    } finally {
      clearInterval(poll);
    }
  };
  skipButton.onclick = () => {
    if (offered) skipVersion(offered.version);
    shade.remove();
  };
  closeButton.onclick = () => shade.remove();
  startupBox.onchange = async () => {
    try {
      await saveCheckSetting(startupBox.checked);
    } catch (error) {
      startupBox.checked = !startupBox.checked;
      say(String(error?.message || error), "#fca5a5");
    }
  };
  checkButton.onclick = check;
  shade.addEventListener("keydown", (event) => event.key === "Escape" && closeButton.click());
  document.body.append(shade);

  config().then((saved) => {
    startupBox.checked = saved.check_for_updates !== false;
  }).catch(() => {});
  if (!shell()) {
    installed.textContent = "Updates are installed from the app window. This page is open in a browser.";
    checkButton.style.display = "none";
    return;
  }
  ask("app_version").then((version) => {
    installed.textContent = `Version ${version} is installed.`;
  }).catch(() => {});
  if (found) show(found);
  else check();
}

function showNote(info) {
  if (document.getElementById("h3-update-note")) return;
  const note = element("div", "position:fixed;right:16px;bottom:16px;z-index:100005;max-width:340px;box-sizing:border-box;border:1px solid #155e75;border-radius:10px;background:#0b1220;color:#e5e7eb;padding:12px 14px;font:13px 'Segoe UI',Inter,Arial,sans-serif;box-shadow:0 12px 40px rgba(0,0,0,.5);");
  note.id = "h3-update-note";
  note.append(element("div", "font-weight:800;color:#f0fdfa;margin-bottom:2px;", `Version ${info.version} is available`), element("div", "color:#94a3b8;margin-bottom:10px;", `You have ${info.current_version}.`));
  const row = element("div", "display:flex;gap:8px;justify-content:flex-end;");
  const view = button("View update", true);
  const later = button("Later");
  view.onclick = () => {
    note.remove();
    openUpdates(info);
  };
  later.onclick = () => note.remove();
  row.append(later, view);
  note.append(row);
  document.body.append(note);
}

/** A few seconds after the app starts, look for a new version unless the check is switched off. A failed check says nothing. */
export function startUpdates() {
  if (!shell()) return;
  setTimeout(async () => {
    try {
      if ((await config()).check_for_updates === false) return;
      const info = await ask("update_check");
      if (info && info.version !== skippedVersion()) showNote(info);
    } catch {
      // Offline, or no release yet: nothing to show.
    }
  }, START_DELAY_MS);
}
