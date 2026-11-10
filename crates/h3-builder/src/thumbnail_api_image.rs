//! Thumbnail "API image" engine: the chosen frame edited by Gemini, Grok or OpenAI with the key from the LLM Runner. Port of the pack's
//! `builder/thumbnail_api_image.py`. The HTTP calls are in `h3_llm::api_image`.
//!
//! The result is kept as `<project>/thumbnail/thumbnail_NNNN.png` like every other thumbnail engine's, so the dialog skips ComfyUI and
//! `thumbnail/save`.

use std::fs;
use std::future::Future;

use h3_core::pyval::Obj;
use h3_llm::api_image;
use image::{ImageFormat, RgbImage};
use serde_json::{json, Value};

use crate::payload::{path_text, text};
use crate::thumbnail::{absolute, next_thumbnail_path, THUMBNAIL_FOLDER};

type Reply = Result<Value, String>;

/// `POST /vrgdg/music_builder/thumbnail/api_image`: edit the frame with the image API of the LLM Runner's provider.
///
/// `source_path` is the frame, `prompt` the edit prompt, `provider` the LLM Runner's provider (`google`, `grok`, `openai`), `api_key` its key
/// and `model` the image model (empty: the provider's first one).
pub async fn make_thumbnail_api_image(payload: &Obj) -> Reply {
    make_with(payload, |provider, key, model, prompt, frame| async move { api_image::edit_with_api(provider, &key, &model, &prompt, &frame).await }).await
}

