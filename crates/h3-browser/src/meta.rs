//! Meta AI in the person's Chrome. Port of the pack's `flow_automation/meta-ai-poc.mjs`: open meta.ai, attach the reference pictures through
//! the "+" button, type the prompt, press the arrow beside the Instant button, follow the new conversation, wait for a new picture and
//! save it (the picture's own Download button first, its address otherwise).
//!
//! The page text and buttons are Meta's. When Meta changes them this is the file to change, and the messages name what was not found.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::common::{candidate_image_keys, close_viewer, images_on_page, normalize_prompt, save_image_url, wait_for_chat, ImageInfo};
use crate::error::{BrowserError, Result};
use crate::page::{re, sleep, Browser, Element, Loc, Page};
use crate::{EditRequest, Progress};

const URL: &str = "https://www.meta.ai/";
const PREFIXES: [&str; 2] = ["https://www.meta.ai/", "https://meta.ai/"];
const FALLBACK_NAME: &str = "meta-ai-image";

/// Make one picture in Meta AI and return the file it was saved to (inside `request.out_dir`).
pub async fn edit(browser: &Browser, request: &EditRequest, progress: Progress<'_>) -> Result<PathBuf> {
    for image in &request.images {
        if !image.is_file() {
            return Err(BrowserError::File(format!("Image file does not exist: {}", image.display())));
        }
    }
    browser.allow_downloads(&request.out_dir).await?;
    let mut page = browser.working_tab(&PREFIXES, "", URL).await?;
    if request.reuse_open_project {
        progress(&format!("Using the open Meta AI tab: {}", page.url().await));
        close_viewer(&page).await;
    } else {
        progress("Opening Meta AI...");
        page.goto(URL, Duration::from_secs(60)).await?;
        sleep(3000).await;
    }
    page.bring_to_front().await;
    if wait_for_composer(&page, Duration::from_secs(120)).await?.is_none() {
        if login_prompt_visible(&page).await {
            return Err(BrowserError::page("Meta AI is asking for login. Use \"Open and sign in\" for Meta AI, sign in in that Chrome window, then try again."));
        }
        return Err(BrowserError::page("Could not find the Meta AI prompt box. Use \"Open and sign in\" for Meta AI, sign in in that Chrome window, then try again."));
    }

    for (index, image) in request.images.iter().enumerate() {
        progress(&format!("Attaching image {}/{}...", index + 1, request.images.len()));
        attach_image(&page, image, progress).await?;
    }
    if !request.images.is_empty() {
        progress("Waiting for Meta AI to finish preparing the attachment...");
        sleep(2000).await;
    }

    let before = candidate_image_keys(&page).await;
    let composer = find_composer(&page).await?.ok_or_else(|| BrowserError::page("Could not find the Meta AI prompt box."))?;
    progress("Typing the prompt...");
    enter_prompt(&page, &composer, &request.prompt, !request.images.is_empty()).await?;
    let history_before: HashSet<String> = sidebar_chat_hrefs(&page).await.into_iter().collect();

    progress("Sending the prompt to Meta AI...");
    if !click_submit_beside_instant(&page).await? {
        return Err(BrowserError::page("Could not find the Meta AI submit arrow immediately beside the Instant button."));
    }
    if open_new_sidebar_conversation(&page, &composer, &history_before, Duration::from_secs(30)).await? {
        progress("Opened the new Meta AI conversation.");
    }

    progress("Waiting for Meta AI to finish the picture...");
    let (followed, image) = wait_for_new_image(browser, page, &before, request.timeout).await?;
    page = followed;
    progress("Saving the picture from Meta AI...");
    let saved = match download_from_overlay(&page, &image, &request.out_dir).await {
        Some(path) => Ok(path),
        None => save_image_url(browser, &page, &image.src, &request.out_dir, &request.prompt, FALLBACK_NAME, true).await,
    };
    close_viewer(&page).await;
    let path = saved?;
    if !wait_for_chat(&page, &composer_locators(), Duration::from_secs(10)).await {
        progress("Meta AI saved the picture, but its viewer did not return to the chat prompt.");
    }
    Ok(path)
}

