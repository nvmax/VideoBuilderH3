//! Thumbnail "Browser AI" engine, run by the app itself: the chosen frame edited by Flow, GPT Image or Meta AI in the person's Chrome.
//! The pack runs this engine as a ComfyUI workflow whose nodes drive Chrome with Node.js and Playwright. The app drives Chrome directly
//! (`h3-browser`), so nothing needs installing: it asks for `thumbnail/capabilities` to say `browser_ai_direct` and the dialog calls this
//! route instead of building a workflow.
//!
//! The result is kept as `<project>/thumbnail/thumbnail_NNNN.png` like every other thumbnail engine's, so the dialog skips ComfyUI and
//! `thumbnail/save`.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use h3_browser::{EditRequest, Provider};
use h3_core::pyval::{to_float, Obj};
use serde_json::{json, Value};

use crate::payload::{path_text, text};
use crate::runner::ComfyFolders;
use crate::thumbnail::{absolute, save_thumbnail_image};

type Reply = Result<Value, String>;

/// The shortest and the longest wait for a picture, as the pack's Browser AI nodes allow (`timeout_seconds` 60 to 2400).
const MIN_TIMEOUT: u64 = 60;
const MAX_TIMEOUT: u64 = 2400;

/// `POST /vrgdg/music_builder/thumbnail/browser_ai`: `{project_folder, source_path, prompt, provider, timeout_seconds, reuse_open_project}`.
///
/// `source_path` is the frame, `provider` the Image panel's Browser AI provider (`flow_nano_banana`, `gpt_image`, `meta_ai`; empty is Flow).
/// The site's own download goes to `work_dir` first and is deleted once the project has its copy.
pub async fn make_thumbnail_browser_ai(payload: &Obj, work_dir: &Path) -> Reply {
    make_with(payload, work_dir, |request| async move {
        h3_browser::make_picture(&request, &|message| eprintln!("[VRGDG-RS Thumbnail] Browser AI: {message}")).await.map_err(|e| e.to_string())
    })
    .await
}

/// `POST /vrgdg/music_builder/thumbnail/browser_ai/open`: open the provider's site in Chrome so the person can sign in. `profile_root` keeps
/// each provider's Chrome profile, which is what makes the sign-in last.
pub async fn open_thumbnail_browser_ai(payload: &Obj, profile_root: &Path) -> Reply {
    let provider = provider_of(payload)?;
    h3_browser::chrome::start_debug_chrome(&profile_root.join(provider.profile_name()), provider.debug_port(), provider.url()).await.map_err(|e| e.to_string())?;
    Ok(json!({"provider": provider.id(), "provider_label": provider.label(), "url": provider.url(), "debug_port": provider.debug_port()}))
}

fn provider_of(payload: &Obj) -> Result<Provider, String> {
    let name = text(payload, "provider");
    Provider::parse(if name.is_empty() { Provider::Flow.id() } else { &name }).map_err(|e| e.to_string())
}

