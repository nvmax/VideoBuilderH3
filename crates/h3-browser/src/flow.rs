//! Google Flow (Nano Banana) in the person's Chrome. Port of the pack's `flow_automation/flow-poc.mjs`, `flow-announcements.mjs` and
//! `flow-upload-selection.mjs`, step by step: open Flow, make sure a project is open, add the reference pictures through the
//! "Add ingredients" picker, type the prompt, submit, wait for a new generated picture and save it.
//!
//! The page text and buttons are Flow's. When Flow changes them this is the file to change, and the messages name what was not found.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fancy_regex::Regex;
use serde_json::{json, Value};

use crate::common::{escape_regex, normalize_prompt, write_picture};
use crate::error::{BrowserError, Result};
use crate::page::{re, sleep, Browser, Element, Loc, Page};
use crate::{EditRequest, Progress};

const FLOW_URL: &str = "https://labs.google/fx/tools/flow";

/// How long an upload may stay listed in the picker before the picker is used to add it, rather than waiting for Flow to add it itself.
const PICKER_GRACE: Duration = Duration::from_secs(15);

/// Make one picture in Flow and return the file it was saved to (inside `request.out_dir`).
pub async fn edit(browser: &Browser, request: &EditRequest, progress: Progress<'_>) -> Result<PathBuf> {
    for image in &request.images {
        if !image.is_file() {
            return Err(BrowserError::File(format!("Image file does not exist: {}", image.display())));
        }
    }
    browser.allow_downloads(&request.out_dir).await?;
    let page = browser.working_tab(&["https://labs.google/", "https://flow.google.com/"], "/signin", FLOW_URL).await?;
    if request.reuse_open_project {
        progress(&format!("Using the open Flow tab: {}", page.url().await));
    } else {
        progress("Opening Flow...");
        page.goto(FLOW_URL, Duration::from_secs(60)).await?;
        sleep(3000).await;
    }
    ensure_project_page(&page, progress).await?;

    for (index, image) in request.images.iter().enumerate() {
        progress(&format!("Uploading image {}/{}...", index + 1, request.images.len()));
        upload_image_and_add_to_prompt(&page, image, progress).await?;
    }

    dismiss_announcement(&page, 0).await?;
    progress("Typing the prompt...");
    match find_prompt_box(&page).await? {
        Some(prompt_box) => {
            prompt_box.click().await?;
            clear_and_type(&page, &prompt_box, &request.prompt).await?;
        }
        None => coordinate_prompt_fallback(&page, &request.prompt).await?,
    }
    sleep(2000).await;

    let before = generated_image_urls(&page).await;
    dismiss_announcement(&page, 0).await?;
    progress("Submitting the prompt to Flow...");
    if !page.click_first(&submit_locators()).await? {
        page.press_key("Enter", 0, &[]).await?;
    }

    progress("Waiting for Flow to finish the picture...");
    let image_url = wait_for_new_generated_image_url(&page, &before, request.timeout).await?;

    progress("Saving the picture from Flow...");
    match save_generated_image_url(browser, &image_url, &request.out_dir, &request.prompt).await {
        Ok(path) => Ok(path),
        Err(error) => {
            progress(&format!("Direct image save failed: {error}. Trying Flow's own Download menu..."));
            retry_download_via_context_menu(&page, &image_url, &request.out_dir, 5, Duration::from_secs(10), progress).await
        }
    }
}

