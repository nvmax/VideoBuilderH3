//! HTTP client for the ComfyUI endpoints the app uses.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// Errors from talking to ComfyUI.
#[derive(Debug, thiserror::Error)]
pub enum ComfyError {
    #[error("ComfyUI is not reachable at {url}: {source}")]
    Unreachable { url: String, source: reqwest::Error },
    #[error("ComfyUI returned HTTP {status} for {path}: {body}")]
    Status { status: u16, path: String, body: String },
    #[error("ComfyUI sent a response that could not be read for {path}: {source}")]
    Decode { path: String, source: reqwest::Error },
}

/// Subset of `GET /system_stats`.
#[derive(Debug, Clone, Deserialize)]
pub struct SystemStats {
    pub system: SystemInfo,
    #[serde(default)]
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemInfo {
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub comfyui_version: String,
    #[serde(default)]
    pub python_version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vram_total: u64,
    #[serde(default)]
    pub vram_free: u64,
}

/// Client bound to one ComfyUI base URL.
#[derive(Debug, Clone)]
pub struct ComfyClient {
    base: String,
    http: reqwest::Client,
}

impl ComfyClient {
    /// Create a client for `base_url` (for example `http://127.0.0.1:8188`).
    pub fn new(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client with static settings");
        Self { base: base_url.trim_end_matches('/').to_string(), http }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ComfyError> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|source| ComfyError::Unreachable { url: self.base.clone(), source })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ComfyError::Status {
                status: status.as_u16(),
                path: path.to_string(),
                body: body.chars().take(500).collect(),
            });
        }
        resp.json::<T>()
            .await
            .map_err(|source| ComfyError::Decode { path: path.to_string(), source })
    }

    /// `GET /system_stats`: health check, versions and GPU memory.
    pub async fn system_stats(&self) -> Result<SystemStats, ComfyError> {
        self.get_json("/system_stats").await
    }

    /// `GET /object_info`: every registered node class with its inputs.
    pub async fn object_info(&self) -> Result<Value, ComfyError> {
        self.get_json("/object_info").await
    }
}
