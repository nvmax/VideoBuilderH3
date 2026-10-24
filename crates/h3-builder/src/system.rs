//! Host readings for the Builder's header meters, and the custom node check. Ports of `_read_resources`,
//! `custom_nodes_status` and the node table in `core/system_routes.py`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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
const CUSTOM_NODES: [(&str, &str, &[&str]); 8] = [
    ("videohelpersuite", "comfyui-videohelpersuite", &["ComfyUI-VideoHelperSuite", "comfyui-videohelpersuite"]),
    ("kjnodes", "comfyui-kjnodes", &["ComfyUI-KJNodes", "comfyui-kjnodes"]),
    ("gguf", "ComfyUI-GGUF", &["ComfyUI-GGUF"]),
    ("ltxvideo", "ComfyUI-LTXVideo", &["ComfyUI-LTXVideo"]),
    ("te_speed_minimax_h3", "https://github.com/HELPMEEADICE/TE-Speed-MiniMaxH3-OSS.git", &["TE-Speed-MiniMaxH3-OSS"]),
    ("mmh3_ultimate_upscale", "Comfyui-MMH3-UltimateUpscale", &["Comfyui-MMH3-UltimateUpscale"]),
    ("minimax_h3_audio_t8", "https://github.com/T8mars/comfyui-minimax-h3-audio-T8.git", &["comfyui-minimax-h3-audio-T8"]),
    ("minimax_h3_latent_upscaler", "https://github.com/LBH-123-AI/Comfyui_Minimax_h3_latent_Upscaler.git", &["Comfyui_Minimax_h3_latent_Upscaler"]),
];

/// `GET /vrgdg/video_builder/custom_nodes/status` for the ComfyUI custom node folder `dir`. Installing is not offered
/// from this app, so ComfyUI-Manager is reported as not available.
pub fn custom_nodes_status(dir: Option<&Path>) -> Value {
    let base: PathBuf = dir.map(Path::to_path_buf).unwrap_or_default();
    let nodes: Vec<Value> = CUSTOM_NODES
        .iter()
        .map(|(id, install, folders)| {
            let found = folders.iter().map(|f| base.join(f)).find(|p| p.is_dir());
            let path = found.clone().unwrap_or_else(|| base.join(folders[0]));
            json!({
                "id": id,
                "folder": found.as_ref().and_then(|p| p.file_name()).map_or(folders[0].to_string(), |n| n.to_string_lossy().to_string()),
                "installed": found.is_some(),
                "path": path.display().to_string(),
                "source": if install.starts_with("https://") { "github" } else { "registry" },
                "install": install,
            })
        })
        .collect();
    json!({"ok": true, "custom_nodes_dir": base.display().to_string(), "manager_available": false, "nodes": nodes})
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
    fn custom_nodes_are_found_by_folder_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("comfyui-kjnodes")).unwrap();
        let status = custom_nodes_status(Some(dir.path()));
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