/// Flow's release announcement can cover the prompt box. Click its "Get started" button, looking for up to `wait_ms`.
pub async fn dismiss_announcement(page: &Page, wait_ms: u64) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    // A "Get started" button counts only when it sits inside the announcement (its page text mentions the changelogs).
    let script = r#"(() => {
      const list = window.__h3.matches({role: "button", name: {src: "^get started$", flags: "i"}});
      for (const button of list) {
        if (!window.__h3.visible(button)) continue;
        for (let p = button.parentElement; p && p !== document.body; p = p.parentElement) {
          if (/view all changelogs/i.test(p.innerText || "")) return button;
        }
      }
      return null;
    })()"#;
    loop {
        if page.count(&Loc::role("button", Some(re("^get started$", "i")))).await? > 0 {
            if let Some(button) = page.eval_element_with_toolkit(script).await? {
                button.click().await?;
                for _ in 0..20 {
                    if !button.is_visible().await {
                        break;
                    }
                    sleep(250).await;
                }
                return Ok(true);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        sleep(250).await;
    }
}

/// Make sure the composer ("What do you want to create?") is on screen: if Flow shows its home page, start a new project.
async fn ensure_project_page(page: &Page, progress: Progress<'_>) -> Result<()> {
    page.bring_to_front().await;
    dismiss_announcement(page, 3000).await?;
    if find_prompt_box(page).await?.is_some() {
        return Ok(());
    }
    let url = page.url().await;
    if url.contains("accounts.google.com") || url.contains("/signin") {
        return Err(BrowserError::page("Flow wants you to sign in to Google first. Use \"Open and sign in\" for Flow, sign in in that Chrome window, then try again."));
    }
    let new_project = [
        Loc::role("button", Some(re("new project", "i"))),
        Loc::text(re("new project", "i")),
        Loc::css_with_text("button", re("New project", "i")),
        Loc::css_with_text("[role='button']", re("New project", "i")),
    ];
    if !page.click_first(&new_project).await? {
        return Err(BrowserError::page("Could not find the New project button on the Flow home page. If Flow shows a sign-in page, use \"Open and sign in\" for Flow first."));
    }
    progress("Opened a new Flow project.");
    sleep(5000).await;
    if wait_for_prompt_box(page, Duration::from_secs(60)).await?.is_none() {
        return Err(BrowserError::page("Clicked New project, but the project prompt box did not appear."));
    }
    Ok(())
}

async fn wait_for_prompt_box(page: &Page, limit: Duration) -> Result<Option<Element>> {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        dismiss_announcement(page, 0).await?;
        if let Some(found) = find_prompt_box(page).await? {
            return Ok(Some(found));
        }
        sleep(1000).await;
    }
    Ok(None)
}

fn prompt_box_locators() -> Vec<Loc> {
    vec![
        Loc::placeholder(re("what do you want to create", "i")),
        Loc::role("textbox", Some(re("what do you want to create", "i"))),
        Loc::css("textarea[placeholder*='create' i]"),
        Loc::css("[contenteditable='true'][aria-label*='create' i]"),
        Loc::css("[role='textbox'][aria-label*='create' i]"),
        Loc::css("textarea"),
        Loc::css("[contenteditable='true']"),
        Loc::css("[contenteditable='plaintext-only']"),
        Loc::css("[role='textbox']"),
        Loc::css(".ProseMirror"),
        Loc::css("input[type='text']"),
    ]
}

async fn find_prompt_box(page: &Page) -> Result<Option<Element>> {
    page.find_first(&prompt_box_locators(), false).await
}

fn submit_locators() -> Vec<Loc> {
    vec![
        Loc::role("button", Some(re("^start generation$", "i"))),
        Loc::role("button", Some(re("submit|send|create|generate", "i"))),
        Loc::css("button[aria-label*='Submit' i]"),
        Loc::css("button[aria-label*='Send' i]"),
        Loc::css("button[aria-label*='Create' i]"),
        Loc::css("button[aria-label*='Generate' i]"),
        Loc::icon("button", "arrow_forward"),
        Loc::css("[role='button'][aria-label*='Submit' i]"),
        Loc::css("[role='button'][aria-label*='Send' i]"),
    ]
}

/// Last resort when no prompt box is found by its markup: click where Flow keeps it and type there.
async fn coordinate_prompt_fallback(page: &Page, text: &str) -> Result<()> {
    page.bring_to_front().await;
    let size = page.eval("[window.innerWidth, window.innerHeight]").await?;
    let (width, height) = (size[0].as_f64().unwrap_or(1600.0), size[1].as_f64().unwrap_or(950.0));
    page.mouse_click((width * 0.43).round(), (height * 0.93).round(), "left").await?;
    sleep(300).await;
    let _ = page.select_all().await;
    let _ = page.press_key("Backspace", 0, &[]).await;
    page.insert_text(&normalize_prompt(text)).await
}

