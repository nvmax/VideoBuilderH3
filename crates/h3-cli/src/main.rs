//! `h3` command line.

mod render_cmd;

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
    /// Serve the local /api/v1 for agents and the MCP server.
    Serve {
        /// Address to listen on. Defaults to api_bind in the config.
        #[arg(long)]
        bind: Option<String>,
        /// Folder that holds the projects. Defaults to project_root in the config.
        #[arg(long)]
        project_root: Option<PathBuf>,
    },
    /// List the scenes of a project.
    Scenes {
        /// Project folder (the one with vrgdg_builder_session.json).
        #[arg(long)]
        project: PathBuf,
    },
    /// Render one scene in ComfyUI and find the clip it made.
    Render {
        #[arg(long)]
        project: PathBuf,
        /// Scene number (1 based) or scene id.
        #[arg(long)]
        scene: String,
        /// Override a render setting for this run, for example `--set seed=7` or `--set prompt="..."`.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Build the graph and report it without sending it to ComfyUI.
        #[arg(long)]
        dry_run: bool,
        /// Stop after ComfyUI renders: do not trim, collect or record the clip.
        #[arg(long)]
        raw_only: bool,
        /// Match the clip's opening color to the previous scene's clip.
        #[arg(long)]
        match_start_color: bool,
        /// Write the ComfyUI graph to this file.
        #[arg(long)]
        graph_out: Option<PathBuf>,
        /// Give up waiting after this many hours.
        #[arg(long)]
        timeout_hours: Option<f64>,
    },
    /// Render several scenes in order, then stitch the project.
    RenderAll {
        #[arg(long)]
        project: PathBuf,
        /// `all`, `missing` (scenes without a clip) or `selected`.
        #[arg(long, default_value = "all")]
        scope: String,
        /// Only these scenes (numbers or ids).
        #[arg(long = "scene")]
        scenes: Vec<String>,
        /// Only scenes FIRST-LAST, for example 3-8.
        #[arg(long)]
        range: Option<String>,
        #[arg(long)]
        force: bool,
        /// Do not stitch the final video afterwards.
        #[arg(long)]
        skip_stitch: bool,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        #[arg(long)]
        match_start_color: bool,
        #[arg(long)]
        timeout_hours: Option<f64>,
    },
    /// Join the project's scene clips with the song into one video.
    Stitch {
        #[arg(long)]
        project: PathBuf,
        /// Only these scenes (numbers or ids), for a preview.
        #[arg(long = "scene")]
        scenes: Vec<String>,
        /// `auto`, `embedded` (the scenes' own audio) or `project` (the song).
        #[arg(long, default_value = "auto")]
        audio: String,
        #[arg(long)]
        audio_path: Option<PathBuf>,
        /// File name prefix of the result.
        #[arg(long)]
        prefix: Option<String>,
    },
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

async fn serve(cfg: AppConfig, config_path: &std::path::Path) -> Result<()> {
    let state = h3_api::build_state(cfg, &h3_api::jobs_db_path(config_path)).map_err(|e| anyhow::anyhow!(e))?;
    h3_api::serve(state).await?;
    Ok(())
}

fn finish(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[VRGDG-RS] {e:#}");
            ExitCode::FAILURE
        }
    }
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
        Commands::Serve { bind, project_root } => {
            let mut cfg = cfg;
            if let Some(bind) = bind {
                cfg.api_bind = bind;
            }
            if let Some(root) = project_root {
                cfg.project_root = root;
            }
            finish(serve(cfg, &path).await)
        }
        Commands::Scenes { project } => finish(render_cmd::scenes(&project)),
        Commands::Render { project, scene, set, dry_run, raw_only, match_start_color, graph_out, timeout_hours } => finish(
            render_cmd::render(&cfg, render_cmd::RenderArgs { project, scene, set, dry_run, raw_only, match_start_color, graph_out, timeout_hours }).await,
        ),
        Commands::RenderAll { project, scope, scenes, range, force, skip_stitch, set, match_start_color, timeout_hours } => finish(
            render_cmd::render_all_cmd(&cfg, render_cmd::BatchArgs { project, scope, scenes, range, force, skip_stitch, set, match_start_color, timeout_hours }).await,
        ),
        Commands::Stitch { project, scenes, audio, audio_path, prefix } => {
            finish(render_cmd::stitch(&cfg, render_cmd::StitchArgs { project, scenes, audio, audio_path, prefix }).await)
        }
    }
}
