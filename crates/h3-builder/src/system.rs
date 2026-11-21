//! Host readings for the Builder's header meters, and the custom node check. Ports of `_read_resources`,
//! `custom_nodes_status` and the node table in `core/system_routes.py`.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

const FIELDS: &str = "index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,fan.speed,clocks.gr,power.draw,power.limit";
const GPU_KEYS: [&str; 8] = ["load", "used", "total", "temperature", "fan", "clock", "power", "power_limit"];

/// `_number`: a finite, non negative number from nvidia-smi text, else `None`.
fn number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)
}

/// Ask `nvidia-smi` for the GPUs. Gives up after two seconds, and is empty when there is no NVIDIA driver.
fn read_gpus() -> Vec<Value> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut command = Command::new("nvidia-smi");
        command.arg(format!("--query-gpu={FIELDS}")).arg("--format=csv,noheader,nounits").stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let _ = sender.send(command.output());
    });
    let Ok(Ok(output)) = receiver.recv_timeout(Duration::from_secs(2)) else { return Vec::new() };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();
    for line in text.lines() {
        let row: Vec<&str> = line.split(',').map(str::trim).collect();
        if row.len() != 10 {
            continue;
        }
        let mut gpu = Map::new();
        for (key, value) in GPU_KEYS.iter().zip(&row[2..]) {
            // Memory is reported in MiB.
            let scaled = if matches!(*key, "used" | "total") { number(value).map(|n| n * 1024.0 * 1024.0) } else { number(value) };
            gpu.insert((*key).into(), scaled.map_or(Value::Null, |n| json!(n)));
        }
        gpu.insert("index".into(), json!(row[0]));
        gpu.insert("name".into(), json!(row[1]));
        gpus.push(Value::Object(gpu));
    }
    gpus
}

/// `GET /vrgdg/resource-monitor`: RAM and every NVIDIA GPU.
pub fn read_resources() -> Value {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total = system.total_memory();
    let used = total.saturating_sub(system.available_memory());
    let gpus = read_gpus();
    let status = if gpus.is_empty() { "unavailable" } else { "available" };
    json!({"ram": {"used": used, "total": total}, "gpus": gpus, "gpu_status": status})
}

/// The custom nodes the Builder depends on: `(id, install name, folder names)`. The first folder is where a GitHub clone
/// lands. Only these can be asked for by the browser, never an arbitrary path.
const CUSTOM_NODES: [(&str, &str, &[&str]); 9] = [
    ("videohelpersuite", "comfyui-videohelpersuite", &["ComfyUI-VideoHelperSuite", "comfyui-videohelpersuite"]),
    ("kjnodes", "comfyui-kjnodes", &["ComfyUI-KJNodes", "comfyui-kjnodes"]),
    ("euler_flow_matching", "erosdiffusion-eulerflowmatchingdiscretescheduler", &["erosdiffusion-eulerflowmatchingdiscretescheduler", "ComfyUI-EulerDiscreteScheduler", "ComfyUI-EulerFlowMatchingDiscreteScheduler"]),
    ("gguf", "ComfyUI-GGUF", &["ComfyUI-GGUF"]),
    ("ltxvideo", "ComfyUI-LTXVideo", &["ComfyUI-LTXVideo"]),
    ("te_speed_minimax_h3", "https://github.com/HELPMEEADICE/TE-Speed-MiniMaxH3-OSS.git", &["TE-Speed-MiniMaxH3-OSS"]),
    ("mmh3_ultimate_upscale", "Comfyui-MMH3-UltimateUpscale", &["Comfyui-MMH3-UltimateUpscale"]),
    ("minimax_h3_audio_t8", "https://github.com/T8mars/comfyui-minimax-h3-audio-T8.git", &["minimax-h3-audio-t8", "comfyui-minimax-h3-audio-T8"]),
    ("minimax_h3_latent_upscaler", "https://github.com/LBH-123-AI/Comfyui_Minimax_h3_latent_Upscaler.git", &["Comfyui_Minimax_h3_latent_Upscaler"]),
];

