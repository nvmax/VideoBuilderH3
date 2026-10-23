//! ComfyUI HTTP and WebSocket client and the startup node check.
//!
//! Every call the app makes to ComfyUI goes through [`ComfyClient`]. The app never proxies arbitrary
//! requests to ComfyUI, which has no authentication.

pub mod client;
pub mod models;
pub mod preflight;
pub mod queue;

pub use client::{ComfyClient, ComfyError, SystemStats};
pub use queue::{extract_images, extract_texts, extract_videos, prompt_error, prompt_finished, ComfyDirs, Queued};
pub use preflight::{check_nodes, Feature, NodeReport, REQUIRED_NODES};