/// An image model name: a letter or digit first, then letters, digits and `. _ : -`, at most 120 characters.
fn valid_model(model: &str) -> bool {
    let mut chars = model.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && model.chars().count() <= 120
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// [`make_thumbnail_api_image`] with the call to the provider given, so a test needs no network. `draw` gets the provider key, the API key,
/// the model, the prompt and the frame, and answers with the picture.
pub(crate) async fn make_with<F, Fut>(payload: &Obj, draw: F) -> Reply
where
    F: FnOnce(&'static str, String, String, String, RgbImage) -> Fut,
    Fut: Future<Output = Result<RgbImage, String>>,
{
    let project_text = path_text(payload, "project_folder");
    if project_text.is_empty() {
        return Err("Project folder is empty.".into());
    }
    let source_text = path_text(payload, "source_path");
    if source_text.is_empty() {
        return Err("Choose the frame first: scrub the video to the moment you want and click Use this frame.".into());
    }
    let provider = api_image::provider(&text(payload, "provider"))?;
    let mut model = text(payload, "model");
    if model.is_empty() {
        model = api_image::default_model(provider);
    }
    if !valid_model(&model) {
        return Err(format!("'{model}' is not a valid image model name."));
    }
    let source = absolute(&source_text);
    if !source.is_file() {
        return Err(format!("The frame was not found: {}", source.display()));
    }
    let frame = image::open(&source).map_err(|e| format!("Could not read the picture {}: {e}", source.display()))?.to_rgb8();
    let picture = draw(provider, text(payload, "api_key"), model.clone(), text(payload, "prompt"), frame).await?;

    // The folder is made only now, so a refusal leaves nothing behind.
    let folder = absolute(&project_text).join(THUMBNAIL_FOLDER);
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let target = next_thumbnail_path(&folder, "thumbnail");
    picture.save_with_format(&target, ImageFormat::Png).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(json!({"image_path": target.display().to_string(), "width": picture.width(), "height": picture.height(), "provider": provider, "model": model}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use image::Rgb;

    /// What `draw` was asked: provider, key, model, prompt and the frame's size.
    type Call = (String, String, String, String, (u32, u32));

    struct Project {
        dir: tempfile::TempDir,
        frame: std::path::PathBuf,
    }

    impl Project {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let frame = dir.path().join("source.png");
            RgbImage::from_pixel(640, 360, Rgb([20, 20, 20])).save(&frame).unwrap();
            Self { dir, frame }
        }

        fn payload(&self, extra: Value) -> Obj {
            let mut base = json!({"project_folder": self.dir.path().display().to_string(), "source_path": self.frame.display().to_string(), "prompt": "Add HEAT.", "provider": "google", "api_key": "sk-test"});
            base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            base.as_object().unwrap().clone()
        }
    }

    /// A `draw` that records what it was asked and answers with a green 640x360 picture.
    fn drawn(calls: &RefCell<Vec<Call>>) -> impl FnOnce(&'static str, String, String, String, RgbImage) -> std::future::Ready<Result<RgbImage, String>> + '_ {
        move |provider, key, model, prompt, frame| {
            calls.borrow_mut().push((provider.to_string(), key, model, prompt, frame.dimensions()));
            std::future::ready(Ok(RgbImage::from_pixel(640, 360, Rgb([10, 200, 30]))))
        }
    }

    #[tokio::test]
    async fn the_picture_is_kept_as_the_next_numbered_thumbnail() {
        let project = Project::new();
        let calls = RefCell::new(Vec::new());
        let first = make_with(&project.payload(json!({})), drawn(&calls)).await.unwrap();
        let second = make_with(&project.payload(json!({})), drawn(&calls)).await.unwrap();
        let folder = project.dir.path().join("thumbnail");
        assert_eq!(first["image_path"], folder.join("thumbnail_0001.png").display().to_string());
        assert_eq!(second["image_path"], folder.join("thumbnail_0002.png").display().to_string());
        assert_eq!((first["width"].clone(), first["height"].clone(), first["provider"].clone()), (json!(640), json!(360), json!("gemini")));
        assert_eq!(image::open(folder.join("thumbnail_0001.png")).unwrap().to_rgb8().get_pixel(3, 3).0, [10, 200, 30]);
        // The provider got what the dialog sent, with the LLM Runner's name turned into the provider.
        assert_eq!(calls.borrow()[0], ("gemini".to_string(), "sk-test".to_string(), "gemini-3-pro-image-preview".to_string(), "Add HEAT.".to_string(), (640, 360)));
    }

    #[tokio::test]
    async fn it_shares_the_numbering_with_the_other_engines() {
        let project = Project::new();
        let folders = crate::runner::ComfyFolders { output: String::new(), input: String::new(), temp: String::new() };
        crate::thumbnail::save_thumbnail_image(&folders, &project.payload(json!({}))).unwrap();
        let made = make_with(&project.payload(json!({})), drawn(&RefCell::new(Vec::new()))).await.unwrap();
        assert!(made["image_path"].as_str().unwrap().ends_with("thumbnail_0002.png"), "{made}");
    }

    #[tokio::test]
    async fn no_model_means_the_providers_first_one_and_a_chosen_one_is_used() {
        let project = Project::new();
        let calls = RefCell::new(Vec::new());
        let first = make_with(&project.payload(json!({})), drawn(&calls)).await.unwrap();
        let second = make_with(&project.payload(json!({"model": "my-own-model.2"})), drawn(&calls)).await.unwrap();
        assert_eq!(first["model"], api_image::default_model("gemini"));
        assert_eq!(second["model"], "my-own-model.2");
        assert_eq!(calls.borrow()[1].2, "my-own-model.2");
    }

    #[tokio::test]
    async fn a_bad_request_is_refused_before_any_call() {
        let project = Project::new();
        let calls = RefCell::new(Vec::new());
        let cases = [
            (json!({"project_folder": ""}), "Project folder is empty"),
            (json!({"source_path": "  "}), "Choose the frame first"),
            (json!({"provider": "anthropic"}), "cannot draw pictures"),
            (json!({"provider": ""}), "no API provider"),
            (json!({"model": "bad model/../x"}), "not a valid image model name"),
            (json!({"source_path": project.dir.path().join("nope.png").display().to_string()}), "The frame was not found"),
        ];
        for (extra, message) in cases {
            let error = make_with(&project.payload(extra), drawn(&calls)).await.unwrap_err();
            assert!(error.contains(message), "{message}: {error}");
        }
        assert!(calls.borrow().is_empty());
        assert!(!project.dir.path().join("thumbnail").exists());
    }

    #[tokio::test]
    async fn a_provider_error_is_returned_with_its_text_and_leaves_no_file_behind() {
        let project = Project::new();
        let error = make_with(&project.payload(json!({})), |_, _, _, _, _| std::future::ready(Err("Gemini HTTP 429: quota".to_string()))).await.unwrap_err();
        assert_eq!(error, "Gemini HTTP 429: quota");
        assert!(!project.dir.path().join("thumbnail").exists());
    }

    #[tokio::test]
    async fn the_real_call_refuses_a_missing_key_and_prompt_before_it_sends_anything() {
        let project = Project::new();
        assert_eq!(make_thumbnail_api_image(&project.payload(json!({"api_key": ""}))).await.unwrap_err(), "Add your Gemini API key in the LLM Runner first.");
        assert_eq!(make_thumbnail_api_image(&project.payload(json!({"prompt": " "}))).await.unwrap_err(), "Write the prompt first.");
    }

    #[test]
    fn image_model_names_follow_the_packs_pattern() {
        for good in ["gpt-image-2", "gemini-3.1-flash-image-preview", "my-own-model.2", "a", "x:y_z"] {
            assert!(valid_model(good), "{good}");
        }
        for bad in ["", "-x", ".x", "bad model", "a/b", "../x", &"a".repeat(121)] {
            assert!(!valid_model(bad), "{bad}");
        }
        assert!(valid_model(&"a".repeat(120)));
    }
}