/// The one Python library listed with the nodes (`nvidia_vfx` in `core/system_routes.py`): not a custom node, it is installed with pip from
/// NVIDIA's index (plain PyPI only has a stub that fails to build) and needs no ComfyUI-Manager. `(id, pip name, index)`.
const NVIDIA_VFX: (&str, &str, &str) = (NVIDIA_VFX_ID, "nvidia-vfx", "https://pypi.nvidia.com");

/// The id of that library in the Custom Nodes window.
pub const NVIDIA_VFX_ID: &str = "nvidia_vfx";

/// A ComfyUI version as `(major, minor, patch)`: `"0.46.0"`, `"v0.44"` or `"0.44.0rc1"`. `None` when it is not a version.
fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let mut number = || -> Option<u32> { parts.next()?.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok() };
    let major = number()?;
    let minor = number()?;
    Some((major, minor, number().unwrap_or(0)))
}

/// The ComfyUI version that has this pack's nodes built in (`built_in_since` of `core/system_routes.py`), for the packs that have one.
fn built_in_since(id: &str) -> Option<(u32, u32, u32)> {
    (id == "minimax_h3_latent_upscaler").then_some((0, 44, 0))
}

/// True when ComfyUI `version` already has what pack `id` provides. An unknown version has nothing built in.
fn is_built_in(id: &str, version: Option<&str>) -> bool {
    matches!((built_in_since(id), version.and_then(parse_version)), (Some(since), Some(have)) if have >= since)
}

/// `GET /vrgdg/video_builder/custom_nodes/status` for the ComfyUI custom node folder `dir`, read by folder name. The app uses this only when
/// the VRGDG pack inside ComfyUI cannot answer (ComfyUI is down, or the pack is not installed in it). `manager_available` says whether the app
/// can install nodes itself with the ComfyUI-Manager in ComfyUI's Python (see [`manager_python`]), and `comfyui_version` (when ComfyUI told it)
/// decides which packs ComfyUI has built in. `nvidia_vfx_installed` is whether ComfyUI's Python imports `nvvfx` (see [`nvidia_vfx_installed`]).
pub fn custom_nodes_status(dir: Option<&Path>, comfyui_version: Option<&str>, manager_available: bool, nvidia_vfx_installed: bool) -> Value {
    let base: PathBuf = dir.map(Path::to_path_buf).unwrap_or_default();
    let mut nodes: Vec<Value> = CUSTOM_NODES
        .iter()
        .map(|(id, install, folders)| {
            let found = folders.iter().map(|f| base.join(f)).find(|p| p.is_dir());
            let path = found.clone().unwrap_or_else(|| base.join(folders[0]));
            let mut node = json!({
                "id": id,
                "folder": found.as_ref().and_then(|p| p.file_name()).map_or(folders[0].to_string(), |n| n.to_string_lossy().to_string()),
                "installed": found.is_some(),
                "path": path.display().to_string(),
                "source": if install.starts_with("https://") { "github" } else { "registry" },
                "install": install,
            });
            if let Some((major, minor, patch)) = built_in_since(id) {
                node["built_in"] = json!(is_built_in(id, comfyui_version));
                node["built_in_since"] = json!(format!("{major}.{minor}.{patch}"));
            }
            node
        })
        .collect();
    nodes.push(json!({"id": NVIDIA_VFX.0, "folder": "", "installed": nvidia_vfx_installed, "path": "", "source": "pip", "install": NVIDIA_VFX.1}));
    let mut status = json!({"ok": true, "custom_nodes_dir": base.display().to_string(), "manager_available": manager_available, "nodes": nodes});
    if let Some(version) = comfyui_version.and_then(parse_version) {
        status["comfyui_version"] = json!(format!("{}.{}.{}", version.0, version.1, version.2));
    }
    status
}

/// A command without a console window.
fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.stdin(Stdio::null())
}

/// Gets each line a command prints while it runs.
type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Read a child's pipe to its end on its own thread, so a full pipe never stops the child. Every line goes to `sink` as it comes. A progress
/// bar redraws its line with carriage returns, so only the part that is on screen is passed on.
fn drain<R: Read + Send + 'static>(pipe: Option<R>, sink: LineSink) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = pipe {
            for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line).into_owned();
                if let Some(shown) = line.rsplit('\r').map(str::trim).find(|part| !part.is_empty()) {
                    sink(shown);
                }
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    })
}

