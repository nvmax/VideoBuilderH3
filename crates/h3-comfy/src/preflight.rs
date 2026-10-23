//! Startup node check: which node classes the H3 graphs need, and which ComfyUI is missing.
//!
//! Node lists come from the graphs the pack builds for each render mode (see `fixtures/graphs`) and the nodes its
//! patchers add (`runner/minimax_patches.py`, `minimax_inputs.py`, `minimax_refmod.py`), pack commit 14421d2.
//! The legacy fixed 3 pass graph (`minimax_ref2video_3pass_audio_driven_api.json`) is not reachable from the
//! Video Builder, so its nodes are not required.

use serde::Serialize;
use serde_json::Value;

/// Render feature a node is needed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Reference to Video, Single and 2 Pass, input audio, Latent Continuation Masked (v1 scope).
    Core,
    FastVaeDecode,
    SageAttention,
    ModelPatches,
    TwoPassAdvanced,
    ImageToVideo,
    RefMod,
    BuiltInAudio,
}

impl Feature {
    pub fn label(self) -> &'static str {
        match self {
            Feature::Core => "Core (Reference to Video, Single and 2 Pass)",
            Feature::FastVaeDecode => "Fast VAE decode",
            Feature::SageAttention => "Memory-efficient Sage attention",
            Feature::ModelPatches => "Feed-forward chunking and block-sparse attention",
            Feature::TwoPassAdvanced => "2 Pass Advanced",
            Feature::ImageToVideo => "Image to Video",
            Feature::RefMod => "RefMod pipeline",
            Feature::BuiltInAudio => "Built-in audio",
        }
    }
}

/// One required node class.
#[derive(Debug, Clone, Copy)]
pub struct NodeRequirement {
    pub class_type: &'static str,
    pub provider: &'static str,
    pub feature: Feature,
}

const fn req(class_type: &'static str, provider: &'static str, feature: Feature) -> NodeRequirement {
    NodeRequirement { class_type, provider, feature }
}

const CORE_COMFY: &str = "ComfyUI core (0.34 or newer)";
const VRGDG: &str = "comfyui-vrgamedevgirl";
const KJ: &str = "comfyui-kjnodes";
const VHS: &str = "ComfyUI-VideoHelperSuite";
const T8: &str = "comfyui-minimax-h3-audio-T8";
const TE_SPEED: &str = "TE-Speed-MiniMaxH3-OSS";
const H3MOD: &str = "ComfyUI-MiniMaxH3Mod";
const MMH3: &str = "Comfyui-MMH3-UltimateUpscale";

/// Every node class the H3 graphs can use, grouped by the feature that needs it.
pub const REQUIRED_NODES: &[NodeRequirement] = &[
    req("MiniMaxH3ReferenceToVideo", CORE_COMFY, Feature::Core),
    req("ResolutionSelector", CORE_COMFY, Feature::Core),
    req("ComfyMathExpression", CORE_COMFY, Feature::Core),
    req("EasyCache", CORE_COMFY, Feature::Core),
    req("BasicGuider", CORE_COMFY, Feature::Core),
    req("BasicScheduler", CORE_COMFY, Feature::Core),
    req("KSamplerSelect", CORE_COMFY, Feature::Core),
    req("RandomNoise", CORE_COMFY, Feature::Core),
    req("SamplerCustomAdvanced", CORE_COMFY, Feature::Core),
    req("CLIPLoader", CORE_COMFY, Feature::Core),
    req("VAELoader", CORE_COMFY, Feature::Core),
    req("VAEDecode", CORE_COMFY, Feature::Core),
    req("LoraLoaderModelOnly", CORE_COMFY, Feature::Core),
    req("PrimitiveFloat", CORE_COMFY, Feature::Core),
    req("PrimitiveInt", CORE_COMFY, Feature::Core),
    req("PrimitiveStringMultiline", CORE_COMFY, Feature::Core),
    req("VRGDG_MiniMaxH3AudioDrive", VRGDG, Feature::Core),
    req("VRGDG_MiniMaxH3ReferenceMediaFromPaths", VRGDG, Feature::Core),
    req("VRGDG_MiniMaxH3SaveLatent", VRGDG, Feature::Core),
    req("VRGDG_MiniMaxH3LoadLatent", VRGDG, Feature::Core),
    req("VRGDG_MiniMaxH3ApplyMaskedContinuation", VRGDG, Feature::Core),
    req("VRGDG_MiniMaxH3LatentUpscaleModelLoader", VRGDG, Feature::Core),
    req("VRGDG_MiniMaxH3LearnedLatentUpscale", VRGDG, Feature::Core),
    req("VRGDG_MiniMaxH3ReplaceUpscaledVideoLatent", VRGDG, Feature::Core),
    req("DiffusionModelLoaderKJ", KJ, Feature::Core),
    req("ImageResizeKJv2", KJ, Feature::Core),
    req("VHS_LoadAudio", VHS, Feature::Core),
    req("VHS_VideoCombine", VHS, Feature::Core),
    req("MiniMaxH3AVLatentSeparateT8", T8, Feature::Core),
    req("TESpeedMiniMaxH3", TE_SPEED, Feature::Core),
    req("H3FastVAEDecode", VRGDG, Feature::FastVaeDecode),
    req("MiniMaxH3MemoryEfficientSageAttentionPatch", KJ, Feature::SageAttention),
    req("MiniMaxChunkFeedForward", KJ, Feature::ModelPatches),
    req("BlockSparseAttention", CORE_COMFY, Feature::ModelPatches),
    req("VRGDG_MiniMaxH3SpatialTilePlan", VRGDG, Feature::TwoPassAdvanced),
    req("VRGDG_MiniMaxH3UltimateUpscaleParams", VRGDG, Feature::TwoPassAdvanced),
    req("MMH3UltimateUpscale", MMH3, Feature::TwoPassAdvanced),
    req("MMH3SpatialSplitParams", MMH3, Feature::TwoPassAdvanced),
    req("MMH3TemporalSplitParams", MMH3, Feature::TwoPassAdvanced),
    req("MiniMaxH3ImageToVideo", CORE_COMFY, Feature::ImageToVideo),
    req("VRGDG_MiniMaxH3KeyframeTiming", VRGDG, Feature::ImageToVideo),
    req("VRGDG_MiniMaxH3ImageReferenceToVideo", VRGDG, Feature::ImageToVideo),
    req("MiniMaxH3RefModsLoader", H3MOD, Feature::RefMod),
    req("MiniMaxH3RefModTextEncode", H3MOD, Feature::RefMod),
    req("MiniMaxH3RefModAudioExtract", H3MOD, Feature::RefMod),
    req("VRGDG_RefModCombine", VRGDG, Feature::RefMod),
    req("LTXVAudioVAEDecode", CORE_COMFY, Feature::BuiltInAudio),
];

