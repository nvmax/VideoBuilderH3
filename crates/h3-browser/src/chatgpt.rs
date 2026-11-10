//! ChatGPT Images in the person's Chrome. Port of the pack's `flow_automation/chatgpt-images-poc.mjs` and `browser-image-viewer.mjs`: open
//! the Images page, attach the reference pictures, type the prompt, send it, wait for a new picture and save it.
//!
//! The page text and buttons are ChatGPT's. When ChatGPT changes them this is the file to change, and the messages name what was not found.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::{close_viewer, click_generated_image, candidate_image_keys, download_from_viewer, images_on_page, newest_visible_image, normalize_prompt, save_image_url, wait_for_chat, ImageInfo};
use crate::error::{BrowserError, Result};
use crate::page::{re, sleep, Browser, Element, Loc, Page};
use crate::{EditRequest, Progress};

const URL: &str = "https://chatgpt.com/images";
const FALLBACK_NAME: &str = "chatgpt-image";

/// Make one picture in ChatGPT Images and return the file it was saved to (inside `request.out_dir`).
pub async fn edit(browser: &Browser, request: &EditRequest, progress: Progress<'_>) -> Result<PathBuf> {
    for image in &request.images {
        if !image.is_file() {
            return Err(BrowserError::File(format!("Image file does not exist: {}", image.display())));
        }
    }
    browser.allow_downloads(&request.out_dir).await?;
    let page = browser.working_tab(&["https://chatgpt.com/"], "", URL).await?;
    if request.reuse_open_project {
        progress(&format!("Using the open ChatGPT tab: {}", page.url().await));
        close_viewer(&page).await;
    } else {
        progress("Opening ChatGPT Images...");
        page.goto(URL, Duration::from_secs(60)).await?;
        sleep(3000).await;
    }
    page.bring_to_front().await;
    if wait_for_composer(&page, Duration::from_secs(120)).await?.is_none() {
        return Err(BrowserError::page("Could not find the ChatGPT Images prompt box. Use \"Open and sign in\" for GPT Image, sign in to ChatGPT in that Chrome window, then try again."));
    }

    for (index, image) in request.images.iter().enumerate() {
        progress(&format!("Attaching image {}/{}...", index + 1, request.images.len()));
        attach_image(&page, image).await?;
    }

    let before = candidate_image_keys(&page).await;
    let composer = find_composer(&page).await?.ok_or_else(|| BrowserError::page("Could not find the ChatGPT Images prompt box."))?;
    progress("Typing the prompt...");
    enter_prompt(&page, &composer, &request.prompt).await?;
    progress("Sending the prompt to ChatGPT...");
    if !page.click_first(&send_locators()).await? {
        page.press_key("Enter", 0, &[]).await?;
    }

    progress("Waiting for ChatGPT to finish the picture...");
    let image = wait_for_new_image(&page, &before, request.timeout, progress).await?;
    progress("Saving the picture from ChatGPT...");
    let saved = save_generated(browser, &page, &image, request, progress).await;
    close_viewer(&page).await;
    let path = saved?;
    if !wait_for_chat(&page, &composer_locators(), Duration::from_secs(10)).await {
        progress("ChatGPT saved the picture, but its viewer did not return to the chat prompt.");
    }
    Ok(path)
}

fn composer_locators() -> Vec<Loc> {
    vec![
        Loc::placeholder(re("describe a new image", "i")),
        Loc::placeholder(re("ask anything", "i")),
        Loc::css("textarea[placeholder*='Describe' i]"),
        Loc::css("textarea[placeholder*='Ask' i]"),
        Loc::css("[contenteditable='true'][data-placeholder*='Describe' i]"),
        Loc::css("[contenteditable='true'][aria-label*='message' i]"),
        Loc::css("[contenteditable='true']"),
        Loc::css("textarea"),
    ]
}

async fn find_composer(page: &Page) -> Result<Option<Element>> {
    page.find_first(&composer_locators(), false).await
}

async fn wait_for_composer(page: &Page, limit: Duration) -> Result<Option<Element>> {
    page.wait_for_any(&composer_locators(), limit).await
}

fn send_locators() -> Vec<Loc> {
    vec![
        Loc::role("button", Some(re("^send prompt$", "i"))),
        Loc::css("button[aria-label*='Send prompt' i]"),
        Loc::role("button", Some(re("send prompt|send message|send", "i"))),
        Loc::css("button[aria-label*='Send' i]"),
        Loc::css("button[data-testid*='send' i]"),
    ]
}