/// Run `command` to its end and give whether it succeeded and its output (stdout then stderr), or kill it after `timeout`. Each printed line
/// also goes to `sink` while the command runs.
fn run_with_timeout(command: &mut Command, timeout: Duration, sink: &LineSink) -> Result<(bool, String), String> {
    let mut child = quiet(command).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let out = drain(child.stdout.take(), Arc::clone(sink));
    let err = drain(child.stderr.take(), Arc::clone(sink));
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it did not finish in time".into());
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    let joined = [out.trim(), err.trim()].iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join("\n");
    Ok((status.success(), joined))
}

/// The Python programs ComfyUI may run with, best guess first: the portable build's `python_embeded` next to the ComfyUI folder, then a
/// `.venv` or `venv` in the ComfyUI folder or next to it (ComfyUI Desktop and manual installs). `custom_nodes` is ComfyUI's node folder.
fn python_candidates(custom_nodes: &Path) -> Vec<PathBuf> {
    let root = custom_nodes.parent();
    let outer = root.and_then(Path::parent);
    let embedded = [outer, root].into_iter().flatten().map(|base| base.join("python_embeded").join("python.exe"));
    let venvs = [".venv/Scripts/python.exe", "venv/Scripts/python.exe", ".venv/bin/python", "venv/bin/python"];
    embedded.chain([root, outer].into_iter().flatten().flat_map(|base| venvs.iter().map(move |venv| base.join(venv)))).collect()
}

/// A sink that drops every line.
fn no_sink() -> LineSink {
    Arc::new(|_: &str| {})
}

/// True when `python` runs `code` without an error.
fn python_runs(python: &Path, code: &str, timeout: Duration) -> bool {
    let mut command = Command::new(python);
    command.args(["-c", code]);
    matches!(run_with_timeout(&mut command, timeout, &no_sink()), Ok((true, _)))
}

/// True when `python` can import `cm_cli`, which is ComfyUI-Manager's command line.
fn has_cm_cli(python: &Path) -> bool {
    python_runs(python, "import importlib.util, sys; sys.exit(0 if importlib.util.find_spec('cm_cli') else 1)", Duration::from_secs(30))
}

/// True when `python` imports `nvvfx` (pip package `nvidia-vfx`), which the `nvidia_rtx_vsr` resize method of KJNodes needs.
pub fn nvidia_vfx_installed(python: &Path) -> bool {
    python_runs(python, "import nvvfx", Duration::from_secs(60))
}

/// The Python programs of the ComfyUI whose node folder is `custom_nodes` that exist, best guess first. `Err` says why there is none, in words for
/// the Custom Nodes window.
fn existing_pythons(custom_nodes: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let Some(dir) = custom_nodes else {
        return Err("ComfyUI did not say where it keeps its custom nodes, so nothing can be installed from here. Update ComfyUI and open this window again.".into());
    };
    let pythons: Vec<PathBuf> = python_candidates(dir).into_iter().filter(|p| p.is_file()).collect();
    if pythons.is_empty() {
        return Err(format!(
            "H3 Studio could not find the Python that runs ComfyUI (it looks for python_embeded, .venv and venv next to {}), so nodes cannot be installed from here. Install the nodes with ComfyUI-Manager, then restart ComfyUI.",
            dir.parent().unwrap_or(dir).display()
        ));
    }
    Ok(pythons)
}

/// The Python of the ComfyUI whose node folder is `custom_nodes`, for what needs no ComfyUI-Manager (a pip library). `Err` says why there is none.
pub fn comfy_python(custom_nodes: Option<&Path>) -> Result<PathBuf, String> {
    existing_pythons(custom_nodes).map(|pythons| pythons[0].clone())
}

/// The Python of the ComfyUI whose node folder is `custom_nodes`, when it has ComfyUI-Manager. The text of `Err` says why the app cannot install
/// nodes itself, in words for the Custom Nodes window.
pub fn manager_python(custom_nodes: Option<&Path>) -> Result<PathBuf, String> {
    let pythons = existing_pythons(custom_nodes)?;
    pythons.iter().find(|python| has_cm_cli(python)).cloned().ok_or_else(|| {
        "ComfyUI-Manager is not installed in ComfyUI's Python, so nodes cannot be installed from here. Start ComfyUI with --enable-manager, or run pip install -r manager_requirements.txt from the ComfyUI folder, then restart ComfyUI.".to_string()
    })
}