/// [`make_thumbnail_browser_ai`] with the browser work given, so a test needs no Chrome. `make` gets the request and answers with the file
/// the site's picture was saved to.
pub(crate) async fn make_with<F, Fut>(payload: &Obj, work_dir: &Path, make: F) -> Reply
where
    F: FnOnce(EditRequest) -> Fut,
    Fut: Future<Output = Result<PathBuf, String>>,
{
    let project_text = path_text(payload, "project_folder");
    if project_text.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let source_text = path_text(payload, "source_path");
    if source_text.is_empty() {
        return Err("Choose the frame first: scrub the video to the moment you want and click Use this frame.".into());
    }
    let prompt = text(payload, "prompt");
    if prompt.trim().is_empty() {
        return Err("Write the prompt first.".into());
    }
    let provider = provider_of(payload)?;
    let seconds = payload.get("timeout_seconds").and_then(to_float).filter(|s| s.is_finite() && *s > 0.0).map_or(provider.default_timeout_seconds(), |s| s as u64).clamp(MIN_TIMEOUT, MAX_TIMEOUT);
    let source = absolute(&source_text);
    if !source.is_file() {
        return Err(format!("The frame was not found: {}", source.display()));
    }

    let request = EditRequest {
        provider,
        prompt,
        images: vec![source],
        out_dir: work_dir.join(provider.id()),
        timeout: Duration::from_secs(seconds),
        reuse_open_project: payload.get("reuse_open_project").is_some_and(h3_core::pyval::truthy),
    };
    let made = make(request).await?;

    // The project's copy is made only now, so a failed try leaves nothing behind in it.
    let folders = ComfyFolders { output: String::new(), input: String::new(), temp: String::new() };
    let mut keep = Obj::new();
    keep.insert("project_folder".into(), Value::String(project_text));
    keep.insert("source_path".into(), Value::String(made.display().to_string()));
    let saved = save_thumbnail_image(&folders, &keep);
    let _ = fs::remove_file(&made);
    let mut saved = saved?;
    saved["provider"] = json!(provider.id());
    saved["provider_label"] = json!(provider.label());
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use image::{Rgb, RgbImage};

    struct Project {
        dir: tempfile::TempDir,
        frame: PathBuf,
    }

    impl Project {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let frame = dir.path().join("source.png");
            RgbImage::from_pixel(640, 360, Rgb([20, 20, 20])).save(&frame).unwrap();
            Self { dir, frame }
        }

        fn payload(&self, extra: Value) -> Obj {
            let mut base = json!({"project_folder": self.dir.path().display().to_string(), "source_path": self.frame.display().to_string(), "prompt": "Add HEAT.", "provider": "flow_nano_banana", "timeout_seconds": 420});
            base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            base.as_object().unwrap().clone()
        }

        fn work(&self) -> PathBuf {
            self.dir.path().join("work")
        }
    }

    /// A `make` that records the request and writes a green 800x450 picture where the site would have saved it.
    fn made<'a>(requests: &'a RefCell<Vec<EditRequest>>) -> impl FnOnce(EditRequest) -> std::future::Ready<Result<PathBuf, String>> + 'a {
        move |request| {
            fs::create_dir_all(&request.out_dir).unwrap();
            let file = request.out_dir.join("from-the-site.png");
            RgbImage::from_pixel(800, 450, Rgb([10, 200, 30])).save(&file).unwrap();
            requests.borrow_mut().push(request);
            std::future::ready(Ok(file))
        }
    }

    #[tokio::test]
    async fn the_picture_is_kept_as_the_next_numbered_thumbnail_and_the_download_is_removed() {
        let project = Project::new();
        let requests = RefCell::new(Vec::new());
        let first = make_with(&project.payload(json!({})), &project.work(), made(&requests)).await.unwrap();
        let second = make_with(&project.payload(json!({})), &project.work(), made(&requests)).await.unwrap();
        let folder = project.dir.path().join("thumbnail");
        assert_eq!(first["image_path"], folder.join("thumbnail_0001.png").display().to_string());
        assert_eq!(second["image_path"], folder.join("thumbnail_0002.png").display().to_string());
        assert_eq!((first["width"].clone(), first["height"].clone(), first["provider"].clone(), first["provider_label"].clone()), (json!(800), json!(450), json!("flow_nano_banana"), json!("Flow Nano Banana")));
        assert_eq!(image::open(folder.join("thumbnail_0001.png")).unwrap().to_rgb8().get_pixel(3, 3).0, [10, 200, 30]);
        // The site's own file is gone, the project has its copy, and the frame is untouched.
        assert!(!project.work().join("flow_nano_banana").join("from-the-site.png").exists());
        assert!(project.frame.is_file());
    }

    #[tokio::test]
    async fn the_site_is_asked_with_the_frame_the_prompt_and_the_providers_timeout_rules() {
        let project = Project::new();
        let requests = RefCell::new(Vec::new());
        make_with(&project.payload(json!({})), &project.work(), made(&requests)).await.unwrap();
        make_with(&project.payload(json!({"provider": "ChatGPT", "timeout_seconds": 5})), &project.work(), made(&requests)).await.unwrap();
        make_with(&project.payload(json!({"provider": "", "timeout_seconds": null, "reuse_open_project": true})), &project.work(), made(&requests)).await.unwrap();
        make_with(&project.payload(json!({"provider": "meta", "timeout_seconds": 99999})), &project.work(), made(&requests)).await.unwrap();
        let log = requests.borrow();
        assert_eq!(log[0].provider, Provider::Flow);
        assert_eq!((log[0].prompt.as_str(), log[0].images.clone(), log[0].timeout, log[0].reuse_open_project), ("Add HEAT.", vec![project.frame.clone()], Duration::from_secs(420), false));
        // Too short a wait is raised to the least the pack allows, no timeout is the provider's own, an empty provider is Flow, too long is capped.
        assert_eq!((log[1].provider, log[1].timeout), (Provider::GptImage, Duration::from_secs(60)));
        assert_eq!((log[2].provider, log[2].timeout, log[2].reuse_open_project), (Provider::Flow, Duration::from_secs(420), true));
        assert_eq!((log[3].provider, log[3].timeout), (Provider::MetaAi, Duration::from_secs(2400)));
        // Each provider downloads into its own folder.
        assert_eq!(log[1].out_dir, project.work().join("gpt_image"));
    }

    #[tokio::test]
    async fn a_bad_request_is_refused_before_chrome_is_asked() {
        let project = Project::new();
        let requests = RefCell::new(Vec::new());
        let cases = [
            (json!({"project_folder": ""}), "Project folder is empty"),
            (json!({"source_path": "  "}), "Choose the frame first"),
            (json!({"prompt": "   "}), "Write the prompt first"),
            (json!({"provider": "midjourney"}), "Unknown browser image provider: midjourney"),
            (json!({"source_path": project.dir.path().join("nope.png").display().to_string()}), "The frame was not found"),
        ];
        for (extra, message) in cases {
            let error = make_with(&project.payload(extra), &project.work(), made(&requests)).await.unwrap_err();
            assert!(error.contains(message), "{message}: {error}");
        }
        assert!(requests.borrow().is_empty());
        assert!(!project.dir.path().join("thumbnail").exists());
    }

    #[tokio::test]
    async fn a_failed_try_returns_the_reason_and_leaves_the_project_alone() {
        let project = Project::new();
        let error = make_with(&project.payload(json!({})), &project.work(), |_| std::future::ready(Err("Flow wants you to sign in to Google first.".to_string()))).await.unwrap_err();
        assert_eq!(error, "Flow wants you to sign in to Google first.");
        assert!(!project.dir.path().join("thumbnail").exists());
    }

    #[tokio::test]
    async fn a_site_file_that_is_not_a_picture_is_an_error_and_is_cleaned_up() {
        let project = Project::new();
        let error = make_with(&project.payload(json!({})), &project.work(), |request| {
            fs::create_dir_all(&request.out_dir).unwrap();
            let file = request.out_dir.join("broken.png");
            fs::write(&file, b"not a picture").unwrap();
            std::future::ready(Ok(file))
        })
        .await
        .unwrap_err();
        assert!(error.starts_with("Could not read the picture"), "{error}");
        assert!(!project.work().join("flow_nano_banana").join("broken.png").exists());
    }

    #[tokio::test]
    async fn opening_the_site_needs_a_known_provider() {
        let dir = tempfile::tempdir().unwrap();
        let payload = json!({"provider": "midjourney"}).as_object().unwrap().clone();
        assert_eq!(open_thumbnail_browser_ai(&payload, dir.path()).await.unwrap_err(), "Unknown browser image provider: midjourney");
    }
}