/// Attach a picture: through the page's file box when it has one, else through the paperclip and its file dialog.
async fn attach_image(page: &Page, file: &Path) -> Result<()> {
    for loc in [Loc::css("input[type='file'][accept*='image' i]"), Loc::css("input[type='file']")] {
        if let Some(input) = page.find_existing(&loc).await? {
            input.set_files(&[file.to_path_buf()]).await?;
            sleep(5000).await;
            return Ok(());
        }
    }
    let openers = [
        Loc::role("button", Some(re("^add photos$", "i"))),
        Loc::css("button[aria-label*='Add photos' i]"),
        Loc::role("button", Some(re("attach|upload|add photo|add files", "i"))),
        Loc::css("button[aria-label*='Attach' i]"),
        Loc::css("button[aria-label*='Upload' i]"),
        Loc::css("button[aria-label*='paperclip' i]"),
        Loc::css("[data-testid*='attach' i]"),
        Loc::css("[data-testid*='upload' i]"),
    ];
    page.choose_files(&[file.to_path_buf()], Duration::from_secs(20), || async {
        if page.click_first(&openers).await? {
            Ok(())
        } else {
            Err(BrowserError::page("Could not find the ChatGPT paperclip/attach button."))
        }
    })
    .await?;
    sleep(5000).await;
    Ok(())
}

async fn prompt_looks_entered(composer: &Element, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let wanted: String = text.chars().take(40).collect::<String>().to_lowercase();
    composer.text().await.to_lowercase().contains(&wanted)
}

/// Replace what is in the box with the prompt: select all and insert it, check it stuck, else clear with Backspace and insert again.
async fn enter_prompt(page: &Page, composer: &Element, prompt: &str) -> Result<()> {
    let text = normalize_prompt(prompt);
    composer.click().await?;
    let _ = page.select_all().await;
    page.insert_text(&text).await?;
    sleep(300).await;
    if prompt_looks_entered(composer, &text).await {
        return Ok(());
    }
    let _ = page.select_all().await;
    let _ = page.press_key("Backspace", 0, &[]).await;
    page.insert_text(&text).await?;
    sleep(500).await;
    if prompt_looks_entered(composer, &text).await {
        return Ok(());
    }
    Err(BrowserError::page("Could not enter prompt text in ChatGPT Images."))
}

/// The first picture that is visible, loaded and not among the ones from before the prompt. When none is new before `limit`, the newest
/// visible one is used, as the script did.
async fn wait_for_new_image(page: &Page, before: &HashSet<String>, limit: Duration, progress: Progress<'_>) -> Result<ImageInfo> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut last: Option<ImageInfo> = None;
    loop {
        for info in images_on_page(page, 200).await.into_iter().rev() {
            if !info.visible || !info.complete {
                continue;
            }
            if !before.contains(&info.key) {
                return Ok(info);
            }
            last.get_or_insert(info);
        }
        if tokio::time::Instant::now() >= deadline {
            return match last {
                Some(info) => {
                    progress("No brand-new image was detected; using the newest visible image.");
                    Ok(info)
                }
                None => Err(BrowserError::timeout("Timed out waiting for a generated ChatGPT image.")),
            };
        }
        sleep(3000).await;
    }
}

/// Open the picture in ChatGPT's viewer and save it: from its address, else with the viewer's download button. Up to five tries, 5 seconds
/// apart. Then, if nothing worked, the address of the largest visible picture.
async fn save_generated(browser: &Browser, page: &Page, image: &ImageInfo, request: &EditRequest, progress: Progress<'_>) -> Result<PathBuf> {
    let dir = request.out_dir.as_path();
    let mut wanted = image.clone();
    for attempt in 1..=5 {
        progress(&format!("Opening the generated image, attempt {attempt}/5..."));
        let _ = page.press_key("Escape", 0, &[]).await;
        sleep(500).await;
        let mut opened = click_generated_image(page, &wanted).await?;
        if !opened {
            if let Ok(newest) = newest_visible_image(page).await {
                wanted = newest;
                opened = click_generated_image(page, &wanted).await?;
            }
        }
        if !opened {
            if attempt < 5 {
                sleep(5000).await;
            }
            continue;
        }
        sleep(3000).await;
        if let Ok(newest) = newest_visible_image(page).await {
            match save_image_url(browser, page, &newest.src, dir, &request.prompt, FALLBACK_NAME, false).await {
                Ok(path) => return Ok(path),
                Err(error) => progress(&format!("Direct viewer image save did not work: {error}")),
            }
        }
        if let Some(path) = download_from_viewer(page, dir, true, Duration::from_secs(5)).await {
            return Ok(path);
        }
        close_viewer(page).await;
        if attempt < 5 {
            sleep(5000).await;
        }
    }
    progress("The viewer did not give a file; trying a direct image save.");
    let newest = newest_visible_image(page).await?;
    save_image_url(browser, page, &newest.src, dir, &request.prompt, FALLBACK_NAME, false).await
}