fn composer_locators() -> Vec<Loc> {
    vec![
        Loc::placeholder(re("ask meta ai", "i")),
        Loc::placeholder(re("where should we start", "i")),
        Loc::placeholder(re("ask anything", "i")),
        Loc::placeholder(re("message", "i")),
        Loc::role("textbox", Some(re("ask meta ai|message|prompt", "i"))),
        Loc::css("[contenteditable='true'][aria-label*='Ask Meta' i]"),
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

/// Wait for the prompt box, but stop at once when Meta shows its login prompt instead.
async fn wait_for_composer(page: &Page, limit: Duration) -> Result<Option<Element>> {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if let Some(found) = find_composer(page).await? {
            return Ok(Some(found));
        }
        if login_prompt_visible(page).await {
            return Ok(None);
        }
        sleep(1000).await;
    }
    Ok(None)
}

async fn login_prompt_visible(page: &Page) -> bool {
    let script = r#"(() => {
      const text = (document.body && document.body.innerText) || "";
      return /\b(log in|sign up|get more from meta ai)\b/i.test(text) && Array.from(document.querySelectorAll("button,a,[role='button']")).some((el) => {
        const rect = el.getBoundingClientRect(); const style = getComputedStyle(el);
        return rect.width > 0 && rect.height > 0 && style.display !== "none" && style.visibility !== "hidden" && /log in/i.test(el.textContent || el.getAttribute("aria-label") || "");
      });
    })()"#;
    page.eval(script).await.map(|v| v == true).unwrap_or(false)
}

