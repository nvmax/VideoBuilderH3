//! `h3-studio --mcp-stdio`: the app's MCP server for an agent that can only start a program (Claude Desktop, for one). The agent talks to this process
//! over standard input and output, one JSON-RPC message per line, and every message is passed to the running app's `/mcp` address. When the app is not
//! running it is started first. Nothing but JSON messages is ever written to standard output. Messages for the user go to standard error.

use std::sync::Arc;
use std::time::{Duration, Instant};

use h3_project::AppConfig;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// How long a started app gets to open its address.
const LAUNCH_WAIT: Duration = Duration::from_secs(90);

/// Where the app's MCP server listens: `H3_MCP_URL`, else the address in the app's config file.
pub fn target_url() -> String {
    if let Some(url) = std::env::var("H3_MCP_URL").ok().filter(|u| !u.trim().is_empty()) {
        return url.trim().to_string();
    }
    let bind = AppConfig::load(&AppConfig::default_path()).map(|c| c.api_bind).unwrap_or_else(|_| "127.0.0.1:8190".into());
    let (host, port) = bind.rsplit_once(':').unwrap_or(("127.0.0.1", "8190"));
    let host = if matches!(host, "0.0.0.0" | "" | "[::]") { "127.0.0.1" } else { host };
    format!("http://{host}:{port}/mcp")
}

/// `host:port` of an address, for checking whether anything listens there.
fn socket_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or("127.0.0.1:8190").to_string()
}

async fn listening(socket: &str) -> bool {
    matches!(tokio::time::timeout(Duration::from_millis(800), tokio::net::TcpStream::connect(socket)).await, Ok(Ok(_)))
}

/// Start the app (this same program, without the flag) apart from this process, then wait for its address to answer.
async fn start_app(socket: &str, launch: &Mutex<()>) -> Result<(), String> {
    let _only_one = launch.lock().await;
    if listening(socket).await {
        return Ok(());
    }
    let program = std::env::current_exe().map_err(|e| format!("The app's file could not be found: {e}"))?;
    eprintln!("[VRGDG-RS MCP] H3 Studio is not running. Starting it...");
    let mut command = std::process::Command::new(program);
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Detached, so closing the agent does not close the app.
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so closing the agent does not close the app.
        command.process_group(0);
    }
    command.spawn().map_err(|e| format!("H3 Studio could not be started: {e}"))?;
    let until = Instant::now() + LAUNCH_WAIT;
    while Instant::now() < until {
        if listening(socket).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("H3 Studio was started but did not open its address in time. Open the app and try again.".into())
}

/// The server's answer to one message (`None` when it sends none), starting the app once when nobody answers.
async fn post(client: &reqwest::Client, url: &str, line: &str, launch: &Mutex<()>) -> Result<Option<String>, String> {
    for attempt in 0..2 {
        let sent = client.post(url).header("content-type", "application/json").body(line.to_string()).send().await;
        match sent {
            Ok(response) => {
                if response.status().as_u16() == 202 {
                    return Ok(None);
                }
                let body = response.text().await.map_err(|e| format!("The reply could not be read: {e}"))?;
                return Ok(Some(body));
            }
            Err(error) if error.is_connect() && attempt == 0 => start_app(&socket_of(url), launch).await?,
            Err(error) => return Err(format!("H3 Studio did not answer at {url}: {error}")),
        }
    }
    Err("H3 Studio did not answer.".into())
}

async fn forward(client: reqwest::Client, url: String, line: String, out: Arc<Mutex<tokio::io::Stdout>>, launch: Arc<Mutex<()>>) {
    let id = serde_json::from_str::<Value>(&line).ok().and_then(|m| m.get("id").cloned()).filter(|id| !id.is_null());
    let reply = match post(&client, &url, &line, &launch).await {
        Ok(Some(body)) => Some(body.trim().replace(['\r', '\n'], " ")),
        Ok(None) => None,
        // A request is answered with the failure. A notification has nobody to tell.
        Err(message) => id.map(|id| json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": message}}).to_string()),
    };
    if let Some(reply) = reply.filter(|r| !r.is_empty()) {
        let mut stdout = out.lock().await;
        let _ = stdout.write_all(format!("{reply}\n").as_bytes()).await;
        let _ = stdout.flush().await;
    }
}

async fn bridge() -> i32 {
    let url = target_url();
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(3700)).build() else {
        eprintln!("[VRGDG-RS MCP] The network client could not be made.");
        return 1;
    };
    eprintln!("[VRGDG-RS MCP] Passing messages to {url}");
    let out = Arc::new(Mutex::new(tokio::io::stdout()));
    let launch = Arc::new(Mutex::new(()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut running = tokio::task::JoinSet::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim_start_matches('\u{feff}').trim().to_string();
        if line.is_empty() {
            continue;
        }
        // Each message on its own, so a long job_wait does not hold up the next call.
        running.spawn(forward(client.clone(), url.clone(), line, Arc::clone(&out), Arc::clone(&launch)));
    }
    while running.join_next().await.is_some() {}
    0
}

/// Run the bridge until the agent closes standard input. The process exit code.
pub fn run_stdio() -> i32 {
    match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(bridge()),
        Err(e) => {
            eprintln!("[VRGDG-RS MCP] The bridge could not start: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_comes_from_the_url() {
        assert_eq!(socket_of("http://127.0.0.1:8190/mcp"), "127.0.0.1:8190");
        assert_eq!(socket_of("http://localhost:9000"), "localhost:9000");
    }
}