async fn prompt_looks_entered(prompt_box: &Element, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let wanted: String = text.chars().take(40).collect::<String>().to_lowercase();
    prompt_box.text().await.to_lowercase().contains(&wanted)
}

/// Replace what is in the box with `text`: select all and insert it, then check it stuck; if not, clear with Backspace and insert again.
async fn clear_and_type(page: &Page, prompt_box: &Element, text: &str) -> Result<()> {
    let text = normalize_prompt(text);
    prompt_box.click().await?;
    let _ = page.select_all().await;
    page.insert_text(&text).await?;
    sleep(300).await;
    if prompt_looks_entered(prompt_box, &text).await {
        return Ok(());
    }
    let _ = page.select_all().await;
    let _ = page.press_key("Backspace", 0, &[]).await;
    page.insert_text(&text).await?;
    sleep(500).await;
    if prompt_looks_entered(prompt_box, &text).await {
        return Ok(());
    }
    // Last try: the clipboard.
    let _ = page.select_all().await;
    let _ = page.press_key("Backspace", 0, &[]).await;
    let copied = page.eval(&format!("navigator.clipboard.writeText({}).then(() => true)", Value::String(text.clone()))).await.is_ok();
    if !copied {
        return Err(BrowserError::page("Could not enter the prompt text: typing did not stick and the browser blocked the clipboard."));
    }
    page.press_key("v", Page::shortcut_modifier(), &["paste"]).await
}

/// The pictures Flow has generated on the page: its own tiles that offer "Reuse prompt" (uploaded references do not), or any picture
/// whose alt text says "generated image". Full addresses.
async fn generated_image_urls(page: &Page) -> Vec<String> {
    let script = r#"(() => {
      const urls = [];
      for (const img of document.querySelectorAll("img")) {
        const src = img.currentSrc || img.src || img.getAttribute("src") || "";
        const alt = img.getAttribute("alt") || "";
        const rect = img.getBoundingClientRect();
        const visible = rect.width > 50 && rect.height > 50 && getComputedStyle(img).display !== "none" && getComputedStyle(img).visibility !== "hidden";
        const tile = img.closest("flow-grid-tile-container");
        // The new Flow grid uses the same alt text for uploads and generated images.
        // Generated tiles expose Reuse prompt; uploaded reference tiles do not.
        const isGenerated = tile
          ? Boolean(tile.querySelector('button[aria-label="Reuse prompt"]'))
          : alt.toLowerCase().includes("generated image") || src.includes("media.getMediaUrlRedirect");
        if (!visible || !isGenerated || !src || !img.complete || img.naturalWidth === 0) continue;
        urls.push(new URL(src, window.location.href).href);
      }
      return urls;
    })()"#;
    match page.eval(script).await {
        Ok(Value::Array(urls)) => urls.iter().filter_map(|u| u.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// What makes two addresses the same picture: Flow's CDN signatures and display sizes change while the media stays the same.
pub fn image_url_key(value: &str) -> String {
    let Ok(url) = reqwest::Url::parse(value) else { return value.to_string() };
    let host = url.host_str().unwrap_or("");
    if host == "flow-content.google" && url.path().starts_with("/image/") {
        return url.path().to_string();
    }
    if host == "flow.google.com" && url.path().starts_with("/asb/") {
        // `=s512`, `=s2048-rj` and so on at the end of the path.
        let sizes = Regex::new(r"=s\d+(?:-[a-z]+)*$").expect("a valid pattern");
        return sizes.replace(url.path(), "").into_owned();
    }
    value.to_string()
}

async fn wait_for_new_generated_image_url(page: &Page, before: &[String], limit: Duration) -> Result<String> {
    let known: std::collections::HashSet<String> = before.iter().map(|u| image_url_key(u)).collect();
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let fresh: Vec<String> = generated_image_urls(page).await.into_iter().filter(|u| !known.contains(&image_url_key(u))).collect();
        if let Some(newest) = fresh.last() {
            wait_for_image_url_to_settle(page, newest, Duration::from_secs(10)).await;
            return Ok(newest.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BrowserError::timeout("Timed out waiting for a new completed Flow image; existing images were not downloaded."));
        }
        sleep(2500).await;
    }
}

async fn wait_for_image_url_to_settle(page: &Page, url: &str, limit: Duration) {
    let script = format!(
        r#"(() => {{
          const target = {};
          for (const img of document.querySelectorAll("img")) {{
            const src = new URL(img.currentSrc || img.src || img.getAttribute("src") || "", window.location.href).href;
            if (src === target && img.complete && img.naturalWidth > 0 && img.naturalHeight > 0) return true;
          }}
          return false;
        }})()"#,
        Value::String(url.to_string())
    );
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if page.eval(&script).await.map(|v| v == true).unwrap_or(false) {
            return;
        }
        sleep(500).await;
    }
}

