//! ComfyUI HTTP and WebSocket client and the startup node check.
//!
//! Every call the app makes to ComfyUI goes through [`ComfyClient`]. The app never proxies arbitrary
//! requests to ComfyUI, which has no authentication.

pub mod client;
pub mod preflight;

pub use client::{ComfyClient, ComfyError, SystemStats};
pub use preflight::{check_nodes, Feature, NodeReport, REQUIRED_NODES};