/// How far the install the app runs itself has come: the Custom Nodes window reads it while the install request is still open.
#[derive(Default)]
struct Progress {
    running: bool,
    total: usize,
    /// The pack being installed, since when, and the last line `cm_cli` printed for it.
    current: Option<(String, Instant)>,
    line: String,
    done: Vec<String>,
    failed: Vec<String>,
}

type SharedProgress = Arc<Mutex<Progress>>;

static PROGRESS: LazyLock<SharedProgress> = LazyLock::new(SharedProgress::default);

fn update(progress: &SharedProgress, change: impl FnOnce(&mut Progress)) {
    change(&mut progress.lock().unwrap_or_else(|e| e.into_inner()));
}

/// What a second install gets while one runs.
pub const INSTALL_RUNNING: &str = "An install is already running. Wait for it to finish.";

/// `GET /vrgdg/video_builder/custom_nodes/install/progress`: what the running install has done, or what the last one did.
pub fn install_progress() -> Value {
    let p = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
    json!({
        "ok": true, "running": p.running, "total": p.total, "line": p.line, "done": p.done, "failed": p.failed,
        "current": p.current.as_ref().map(|(id, _)| id), "current_seconds": p.current.as_ref().map(|(_, since)| since.elapsed().as_secs()),
    })
}

/// Marks the install as ended, however it ends.
struct Running(SharedProgress);

impl Drop for Running {
    fn drop(&mut self) {
        update(&self.0, |p| {
            p.running = false;
            p.current = None;
        });
    }
}

/// The last `max` characters of `text`.
fn last_chars(text: &str, max: usize) -> String {
    let skip = text.chars().count().saturating_sub(max);
    text.chars().skip(skip).collect()
}

/// A sink that keeps the last line a command printed in `progress`, for the Custom Nodes window.
fn progress_sink(progress: &SharedProgress) -> LineSink {
    let shared = Arc::clone(progress);
    Arc::new(move |line| update(&shared, |p| p.line = line.chars().take(300).collect()))
}

/// `cm_cli` of ComfyUI-Manager, run in ComfyUI's Python the way the pack inside ComfyUI runs it: from the ComfyUI folder, with `COMFYUI_PATH` set.
/// What it prints is kept as the last line in `progress`.
fn run_cm_cli(python: &Path, comfy_root: &Path, args: &[&str], progress: &SharedProgress) -> Result<String, String> {
    let mut command = Command::new(python);
    command.args(["-m", "cm_cli"]).args(args).current_dir(comfy_root).env("COMFYUI_PATH", comfy_root).env("PYTHONIOENCODING", "utf-8");
    let (ok, output) = run_with_timeout(&mut command, Duration::from_secs(1800), &progress_sink(progress)).map_err(|e| format!("cm_cli {}: {e}", args.join(" ")))?;
    if ok {
        Ok(output)
    } else {
        let tail = last_chars(&output, 2000);
        Err(format!("cm_cli {} failed:\n{}", args.join(" "), if tail.is_empty() { "No output." } else { &tail }))
    }
}

/// `pip install nvidia-vfx` in ComfyUI's Python, as the pack does it: from NVIDIA's index, wheels only (the source download on plain PyPI is a
/// stub). Done when ComfyUI's Python imports `nvvfx` afterwards.
fn install_nvidia_vfx(python: &Path, progress: &SharedProgress) -> Result<(&'static str, String), String> {
    if nvidia_vfx_installed(python) {
        return Ok(("already installed", String::new()));
    }
    let (_, name, index) = NVIDIA_VFX;
    let mut command = Command::new(python);
    command.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input", "--only-binary", name, name, "--extra-index-url", index]);
    command.env("PYTHONIOENCODING", "utf-8");
    let (ok, output) = run_with_timeout(&mut command, Duration::from_secs(3600), &progress_sink(progress)).map_err(|e| format!("pip install {name}: {e}"))?;
    if !ok {
        return Err(format!("pip install {name} failed:\n{}", last_chars(&output, 2000)));
    }
    if !nvidia_vfx_installed(python) {
        return Err(format!("pip finished, but {name} still does not import:\n{}", last_chars(&output, 2000)));
    }
    Ok(("installed", output))
}