/// Result of comparing [`REQUIRED_NODES`] with `/object_info`.
#[derive(Debug, Clone, Serialize)]
pub struct NodeReport {
    pub installed_count: usize,
    pub missing: Vec<MissingNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissingNode {
    pub class_type: &'static str,
    pub provider: &'static str,
    pub feature: Feature,
}

impl NodeReport {
    /// True when every node the v1 render path needs is installed.
    pub fn core_ready(&self) -> bool {
        !self.missing.iter().any(|m| m.feature == Feature::Core)
    }
}

/// Compare the node classes in an `/object_info` response with [`REQUIRED_NODES`].
pub fn check_nodes(object_info: &Value) -> NodeReport {
    let installed = object_info.as_object();
    let missing: Vec<MissingNode> = REQUIRED_NODES
        .iter()
        .filter(|r| !installed.is_some_and(|m| m.contains_key(r.class_type)))
        .map(|r| MissingNode { class_type: r.class_type, provider: r.provider, feature: r.feature })
        .collect();
    NodeReport { installed_count: installed.map_or(0, |m| m.len()), missing }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reports_missing_core_node() {
        let mut info = serde_json::Map::new();
        for r in REQUIRED_NODES {
            info.insert(r.class_type.to_string(), json!({}));
        }
        assert!(check_nodes(&Value::Object(info.clone())).missing.is_empty());

        info.remove("VRGDG_MiniMaxH3AudioDrive");
        info.remove("MMH3UltimateUpscale");
        let report = check_nodes(&Value::Object(info));
        assert_eq!(report.missing.len(), 2);
        assert!(!report.core_ready());
    }

    #[test]
    fn optional_only_missing_is_still_core_ready() {
        let info: serde_json::Map<String, Value> = REQUIRED_NODES
            .iter()
            .filter(|r| r.feature == Feature::Core)
            .map(|r| (r.class_type.to_string(), json!({})))
            .collect();
        assert!(check_nodes(&Value::Object(info)).core_ready());
    }

    /// Every node class in every golden graph from the pack must be in `REQUIRED_NODES`.
    #[test]
    fn required_nodes_cover_every_golden_graph() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/graphs");
        let mut checked = 0;
        let mut uncovered = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&dir).expect("fixtures/graphs exists") {
            let path = entry.unwrap().path();
            // The legacy fixed 3 pass graph is not reachable from the Video Builder (see module docs).
            if path.file_name().unwrap().to_string_lossy().starts_with("legacy_") {
                continue;
            }
            let record: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let Some(graph) = record.pointer("/result/prompt").and_then(Value::as_object) else { continue };
            checked += 1;
            for node in graph.values() {
                let class = node["class_type"].as_str().unwrap();
                if !REQUIRED_NODES.iter().any(|r| r.class_type == class) {
                    uncovered.insert(class.to_string());
                }
            }
        }
        assert!(checked > 50, "expected the golden graphs, found {checked}");
        assert!(uncovered.is_empty(), "node classes used by graphs but not in REQUIRED_NODES: {uncovered:?}");
    }
}
