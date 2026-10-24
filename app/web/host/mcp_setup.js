// MCP Setup: how to connect an AI agent to this app. The app has an MCP server of its own, so an agent needs nothing else installed. This screen shows
// the address and the exact settings for the common agents, with a Copy button, and checks that the server answers. Opened by the "MCP Setup" entry
// of the Builder's Menu (see settings.js).

import { button, element } from "/host/settings.js";

const API = "/api/v1";

async function info() {
  const reply = await fetch(`${API}/setup/mcp`);
  const body = await reply.json().catch(() => ({}));
  if (!reply.ok || body.ok === false) throw new Error(body?.error?.message || `The app answered ${reply.status}.`);
  return body.data;
}

/** The same two calls an agent makes first: initialize, then the tool list. */
async function checkConnection(url) {
  const post = async (message) => {
    const reply = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(message) });
    if (!reply.ok) throw new Error(`The MCP server answered ${reply.status}.`);
    return reply.json();
  };
  const started = await post({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "H3 Studio MCP Setup", version: "1" } } });
  const tools = await post({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  return { name: started?.result?.serverInfo?.name || "server", tools: tools?.result?.tools?.length || 0 };
}

function clients(data) {
  const url = data.mcp_url;
  const exe = data.stdio?.command || "H3 Studio.exe";
  const json = (value) => JSON.stringify(value, null, 2);
  return [
    {
      title: "Claude Code",
      note: "Run this once in a terminal.",
      code: `claude mcp add --transport http h3-studio ${url}`,
    },
    {
      title: "Claude Desktop",
      note: "Add this to claude_desktop_config.json (Settings, Developer, Edit Config), then restart Claude Desktop. It starts the app's own bridge, which opens H3 Studio when it is closed.",
      code: json({ mcpServers: { "h3-studio": { command: exe, args: data.stdio?.args || ["--mcp-stdio"] } } }),
    },
    {
      title: "Cursor or LM Studio",
      note: "Add this to the mcp.json of the program (Cursor: Settings, MCP. LM Studio: Program, Edit mcp.json).",
      code: json({ mcpServers: { "h3-studio": { url } } }),
    },
    {
      title: "VS Code",
      note: "Add this to .vscode/mcp.json in a folder, or to your user MCP settings.",
      code: json({ servers: { "h3-studio": { type: "http", url } } }),
    },
    {
      title: "Codex CLI",
      note: "Add this to ~/.codex/config.toml.",
      code: `[mcp_servers.h3-studio]\nurl = "${url}"`,
    },
    {
      title: "Gemini CLI",
      note: "Add this to ~/.gemini/settings.json.",
      code: json({ mcpServers: { "h3-studio": { httpUrl: url } } }),
    },
    {
      title: "Any other agent",
      note: "Use the address when the agent can connect to a web address. Use the command when it can only start a program. An agent without MCP can read the OpenAPI file and call the REST API directly.",
      code: `MCP address (streamable HTTP): ${url}\nProgram (standard input and output): "${exe}" ${(data.stdio?.args || ["--mcp-stdio"]).join(" ")}\nREST API: ${data.api_url}\nOpenAPI file: ${data.openapi_url}`,
    },
  ];
}

function card(item) {
  const box = element("div", "border:1px solid #1e293b;border-radius:8px;padding:10px 12px;margin-bottom:10px;background:#0f172a;");
  const head = element("div", "display:flex;align-items:center;gap:8px;margin-bottom:4px;");
  head.append(element("div", "flex:1;font-weight:800;color:#e0f2fe;", item.title));
  const copy = button("Copy");
  copy.onclick = async () => {
    try {
      await navigator.clipboard.writeText(item.code);
      copy.textContent = "Copied";
    } catch {
      // The clipboard can be refused. The text is selectable below.
      copy.textContent = "Select and copy";
    }
    setTimeout(() => {
      copy.textContent = "Copy";
    }, 1800);
  };
  head.append(copy);
  box.append(head, element("div", "color:#94a3b8;font-size:12px;line-height:1.45;margin-bottom:6px;", item.note));
  const code = element("pre", "margin:0;padding:8px 10px;border-radius:6px;background:#020617;color:#bae6fd;font:12px Consolas,monospace;white-space:pre-wrap;overflow-wrap:anywhere;user-select:text;", item.code);
  box.append(code);
  return box;
}

export function openMcpSetup() {
  if (document.getElementById("h3-mcp-setup")) return;
  const shade = element("div", "position:fixed;inset:0;z-index:100010;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.6);");
  shade.id = "h3-mcp-setup";
  const panel = element("div", "width:min(780px,94vw);max-height:92vh;overflow:auto;box-sizing:border-box;border:1px solid #155e75;border-radius:10px;background:#0b1220;color:#e5e7eb;padding:18px;font:13px 'Segoe UI',Inter,Arial,sans-serif;box-shadow:0 18px 60px rgba(0,0,0,.6);");
  shade.append(panel);
  panel.append(element("div", "font-size:17px;font-weight:900;color:#f0fdfa;", "MCP Setup"));
  panel.append(element("div", "color:#94a3b8;margin:4px 0 12px;line-height:1.5;", "H3 Studio has an MCP server built in. Connect an AI agent to it and the agent can build a music video through the same Builder you see here: create projects, write the story and the prompts, render the scenes and stitch the video. Nothing else needs to be installed."));

  const status = element("div", "display:flex;align-items:center;gap:10px;flex-wrap:wrap;border:1px solid #1e293b;border-radius:8px;padding:9px 12px;margin-bottom:12px;background:#0f172a;");
  const left = element("div", "flex:1;min-width:220px;line-height:1.5;");
  const statusText = element("div", "color:#cbd5e1;", "Looking for the MCP server...");
  const resultText = element("div", "margin-top:2px;font-size:12px;color:#94a3b8;");
  left.append(statusText, resultText);
  const test = button("Test connection", true);
  status.append(left, test);
  panel.append(status);

  const list = element("div");
  panel.append(list);

  const notes = element("div", "color:#94a3b8;font-size:12px;line-height:1.55;margin:4px 0 12px;");
  notes.append(
    element("div", "", "Start here: ask the agent to use the prompt chat_make_music_video. It asks you for the song, the lyrics, the character and the look, then builds the video."),
    element("div", "margin-top:4px;", "Only programs on this computer can connect. An agent can create, change and delete projects and start renders. It never loads or switches your LLM. An ElevenLabs key saved in App Settings is used for the agent's speech calls and is never shown to it."),
  );
  panel.append(notes);

  const buttons = element("div", "display:flex;gap:8px;justify-content:flex-end;");
  const close = button("Close");
  buttons.append(close);
  panel.append(buttons);

  const closePanel = () => shade.remove();
  close.onclick = closePanel;
  shade.addEventListener("click", (event) => event.target === shade && closePanel());
  shade.addEventListener("keydown", (event) => event.key === "Escape" && closePanel());
  document.body.append(shade);

  let url = "";
  const run = async () => {
    if (!url) return;
    test.disabled = true;
    resultText.style.color = "#94a3b8";
    resultText.textContent = "Testing...";
    try {
      const found = await checkConnection(url);
      resultText.style.color = "#86efac";
      resultText.textContent = `Connected: ${found.name} answered and offers ${found.tools} tools.`;
    } catch (error) {
      resultText.style.color = "#fca5a5";
      resultText.textContent = `Not connected: ${String(error?.message || error)}`;
    } finally {
      test.disabled = false;
    }
  };
  test.onclick = run;

  info()
    .then((data) => {
      url = data.mcp_url;
      statusText.replaceChildren(
        element("div", "font-weight:800;color:#e0f2fe;", url),
        element("div", "color:#94a3b8;font-size:12px;", `${data.tools} tools, ${data.resources} resources, ${data.prompts} prompts. Server ${data.server_name} ${data.version}.`),
      );
      for (const item of clients(data)) list.append(card(item));
      run();
    })
    .catch((error) => {
      statusText.style.color = "#fca5a5";
      statusText.textContent = String(error?.message || error);
    });
}
