//! Drives the person's own Chrome through its debug port with the Chrome DevTools Protocol, to make a picture with Flow, ChatGPT
//! Images or Meta AI. It replaces the pack's Node.js and Playwright scripts (`flow_automation/*.mjs`), so nothing has to be installed
//! next to the app: only Chrome, and a one-time sign-in to the site.

use std::path::PathBuf;
use std::time::Duration;

pub mod cdp;
pub mod chatgpt;
pub mod chrome;
pub mod common;
pub mod error;
pub mod flow;
pub mod meta;
pub mod page;
pub mod provider;

pub use error::{BrowserError, Result};
pub use provider::Provider;

/// Where the automation reports what it is doing, in words for the person ("Uploading image 1/1...").
pub type Progress<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// One picture to make.
#[derive(Clone, Debug)]
pub struct EditRequest {
    pub provider: Provider,
    pub prompt: String,
    /// Pictures to edit or use as references, uploaded to the site in this order. Empty for a picture from the prompt alone.
    pub images: Vec<PathBuf>,
    /// The folder the picture is saved in.
    pub out_dir: PathBuf,
    /// How long to wait for the site to finish the picture.
    pub timeout: Duration,
    /// Work in the tab that is already open instead of opening the site's start page again.
    pub reuse_open_project: bool,
}

/// Make the picture on the provider's site, in the Chrome that listens on the provider's debug port, and return the saved file.
pub async fn make_picture(request: &EditRequest, progress: Progress<'_>) -> Result<PathBuf> {
    let port = request.provider.debug_port();
    let browser = page::Browser::connect(port).await.map_err(|error| match error {
        BrowserError::Chrome(_) => BrowserError::Chrome(format!("{} is not open in Chrome. Use \"Open Chrome and sign in\" for {} first, sign in in that window, then try again.", request.provider.label(), request.provider.label())),
        other => other,
    })?;
    std::fs::create_dir_all(&request.out_dir).map_err(|e| BrowserError::File(format!("Could not create {}: {e}", request.out_dir.display())))?;
    match request.provider {
        Provider::Flow => flow::edit(&browser, request, progress).await,
        Provider::GptImage => chatgpt::edit(&browser, request, progress).await,
        Provider::MetaAi => meta::edit(&browser, request, progress).await,
    }
}
