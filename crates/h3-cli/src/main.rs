//! `h3` command line.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::Result;
use clap::{Parser, Subcommand};
use h3_comfy::{check_nodes, ComfyClient};
use h3_project::AppConfig;

#[derive(Parser)]
#[command(name = "h3", version, about = "VRGDG H3 Studio command line")]
struct Cli {
    /// Config file. Defaults to %APPDATA%\VRGDG-H3\config.json.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// ComfyUI URL for this run, overriding the config.
    #[arg(long, global = true)]
    comfy_url: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check ComfyUI, the required nodes, ffmpeg and ffprobe.
    Doctor,
    /// Print the config in use and where it is stored.
    Config,
}

fn load_config(cli: &Cli) -> Result<(PathBuf, AppConfig)> {
    let path = cli.config.clone().unwrap_or_else(AppConfig::default_path);
    let mut cfg = AppConfig::load(&path)?;
    if let Some(url) = &cli.comfy_url {
        cfg.comfy_url = url.clone();
    }
    Ok((path, cfg))
}

fn tool_version(exe: &str) -> Option<String> {
    let out = Command::new(exe).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().map(|l| l.chars().take(80).collect())
}

async fn doctor(cfg: &AppConfig) -> bool {
    let mut ok = true;
    let client = ComfyClient::new(cfg.comfy_base());
    match client.system_stats().await {
        Ok(stats) => {
            println!("[ok]   ComfyUI {} at {}", stats.system.comfyui_version, client.base_url());
            for d in &stats.devices {
                println!(
                    "       {} ({:.1} GB VRAM, {:.1} GB free)",
                    d.name,
                    d.vram_total as f64 / 1e9,
                    d.vram_free as f64 / 1e9
                );
            }
        }
        Err(e) => {
            println!("[fail] {e}");
            return false;
        }
    }
    match client.object_info().await {
        Ok(info) => {
            let report = check_nodes(&info);
            if report.core_ready() {
                println!("[ok]   All core H3 nodes installed ({} node classes in ComfyUI)", report.installed_count);
            } else {
                ok = false;
                println!("[fail] Core H3 nodes missing");
            }
            for m in &report.missing {
                println!("       missing {} from {} (needed for {})", m.class_type, m.provider, m.feature.label());
            }
        }
        Err(e) => {
            ok = false;
            println!("[fail] {e}");
        }
    }
    for (name, exe) in [("ffmpeg", &cfg.ffmpeg_path), ("ffprobe", &cfg.ffprobe_path)] {
        match tool_version(exe) {
            Some(v) => println!("[ok]   {name}: {v}"),
            None => {
                ok = false;
                println!("[fail] {name} not found at '{exe}'. Install it or set {name}_path in the config.");
            }
        }
    }
    ok
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let (path, cfg) = match load_config(&cli) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[VRGDG-RS] {e}");
            return ExitCode::FAILURE;
        }
    };
    match cli.command {
        Commands::Doctor => {
            if doctor(&cfg).await {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Commands::Config => {
            println!("{}", path.display());
            println!("{cfg:#?}");
            ExitCode::SUCCESS
        }
    }
}