/// Small pictures in the composer area: what Meta shows for an attachment (an `<img>` or a background image), as keys.
async fn attachment_preview_keys(page: &Page) -> Vec<String> {
    let script = r#"(() => {
      const viewportHeight = window.innerHeight || document.documentElement.clientHeight || 900;
      const viewportWidth = window.innerWidth || document.documentElement.clientWidth || 1600;
      const visible = (el) => {
        const rect = el.getBoundingClientRect(); const style = getComputedStyle(el);
        return rect.width > 0 && rect.height > 0 && style.display !== "none" && style.visibility !== "hidden" && rect.bottom > 0 && rect.right > 0 && rect.top < viewportHeight && rect.left < viewportWidth;
      };
      const keys = [];
      const thumb = (rect) => rect.width >= 24 && rect.height >= 24 && rect.width <= 180 && rect.height <= 180;
      const inComposer = (rect) => rect.top > viewportHeight * 0.18 && rect.left > viewportWidth * 0.18 && rect.left < viewportWidth * 0.86;
      for (const img of Array.from(document.querySelectorAll("img"))) {
        if (!visible(img)) continue;
        const rect = img.getBoundingClientRect(); const src = img.currentSrc || img.src || img.getAttribute("src") || "";
        if (thumb(rect) && inComposer(rect)) keys.push("img:" + src + "|" + Math.round(rect.left) + "," + Math.round(rect.top) + "|" + Math.round(rect.width) + "x" + Math.round(rect.height));
      }
      for (const el of Array.from(document.querySelectorAll("[style*='background-image']"))) {
        if (!visible(el)) continue;
        const rect = el.getBoundingClientRect(); const background = getComputedStyle(el).backgroundImage || "";
        if (thumb(rect) && inComposer(rect) && /url\(/i.test(background)) keys.push("bg:" + background + "|" + Math.round(rect.left) + "," + Math.round(rect.top) + "|" + Math.round(rect.width) + "x" + Math.round(rect.height));
      }
      return keys;
    })()"#;
    match page.eval(script).await {
        Ok(Value::Array(keys)) => keys.iter().filter_map(|k| k.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

async fn wait_for_attachment_preview(page: &Page, before: &[String], file: &Path, progress: Progress<'_>) {
    let known: HashSet<&String> = before.iter().collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        let after = attachment_preview_keys(page).await;
        if after.iter().any(|k| !known.contains(k)) || after.len() > before.len() {
            progress(&format!("Meta AI shows the attachment {}.", file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
            sleep(1500).await;
            return;
        }
        sleep(1000).await;
    }
    progress("The attachment preview was not detected; continuing because the file box accepted the picture.");
}

/// Attach a picture: the "+" button opens Meta's media dialog, which has a file box; its browse control opens a file dialog if not.
async fn attach_image(page: &Page, file: &Path, progress: Progress<'_>) -> Result<()> {
    let before = attachment_preview_keys(page).await;
    let openers = [
        Loc::role("button", Some(re("^\\+$", "i"))),
        Loc::css("button[aria-label*='Add' i]"),
        Loc::css("button[aria-label*='Attach' i]"),
        Loc::css("button[aria-label*='Upload' i]"),
        Loc::role("button", Some(re("attach|upload|add photo|add files|photo|image", "i"))),
        Loc::css("[data-testid*='attach' i]"),
        Loc::css("[data-testid*='upload' i]"),
        Loc::css_with_text("button", re("^\\s*\\+\\s*$", "")),
    ];
    let boxes_after_open = [
        Loc::css("[role='dialog'] input[type='file'][accept*='image' i]"),
        Loc::css("[role='dialog'] input[type='file']"),
        Loc::css("input[type='file'][accept*='image' i]"),
        Loc::css("input[type='file']"),
    ];
    if !page.click_first(&openers).await? {
        for loc in &boxes_after_open[2..] {
            if let Some(input) = page.find_existing(loc).await? {
                input.set_files(&[file.to_path_buf()]).await?;
                wait_for_attachment_preview(page, &before, file, progress).await;
                return Ok(());
            }
        }
        return Err(BrowserError::page("Could not find the Meta AI plus/upload button."));
    }
    // The plus button opens Meta's media dialog, not the system dialog: give it a moment, then use its file box.
    sleep(300).await;
    for loc in &boxes_after_open {
        if let Some(input) = page.find_existing(loc).await? {
            input.set_files(&[file.to_path_buf()]).await?;
            wait_for_attachment_preview(page, &before, file, progress).await;
            return Ok(());
        }
    }
    let browse = [Loc::text(re("click to browse", "i")), Loc::role("button", Some(re("click to browse|browse|upload", "i"))), Loc::css_with_text("[role='dialog'] *", re("browse", "i"))];
    page.choose_files(&[file.to_path_buf()], Duration::from_secs(5), || async {
        if page.click_first(&browse).await? {
            Ok(())
        } else {
            Err(BrowserError::page("Meta AI media dialog opened, but its browse control was not found."))
        }
    })
    .await?;
    wait_for_attachment_preview(page, &before, file, progress).await;
    Ok(())
}

async fn prompt_looks_entered(composer: &Element, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let wanted: String = text.chars().take(40).collect::<String>().to_lowercase();
    composer.text().await.to_lowercase().contains(&wanted)
}

async fn prompt_visible_on_page(page: &Page, text: &str) -> bool {
    let wanted: String = text.chars().take(40).collect::<String>().to_lowercase();
    let script = format!("String((document.body && document.body.innerText) || '').replace(/[\\r\\n\\t]+/g, ' ').replace(/\\s{{2,}}/g, ' ').toLowerCase().includes({})", Value::String(wanted));
    page.eval(&script).await.map(|v| v == true).unwrap_or(false)
}

/// Type the prompt. With attachments the box must not be cleared (that would drop them), so the cursor is put in the text part of the box
/// and the prompt is inserted there.
async fn enter_prompt(page: &Page, composer: &Element, prompt: &str, preserve_attachments: bool) -> Result<()> {
    let text = normalize_prompt(prompt);
    if preserve_attachments {
        match composer.rect().await {
            Ok((x, y, w, h)) => page.mouse_click(x + (w * 0.45).max(30.0).min((w - 30.0).max(0.0)), y + (h - 42.0).max(24.0), "left").await?,
            Err(_) => composer.click().await?,
        }
        sleep(250).await;
        page.insert_text(&text).await?;
        sleep(500).await;
        if prompt_looks_entered(composer, &text).await || prompt_visible_on_page(page, &text).await {
            return Ok(());
        }
        return Err(BrowserError::page("Could not enter prompt text in Meta AI without disturbing the uploaded attachment."));
    }
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
    Err(BrowserError::page("Could not enter prompt text in Meta AI."))
}

/// Click the send arrow that sits to the right of the Instant button, on the same line. Meta's arrow has no label, so it is found by its
/// icon. False when it is not there.
async fn click_submit_beside_instant(page: &Page) -> Result<bool> {
    let script = r#"(() => {
      const h3 = window.__h3;
      const instants = h3.matches({role: "button", name: {src: "^instant$", flags: "i"}}).filter((el) => h3.visible(el));
      const instant = instants[instants.length - 1];
      if (!instant) return null;
      const box = instant.getBoundingClientRect();
      const arrows = Array.from(document.querySelectorAll("svg path[d^='M16 6.125a.89.89'][d*='V25'][d*='7.5-7.5']")).map((p) => p.closest("button,[role='button']")).filter(Boolean);
      const found = [];
      for (const button of arrows) {
        if (!h3.visible(button) || button.disabled || button.getAttribute("aria-disabled") === "true") continue;
        const rect = button.getBoundingClientRect();
        const x = rect.x + rect.width / 2, y = rect.y + rect.height / 2;
        if (x <= box.x + box.width) continue;
        if (Math.abs(y - (box.y + box.height / 2)) > 36) continue;
        found.push({ x, y, distance: x - (box.x + box.width) });
      }
      found.sort((a, b) => a.distance - b.distance);
      return found[0] || null;
    })()"#;
    let target = page.eval(&format!("(() => {{ {}\nreturn {script}; }})()", crate::page::toolkit())).await?;
    match (target["x"].as_f64(), target["y"].as_f64()) {
        (Some(x), Some(y)) => {
            page.mouse_click(x, y, "left").await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The links in the sidebar (left 450 px, below the header): the conversations.
async fn sidebar_chat_hrefs(page: &Page) -> Vec<String> {
    let script = r#"(() => Array.from(document.querySelectorAll("a[href]")).filter((a) => {
      const rect = a.getBoundingClientRect(); const style = getComputedStyle(a);
      return rect.left >= 0 && rect.left < 450 && rect.top > 180 && rect.width > 40 && rect.height > 12 && style.display !== "none" && style.visibility !== "hidden";
    }).map((a) => a.href).filter(Boolean))()"#;
    match page.eval(script).await {
        Ok(Value::Array(list)) => list.iter().filter_map(|h| h.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// After sending, Meta adds the new conversation to its sidebar. Once the prompt box has been cleared, open that new link. False when none appears.
async fn open_new_sidebar_conversation(page: &Page, composer: &Element, old: &HashSet<String>, limit: Duration) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + limit;
    let known = serde_json::to_string(&old.iter().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into());
    let script = r#"(() => {
      const old = new Set(__OLD__);
      for (const a of document.querySelectorAll("a[href]")) {
        if (!a.href || old.has(a.href)) continue;
        const rect = a.getBoundingClientRect(); const style = getComputedStyle(a);
        if (rect.left < 0 || rect.left >= 450 || rect.top <= 180 || rect.width <= 40 || rect.height <= 12) continue;
        if (style.display === "none" || style.visibility === "hidden") continue;
        return a;
      }
      return null;
    })()"#
        .replace("__OLD__", &known);
    let mut cleared = false;
    while tokio::time::Instant::now() < deadline {
        if !cleared {
            cleared = composer.text().await.trim().is_empty();
            if !cleared {
                sleep(250).await;
                continue;
            }
        }
        if let Some(link) = page.eval_element(&script).await? {
            link.click().await?;
            sleep(750).await;
            return Ok(true);
        }
        sleep(250).await;
    }
    Ok(false)
}

/// The first picture that is visible, loaded and new. Meta can replace the tab while it works, so a closed tab is swapped for the open Meta tab.
async fn wait_for_new_image(browser: &Browser, mut page: Page, before: &HashSet<String>, limit: Duration) -> Result<(Page, ImageInfo)> {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if page.eval("1").await.is_err() {
            match browser.working_tab(&PREFIXES, "", URL).await {
                Ok(replacement) => page = replacement,
                Err(_) => {
                    sleep(250).await;
                    continue;
                }
            }
        }
        for info in images_on_page(&page, 200).await.into_iter().rev() {
            if info.visible && info.complete && !before.contains(&info.key) {
                return Ok((page, info));
            }
        }
        sleep(500).await;
    }
    Err(BrowserError::timeout("Timed out waiting for a generated Meta AI image."))
}

/// Hover the picture to show its Download button and use it. `None` when the picture or the button is not there, or no file came.
async fn download_from_overlay(page: &Page, image: &ImageInfo, dir: &Path) -> Option<PathBuf> {
    let script = r#"(() => {
      const target = __SRC__;
      const imgs = Array.from(document.querySelectorAll("img"));
      for (let i = imgs.length - 1; i >= 0; i--) {
        const img = imgs[i]; const rect = img.getBoundingClientRect(); const style = getComputedStyle(img);
        const src = img.currentSrc || img.src || img.getAttribute("src") || "";
        if (src === target && rect.width > 0 && rect.height > 0 && style.display !== "none" && style.visibility !== "hidden") return img;
      }
      return null;
    })()"#
        .replace("__SRC__", &json!(image.src).to_string());
    let target = page.eval_element(&script).await.ok().flatten()?;
    target.hover().await.ok()?;
    sleep(350).await;
    let controls = [Loc::role("button", Some(re("^download$", "i"))), Loc::css("button[aria-label='Download' i]"), Loc::css("button[title='Download' i]")];
    let button = page.find_first(&controls, false).await.ok().flatten()?;
    page.wait_download(dir, Duration::from_secs(30), || async { button.click().await }).await.ok()
}