/// Open the picture's address in a temporary tab and write what Chrome received.
async fn save_generated_image_url(browser: &Browser, url: &str, dir: &Path, prompt: &str) -> Result<PathBuf> {
    let (content_type, bytes) = browser.fetch_image(url, Duration::from_secs(60)).await?;
    write_picture(dir, prompt, "flow-output", &content_type, &bytes)
}

async fn retry_download_via_context_menu(page: &Page, url: &str, dir: &Path, attempts: u32, delay: Duration, progress: Progress<'_>) -> Result<PathBuf> {
    let mut last = BrowserError::page("Download failed after retries.");
    for attempt in 1..=attempts {
        progress(&format!("Trying Flow's right-click Download > 2K menu, attempt {attempt}/{attempts}..."));
        match download_via_context_menu(page, url, dir).await {
            Ok(path) => return Ok(path),
            Err(error) => {
                progress(&format!("Download attempt {attempt} failed: {error}"));
                last = error;
                if attempt < attempts {
                    let _ = page.press_key("Escape", 0, &[]).await;
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Err(last)
}

async fn download_via_context_menu(page: &Page, url: &str, dir: &Path) -> Result<PathBuf> {
    let script = format!(
        r#"(() => {{
          const target = {};
          const imgs = Array.from(document.querySelectorAll("img"));
          for (let i = imgs.length - 1; i >= 0; i--) {{
            const img = imgs[i];
            const src = new URL(img.currentSrc || img.src || img.getAttribute("src") || "", window.location.href).href;
            const rect = img.getBoundingClientRect();
            if (src === target && rect.width > 50 && rect.height > 50) return img;
          }}
          return null;
        }})()"#,
        Value::String(url.to_string())
    );
    let image = page.eval_element(&script).await?.ok_or_else(|| BrowserError::page("Could not find the generated image element for the Flow download menu."))?;
    page.bring_to_front().await;
    image.right_click().await?;
    sleep(700).await;

    let download_item = page
        .wait_for_any(&[Loc::role("menuitem", Some(re("^download$", "i"))), Loc::css_with_text("[role='menuitem']", re("Download", "i")), Loc::text(re("download", "i"))], Duration::from_secs(10))
        .await?
        .ok_or_else(|| BrowserError::page("Could not find Download in the Flow context menu."))?;
    download_item.hover().await?;
    sleep(1500).await;

    let option_2k = find_2k_option(page, Duration::from_secs(20)).await?.ok_or_else(|| BrowserError::page("Could not find the 2K download option in the Flow context menu."))?;
    page.wait_download(dir, Duration::from_secs(180), || async { option_2k.click().await }).await
}

async fn find_2k_option(page: &Page, limit: Duration) -> Result<Option<Element>> {
    let script = r#"(() => {
      const visible = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.display !== "none" && s.visibility !== "hidden"; };
      for (const el of document.querySelectorAll("[role='menuitem'], [role='option'], button, div")) {
        if (!visible(el)) continue;
        const text = (el.innerText || el.textContent || "").replace(/\s+/g, " ").trim();
        if (/^2K(\s|$)/i.test(text) || text.toLowerCase().includes("2k upscaled")) return el.closest("[role='menuitem'], button") || el;
      }
      return null;
    })()"#;
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if let Some(found) = page.eval_element(script).await? {
            return Ok(Some(found));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(500).await;
    }
}

