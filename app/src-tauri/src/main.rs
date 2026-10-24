// Prevents an additional console window on Windows in release. Do not remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `h3-studio --mcp-stdio` is the app's MCP server for an agent that can only start a program. It opens no window.
    if std::env::args().skip(1).any(|argument| argument == "--mcp-stdio") {
        std::process::exit(h3_api::mcp::bridge::run_stdio());
    }
    h3_studio_lib::run()
}