/// `POST /vrgdg/video_builder/custom_nodes/install` without the pack: install the packs `ids` into the ComfyUI whose node folder is
/// `custom_nodes`, with ComfyUI-Manager in `python`. A pack that is already there gets its requirements reinstalled, and one the ComfyUI version
/// has built in is left alone. `nvidia_vfx` is a pip library, installed with `python` alone (no ComfyUI-Manager). Every pack is tried, so one that
/// fails does not stop the others, and the failures come back together as `Err`. [`install_progress`] tells how far it is. Only one install runs
/// at a time ([`INSTALL_RUNNING`]).
pub fn install_custom_nodes(python: &Path, custom_nodes: &Path, ids: &[String], comfyui_version: Option<&str>) -> Result<Vec<Value>, String> {
    install_with(&PROGRESS, python, custom_nodes, ids, comfyui_version)
}

fn install_with(progress: &SharedProgress, python: &Path, custom_nodes: &Path, ids: &[String], comfyui_version: Option<&str>) -> Result<Vec<Value>, String> {
    let requested: Vec<&str> = ids.iter().map(String::as_str).filter(|id| *id == NVIDIA_VFX.0 || CUSTOM_NODES.iter().any(|(known, _, _)| known == id)).collect();
    if requested.is_empty() {
        return Err("No recognized Video Builder custom nodes were selected.".into());
    }
    let mut busy = false;
    update(progress, |p| {
        busy = p.running;
        if !busy {
            *p = Progress { running: true, total: requested.len(), ..Progress::default() };
        }
    });
    if busy {
        return Err(INSTALL_RUNNING.into());
    }
    let _running = Running(Arc::clone(progress));
    let comfy_root = custom_nodes.parent().unwrap_or(custom_nodes);
    let installed_at = |folders: &[&str]| folders.iter().map(|f| custom_nodes.join(f)).find(|p| p.is_dir());
    let (mut results, mut failures) = (Vec::new(), Vec::new());
    for id in requested {
        eprintln!("[VRGDG-RS Custom Nodes] {id}: starting");
        update(progress, |p| {
            p.current = Some((id.to_string(), Instant::now()));
            p.line.clear();
        });
        let (folder, outcome) = match CUSTOM_NODES.iter().find(|(known, _, _)| *known == id) {
            None => ("", install_nvidia_vfx(python, progress)),
            Some((_, install, folders)) => (
                folders[0],
                if is_built_in(id, comfyui_version) {
                    Ok(("built into ComfyUI", String::new()))
                } else if let Some(path) = installed_at(folders) {
                    run_cm_cli(python, comfy_root, &["post-install", &path.display().to_string()], progress).map(|output| ("requirements reinstalled", output))
                } else {
                    run_cm_cli(python, comfy_root, &["install", install, "--exit-on-fail"], progress).and_then(|output| match installed_at(folders) {
                        Some(_) => Ok(("installed", output)),
                        None => Err(format!("ComfyUI-Manager finished, but no {} folder was created:\n{output}", folders[0])),
                    })
                },
            ),
        };
        match outcome {
            Ok((action, output)) => {
                eprintln!("[VRGDG-RS Custom Nodes] {id}: {action}");
                update(progress, |p| p.done.push(id.to_string()));
                results.push(json!({"id": id, "folder": folder, "action": action, "output": last_chars(&output, 1200)}));
            }
            Err(message) => {
                eprintln!("[VRGDG-RS Custom Nodes] {id}: failed: {message}");
                update(progress, |p| p.failed.push(id.to_string()));
                failures.push(format!("{id}: {message}"));
            }
        }
    }
    if failures.is_empty() {
        Ok(results)
    } else {
        let done = if results.is_empty() { String::new() } else { format!(" {} installed.", results.len()) };
        Err(format!("{} of {} custom nodes could not be installed.{done}\n{}", failures.len(), failures.len() + results.len(), failures.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_have_ram_and_a_gpu_status() {
        let value = read_resources();
        assert!(value["ram"]["total"].as_u64().unwrap() > 0);
        assert!(matches!(value["gpu_status"].as_str(), Some("available") | Some("unavailable")));
    }

    #[test]
    fn versions_are_read_like_the_pack_reads_them() {
        assert_eq!(parse_version("0.46.0"), Some((0, 46, 0)));
        assert_eq!(parse_version("v0.44"), Some((0, 44, 0)));
        assert_eq!(parse_version("0.44.0rc1"), Some((0, 44, 0)));
        assert_eq!(parse_version("nightly"), None);
        assert!(is_built_in("minimax_h3_latent_upscaler", Some("0.44.0")) && is_built_in("minimax_h3_latent_upscaler", Some("0.46.0")));
        assert!(!is_built_in("minimax_h3_latent_upscaler", Some("0.43.9")) && !is_built_in("minimax_h3_latent_upscaler", None));
        assert!(!is_built_in("gguf", Some("0.46.0")));
    }

    #[test]
    fn the_status_marks_what_the_comfyui_version_has_built_in() {
        let status = custom_nodes_status(None, Some("0.46.0"), true, false);
        let upscaler = status["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "minimax_h3_latent_upscaler").unwrap().clone();
        assert_eq!((upscaler["built_in"].clone(), upscaler["built_in_since"].clone()), (json!(true), json!("0.44.0")));
        assert_eq!((status["manager_available"].clone(), status["comfyui_version"].clone()), (json!(true), json!("0.46.0")));
        let older = custom_nodes_status(None, Some("0.43.0"), false, false);
        assert_eq!(older["nodes"][8]["built_in"], false);
        assert!(older.get("comfyui_version").is_some() && custom_nodes_status(None, None, false, false).get("comfyui_version").is_none());
    }

    #[test]
    fn the_rtx_video_super_resolution_library_is_listed_as_a_pip_library() {
        let missing = custom_nodes_status(None, None, false, false);
        let row = missing["nodes"].as_array().unwrap().last().unwrap().clone();
        assert_eq!((row["id"].clone(), row["source"].clone(), row["install"].clone(), row["installed"].clone()), (json!("nvidia_vfx"), json!("pip"), json!("nvidia-vfx"), json!(false)));
        assert_eq!(custom_nodes_status(None, None, false, true)["nodes"][9]["installed"], true);
    }

    #[test]
    fn the_python_is_looked_for_next_to_comfyui() {
        let dir = tempfile::tempdir().unwrap();
        let custom_nodes = dir.path().join("ComfyUI").join("custom_nodes");
        std::fs::create_dir_all(&custom_nodes).unwrap();
        let candidates = python_candidates(&custom_nodes);
        assert_eq!(candidates[0], dir.path().join("python_embeded").join("python.exe"), "the portable build is tried first");
        assert!(candidates.contains(&dir.path().join("ComfyUI").join(".venv").join("Scripts").join("python.exe")));
        // No Python anywhere: the answer says where it looked.
        let note = manager_python(Some(&custom_nodes)).unwrap_err();
        assert!(note.contains("could not find the Python that runs ComfyUI"), "{note}");
        assert!(manager_python(None).unwrap_err().contains("did not say where it keeps its custom nodes"));
    }

    #[test]
    fn installing_asks_for_known_packs_only() {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python.exe");
        let progress = SharedProgress::default();
        let error = install_with(&progress, &python, &dir.path().join("custom_nodes"), &["../evil".to_string()], None).unwrap_err();
        assert_eq!(error, "No recognized Video Builder custom nodes were selected.");
        assert!(!progress.lock().unwrap().running, "a refused request starts nothing");
        // A pack ComfyUI has built in needs no Python at all.
        let done = install_with(&progress, &python, &dir.path().join("custom_nodes"), &["minimax_h3_latent_upscaler".to_string()], Some("0.46.0")).unwrap();
        assert_eq!(done[0]["action"], "built into ComfyUI");
        let p = progress.lock().unwrap();
        assert_eq!((p.running, p.total, p.done.clone(), p.failed.len()), (false, 1, vec!["minimax_h3_latent_upscaler".to_string()], 0));
    }

    #[test]
    fn a_pack_that_cannot_be_installed_is_counted_as_failed_and_the_others_still_run() {
        let dir = tempfile::tempdir().unwrap();
        let progress = SharedProgress::default();
        // No Python at that path: `gguf` fails, the built in pack after it still goes through.
        let ids = ["gguf".to_string(), "minimax_h3_latent_upscaler".to_string()];
        let error = install_with(&progress, &dir.path().join("none.exe"), &dir.path().join("custom_nodes"), &ids, Some("0.46.0")).unwrap_err();
        assert!(error.starts_with("1 of 2 custom nodes could not be installed. 1 installed.\ngguf: "), "{error}");
        let p = progress.lock().unwrap();
        assert_eq!((p.done.clone(), p.failed.clone(), p.running), (vec!["minimax_h3_latent_upscaler".to_string()], vec!["gguf".to_string()], false));
    }

    #[test]
    fn the_pip_library_is_asked_for_by_its_id_and_a_missing_python_fails_it_alone() {
        let dir = tempfile::tempdir().unwrap();
        let progress = SharedProgress::default();
        let ids = [NVIDIA_VFX_ID.to_string(), "minimax_h3_latent_upscaler".to_string()];
        let error = install_with(&progress, &dir.path().join("none.exe"), &dir.path().join("custom_nodes"), &ids, Some("0.46.0")).unwrap_err();
        assert!(error.starts_with("1 of 2 custom nodes could not be installed. 1 installed.
nvidia_vfx: pip install nvidia-vfx"), "{error}");
        let p = progress.lock().unwrap();
        assert_eq!((p.failed.clone(), p.done.clone(), p.total), (vec!["nvidia_vfx".to_string()], vec!["minimax_h3_latent_upscaler".to_string()], 2));
    }

    #[test]
    fn a_second_install_is_refused_while_one_runs() {
        let dir = tempfile::tempdir().unwrap();
        let progress = SharedProgress::default();
        update(&progress, |p| p.running = true);
        let error = install_with(&progress, &dir.path().join("python.exe"), &dir.path().join("custom_nodes"), &["gguf".to_string()], None).unwrap_err();
        assert_eq!(error, INSTALL_RUNNING);
        assert!(progress.lock().unwrap().running, "the install that runs is left alone");
    }

    #[test]
    fn what_a_command_prints_reaches_the_sink_line_by_line() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let collected = Arc::clone(&seen);
        let sink: LineSink = Arc::new(move |line| collected.lock().unwrap().push(line.to_string()));
        // The test program itself lists its tests, one per line.
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.arg("--list");
        let (ok, output) = run_with_timeout(&mut command, Duration::from_secs(60), &sink).unwrap();
        assert!(ok);
        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|line| line.contains("a_second_install_is_refused_while_one_runs")), "{seen:?}");
        assert!(output.contains("a_second_install_is_refused_while_one_runs"));
    }

    #[test]
    fn the_progress_bar_part_of_a_line_is_what_is_shown() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let collected = Arc::clone(&seen);
        let sink: LineSink = Arc::new(move |line| collected.lock().unwrap().push(line.to_string()));
        let text = drain(Some(std::io::Cursor::new(b"0%|   |\r50%|## |\r100%|###|\nnext\n\n".to_vec())), sink).join().unwrap();
        assert_eq!(seen.lock().unwrap().clone(), vec!["100%|###|".to_string(), "next".to_string()]);
        assert!(text.contains("next"));
    }

    #[test]
    fn custom_nodes_are_found_by_folder_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("comfyui-kjnodes")).unwrap();
        let status = custom_nodes_status(Some(dir.path()), None, false, false);
        let nodes = status["nodes"].as_array().unwrap();
        let kj = nodes.iter().find(|n| n["id"] == "kjnodes").unwrap();
        assert_eq!(kj["installed"], true);
        // Windows file names ignore case, so the first listed spelling is the one that matches.
        assert!(kj["folder"].as_str().unwrap().eq_ignore_ascii_case("comfyui-kjnodes"));
        let gguf = nodes.iter().find(|n| n["id"] == "gguf").unwrap();
        assert_eq!(gguf["installed"], false);
        assert_eq!(gguf["folder"], "ComfyUI-GGUF");
        assert_eq!(gguf["source"], "registry");
    }
}