/// The "Ingredient" chips in Flow's prompt box: how many there are, and how many have their picture loaded.
async fn ingredient_chips(page: &Page) -> (usize, usize) {
    let script = r#"(() => {
      const chips = [...new Set(document.querySelectorAll("button.chip-container, flow-ingredient-chip button"))];
      const ready = chips.filter((chip) => {
        const img = chip.querySelector("img");
        return chip.getAttribute("aria-busy") !== "true" && img && img.complete && img.naturalWidth > 0;
      });
      return [chips.length, ready.length];
    })()"#;
    match page.eval(script).await {
        Ok(Value::Array(counts)) if counts.len() == 2 => (counts[0].as_u64().unwrap_or(0) as usize, counts[1].as_u64().unwrap_or(0) as usize),
        _ => (0, 0),
    }
}

/// Open the "Add ingredients" picker, upload the file through it and add it to the prompt.
async fn upload_image_and_add_to_prompt(page: &Page, file: &Path, progress: Progress<'_>) -> Result<()> {
    dismiss_announcement(page, 0).await?;
    let (chips_before, _) = ingredient_chips(page).await;
    let opener = [
        Loc::role("button", Some(re("^add ingredients(?: to the prompt box)?$", "i"))),
        Loc::role("button", Some(re("^create$", "i"))),
        Loc::icon("button[aria-haspopup='dialog']", "add_2"),
        Loc::icon("button", "add_2"),
        Loc::icon("button", "add"),
    ];
    if !page.wait_and_click_first(&opener, Duration::from_secs(12)).await? {
        return Err(BrowserError::page("Could not find the Add ingredients / Create button to open media upload."));
    }
    sleep(800).await;
    let upload_target = page
        .wait_for_any(
            &[
                Loc::text(re("upload media", "i")),
                Loc::role("button", Some(re("upload media", "i"))),
                Loc::css_with_text("[role='button']", re("Upload media", "i")),
                Loc::css_with_text("button", re("Upload media", "i")),
            ],
            Duration::from_secs(15),
        )
        .await?
        .ok_or_else(|| BrowserError::page("Could not find Upload media in the media picker."))?;

    page.choose_files(&[file.to_path_buf()], Duration::from_secs(15), || async { upload_target.click().await }).await?;
    progress("Upload selected; waiting for Flow to add it to the prompt...");
    sleep(2000).await;

    add_uploaded_image_to_prompt(page, file, chips_before, Duration::from_secs(90)).await?;
    progress("Uploaded image added to the prompt.");
    sleep(3000).await;
    Ok(())
}

/// Wait until the uploaded picture is in the prompt box. Flow's current prompt box takes an upload straight in as an "Ingredient" chip
/// (its picker then lists no asset); an older Flow lists the file in the picker and needs "Add to prompt", which is the fallback.
async fn add_uploaded_image_to_prompt(page: &Page, file: &Path, chips_before: usize, limit: Duration) -> Result<()> {
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let asset_loc = Loc(json!({"role": "option", "hasText": re(&format!("(?:^|\\s){}(?:\\s|$)", escape_regex(&name)), "")}));
    let deadline = tokio::time::Instant::now() + limit;
    // The picker lists the file for a moment while Flow finishes the upload and puts it in the prompt box itself, so the picker is used
    // only when the file stays listed and no chip appears.
    let mut listed_since: Option<tokio::time::Instant> = None;
    loop {
        let (chips, ready) = ingredient_chips(page).await;
        if chips > chips_before && ready == chips {
            return Ok(());
        }
        if page.find(&asset_loc, false).await?.is_some() {
            let since = *listed_since.get_or_insert_with(tokio::time::Instant::now);
            if since.elapsed() >= PICKER_GRACE {
                return add_from_picker(page, file, chips_before, deadline.saturating_duration_since(tokio::time::Instant::now())).await;
            }
        } else {
            listed_since = None;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BrowserError::page(format!("Flow uploaded {name}, but it did not appear in the prompt box as an ingredient and its asset did not appear in the ingredients picker.")));
        }
        sleep(500).await;
    }
}

