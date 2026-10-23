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
    /// ComfyUI refused the graph. `node_errors` holds its per node explanation.
    #[error("ComfyUI refused the workflow: {message}")]
    Queue { message: String, node_errors: Value },
    #[error("Workflow execution failed:
{0}")]
    Execution(String),
    #[error("The render was cancelled.")]
    Cancelled,
    #[error("Prompt {prompt_id} did not finish within {seconds:.0} seconds.")]
    Timeout { prompt_id: String, seconds: f64 },
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
    /// The command line ComfyUI was started with. Its first item is `main.py`.
    #[serde(default)]
    pub argv: Vec<String>,
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

    /// `GET path` as loose JSON.
    pub(crate) async fn get_value(&self, path: &str) -> Result<Value, ComfyError> {
        self.get_json(path).await
    }

    /// `POST path` with a JSON body (`Value::Null` sends no body). Returns the status and the response text, so the
    /// caller can read ComfyUI's own error explanation from a 400.
    pub(crate) async fn post_text(&self, path: &str, body: &Value) -> Result<(u16, String), ComfyError> {
        let url = format!("{}{}", self.base, path);
        let mut request = self.http.post(&url);
        if !body.is_null() {
            request = request.json(body);
        }
        let resp = request.send().await.map_err(|source| ComfyError::Unreachable { url: self.base.clone(), source })?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    /// `GET /internal/folder_paths`: absolute model and node folders. `None` on a ComfyUI that does not have it.
    pub async fn folder_paths(&self) -> Option<Value> {
        self.get_json::<Value>("/internal/folder_paths").await.ok()
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
