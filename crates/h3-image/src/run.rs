//! Run a preset in ComfyUI and find the images it made.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use h3_comfy::{extract_images, ComfyClient, ComfyDirs};
use h3_core::pyval::Obj;
use serde_json::Value;

use crate::preset::{apply_preset, class_types, ApplyCtx, ImagePreset};
use crate::ImageError;

/// What a generation made.
#[derive(Debug, Clone)]
pub struct Generated {
    pub prompt_id: String,
    /// Files ComfyUI saved, in the order it reported them. The last one is the usual pick.
    pub images: Vec<PathBuf>,
    pub seed: Option<u64>,
}

/// A step of a generation, for job progress.
#[derive(Debug, Clone)]
pub struct ImageEvent {
    pub percent: f64,
    pub stage: &'static str,
    pub message: String,
}

fn random_u64() -> u64 {
    // A seed only has to differ between runs. The clock's nanoseconds mixed through a multiply do that.
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()) as u64;
    nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (nanos >> 17)
}

/// Class types the workflow uses that ComfyUI does not have, so a missing custom node is named before queueing.
pub fn missing_nodes(preset: &ImagePreset, object_info: &Value) -> Vec<String> {
    let Some(known) = object_info.as_object().filter(|m| !m.is_empty()) else { return vec![] };
    class_types(&preset.workflow).into_iter().filter(|c| !known.contains_key(c)).collect()
}

/// Fill the preset, queue it, wait for it and return the saved files.
#[allow(clippy::too_many_arguments)]
pub async fn generate(
    client: &ComfyClient,
    dirs: &ComfyDirs,
    object_info: &Value,
    preset: &ImagePreset,
    values: &Obj,
    timeout: Duration,
    cancel: &Arc<AtomicBool>,
    mut on_event: impl FnMut(ImageEvent),
) -> Result<Generated, ImageError> {
    let missing = missing_nodes(preset, object_info);
    if !missing.is_empty() {
        return Err(ImageError::MissingNodes(missing));
    }
    on_event(ImageEvent { percent: 10.0, stage: "compiling_graph", message: format!("Compiling the {} graph...", preset.name) });
    let mut ctx = ApplyCtx { input_dir: PathBuf::from(&dirs.input), random: Box::new(random_u64) };
    let applied = apply_preset(preset, values, &mut ctx)?;
    let queued = client.queue_prompt(&applied.graph).await?;
    on_event(ImageEvent { percent: 30.0, stage: "generating", message: format!("Generating with ComfyUI (prompt {})...", queued.prompt_id) });
    let history = client.wait_for_prompt(&queued.prompt_id, timeout, Duration::from_millis(500), cancel, |_| {}).await?;
    let infos = extract_images(&history, &queued.prompt_id, &preset.output_nodes);
    let images: Vec<PathBuf> = infos.iter().map(|i| PathBuf::from(dirs.resolve_output(i))).filter(|p| p.is_file()).collect();
    if images.is_empty() {
        return Err(ImageError::NoImage(queued.prompt_id));
    }
    on_event(ImageEvent { percent: 75.0, stage: "generated", message: format!("Generated {} image(s).", images.len()) });
    Ok(Generated { prompt_id: queued.prompt_id, images, seed: applied.seed })
}