/// Pick the uploaded file in the picker and press "Add to prompt", but only when the preview shows that file: Flow first previews the
/// account avatar, which must never be added.
async fn add_from_picker(page: &Page, file: &Path, chips_before: usize, limit: Duration) -> Result<()> {
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let asset_loc = Loc(json!({"role": "option", "hasText": re(&format!("(?:^|\\s){}(?:\\s|$)", escape_regex(&name)), "")}));
    let deadline = tokio::time::Instant::now() + limit;

    let asset = loop {
        if let Some(found) = page.find(&asset_loc, false).await? {
            break found;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BrowserError::page(format!("Flow uploaded {name}, but its image asset did not appear in the ingredients picker.")));
        }
        sleep(500).await;
    };
    let loaded_src = r#"function() { const img = this.querySelector("img"); return img && img.complete && img.naturalWidth > 0 ? (img.currentSrc || img.src) : ""; }"#;
    loop {
        if asset.call(loaded_src, &[]).await.ok().and_then(|v| v.as_str().map(|s| !s.is_empty())).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BrowserError::page(format!("Flow has not finished loading uploaded reference {name}.")));
        }
        sleep(250).await;
    }
    // The picker may close under us when Flow adds the upload by itself; the loop below then sees the chip.
    let _ = asset.click().await;

    let add_loc = Loc::role("button", Some(re("^add to prompt$", "i")));
    loop {
        let (chips, ready) = ingredient_chips(page).await;
        if chips > chips_before && ready == chips {
            return Ok(());
        }
        let selection = asset
            .call(r#"function() { const img = this.querySelector("img"); return {active: this.classList.contains("asset-item-active"), src: img && img.complete && img.naturalWidth > 0 ? (img.currentSrc || img.src) : ""}; }"#, &[])
            .await
            .unwrap_or(Value::Null);
        let src = selection["src"].as_str().unwrap_or("").to_string();
        if selection["active"] == true && !src.is_empty() {
            if let Some(add) = page.find(&add_loc, false).await? {
                if add.is_visible().await && add.is_enabled().await {
                    let correct_preview = add
                        .call(
                            r#"function(expected) {
                              const key = (src) => { const url = new URL(src, location.href); return url.origin + url.pathname.replace(/=s\d+(?:-[a-z]+)*$/i, ""); };
                              for (let parent = this.parentElement; parent && parent !== document.body; parent = parent.parentElement) {
                                const previews = [...parent.querySelectorAll("img")].filter((img) => !img.closest('[role="option"]'));
                                if (!previews.length) continue;
                                return previews.some((img) => img.complete && img.naturalWidth > 0 && key(img.currentSrc || img.src) === key(expected));
                              }
                              return false;
                            }"#,
                            &[Value::String(src)],
                        )
                        .await?;
                    if correct_preview == true {
                        add.click().await?;
                        return Ok(());
                    }
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BrowserError::page(format!("Flow did not preview uploaded reference {name}; refusing to add a different image or avatar.")));
        }
        sleep(250).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_addresses_of_one_flow_picture_have_the_same_key() {
        let a = image_url_key("https://flow-content.google/image/abc123?sig=1");
        let b = image_url_key("https://flow-content.google/image/abc123?sig=2&x=3");
        assert_eq!((a.as_str(), a == b), ("/image/abc123", true));
        let small = image_url_key("https://flow.google.com/asb/xyz=s512");
        let big = image_url_key("https://flow.google.com/asb/xyz=s2048-rj");
        assert_eq!((small.as_str(), small == big), ("/asb/xyz", true));
        // Any other address is its own key, and so is something that is not an address at all.
        assert_eq!(image_url_key("https://example.com/a.png?x=1"), "https://example.com/a.png?x=1");
        assert_eq!(image_url_key("not a url"), "not a url");
        assert_ne!(image_url_key("https://flow-content.google/image/one"), image_url_key("https://flow-content.google/image/two"));
    }

    #[test]
    fn a_file_name_is_matched_whole_in_the_picker() {
        let pattern = Regex::new(&format!("(?:^|\\s){}(?:\\s|$)", escape_regex("source_0001 (1).png"))).unwrap();
        assert!(pattern.is_match("source_0001 (1).png").unwrap());
        assert!(pattern.is_match("Image source_0001 (1).png uploaded").unwrap());
        assert!(!pattern.is_match("my_source_0001 (1).png").unwrap());
        assert!(!pattern.is_match("source_0001 (1).pngx").unwrap());
    }
}
