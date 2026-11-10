//! What the three sites' drivers share: how a prompt and a file name are made, how a picture found on a page is saved, and the
//! image viewer that ChatGPT Images and Meta AI open over the chat. Ports of the helpers every `flow_automation/*-poc.mjs` repeats.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::{BrowserError, Result};
use crate::page::{re, sleep, Browser, Element, Loc, Page};

/// The sites take the prompt as one line.
pub fn normalize_prompt(value: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    out
}

/// Lower case letters and digits joined with dashes, at most 80 characters; `fallback` when nothing is left.
pub fn safe_filename(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(ch);
        } else {
            dash = true;
        }
    }
    let out: String = out.chars().take(80).collect();
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// `.jpg`, `.webp` or `.png` for a content type.
pub fn extension_for(content_type: &str) -> &'static str {
    let lower = content_type.to_lowercase();
    if lower.contains("jpeg") || lower.contains("jpg") {
        ".jpg"
    } else if lower.contains("webp") {
        ".webp"
    } else {
        ".png"
    }
}

pub fn unique_name(base: &str, extension: &str) -> String {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("{base}-{stamp}{extension}")
}

/// Write a picture the page gave us into `dir`, named after the prompt.
pub fn write_picture(dir: &Path, prompt: &str, fallback: &str, content_type: &str, bytes: &[u8]) -> Result<PathBuf> {
    if bytes.is_empty() {
        return Err(BrowserError::page("Direct image save failed: no data"));
    }
    let path = dir.join(unique_name(&safe_filename(prompt, fallback), extension_for(content_type)));
    std::fs::write(&path, bytes).map_err(|e| BrowserError::File(format!("Could not save {}: {e}", path.display())))?;
    Ok(path)
}

/// `regex` text for a file name that matches only that whole name.
pub fn escape_regex(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if "\\^$.|?*+()[]{}/-".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// A picture on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageInfo {
    pub src: String,
    /// The address and the size on screen: two sightings of the same picture have the same key.
    pub key: String,
    pub visible: bool,
    pub complete: bool,
    pub area: f64,
}

/// Every `<img>` of the page in document order, with its size on screen. `min_side` is how wide and high a picture must be to count as visible
/// (the chats use 160 to remember what was there before and 200 to accept a new one).
pub async fn images_on_page(page: &Page, min_side: u32) -> Vec<ImageInfo> {
    let script = r#"(() => Array.from(document.querySelectorAll("img")).map((img) => {
      const rect = img.getBoundingClientRect(); const style = getComputedStyle(img);
      const src = img.currentSrc || img.src || img.getAttribute("src") || "";
      return {
        src, key: src + "|" + Math.round(rect.width) + "x" + Math.round(rect.height), area: rect.width * rect.height,
        visible: rect.width > __MIN__ && rect.height > __MIN__ && style.display !== "none" && style.visibility !== "hidden",
        complete: img.complete && img.naturalWidth > 0 && img.naturalHeight > 0,
      };
    }))()"#
        .replace("__MIN__", &min_side.to_string());
    let Ok(Value::Array(list)) = page.eval(&script).await else { return Vec::new() };
    list.iter()
        .map(|item| ImageInfo {
            src: item["src"].as_str().unwrap_or("").to_string(),
            key: item["key"].as_str().unwrap_or("").to_string(),
            visible: item["visible"] == true,
            complete: item["complete"] == true,
            area: item["area"].as_f64().unwrap_or(0.0),
        })
        .collect()
}

/// The keys of the pictures that are on the page now (wider and higher than 160), to tell a new one from them later.
pub async fn candidate_image_keys(page: &Page) -> std::collections::HashSet<String> {
    images_on_page(page, 160).await.into_iter().filter(|i| i.visible).map(|i| i.key).collect()
}

/// The largest picture that is visible and loaded, wider and higher than 200.
pub async fn newest_visible_image(page: &Page) -> Result<ImageInfo> {
    let mut found: Vec<ImageInfo> = images_on_page(page, 200).await.into_iter().filter(|i| i.visible && i.complete && !i.src.is_empty()).collect();
    found.sort_by(|a, b| b.area.partial_cmp(&a.area).unwrap_or(std::cmp::Ordering::Equal));
    found.into_iter().next().ok_or_else(|| BrowserError::page("Could not find a visible image URL to save."))
}

/// Click the picture the page shows for `wanted` (same key, else same address, else the largest one) to open the site's viewer.
pub async fn click_generated_image(page: &Page, wanted: &ImageInfo) -> Result<bool> {
    let script = r#"(() => {
      const wanted = __WANTED__;
      const images = Array.from(document.querySelectorAll("img")).map((img) => {
        const rect = img.getBoundingClientRect(); const style = getComputedStyle(img);
        const src = img.currentSrc || img.src || img.getAttribute("src") || "";
        return { img, src, key: src + "|" + Math.round(rect.width) + "x" + Math.round(rect.height), area: rect.width * rect.height,
          ok: rect.width > 200 && rect.height > 200 && style.display !== "none" && style.visibility !== "hidden" && img.complete && img.naturalWidth > 0 && img.naturalHeight > 0 && src };
      }).filter((item) => item.ok);
      const last = (test) => { for (let i = images.length - 1; i >= 0; i--) if (test(images[i])) return images[i]; return null; };
      const exact = last((item) => item.key === wanted.key);
      const same = last((item) => item.src === wanted.src);
      const largest = images.slice().sort((a, b) => b.area - a.area)[0];
      const chosen = exact || same || largest;
      return chosen ? chosen.img : null;
    })()"#
        .replace("__WANTED__", &serde_json::json!({"key": wanted.key, "src": wanted.src}).to_string());
    for _ in 0..3 {
        let Some(image) = page.eval_element(&script).await? else { return Ok(false) };
        if image.click().await.is_ok() {
            return Ok(true);
        }
        sleep(1000).await;
    }
    Ok(false)
}

/// Read a picture with the page's own `fetch` (its cookies and sign-in apply). The content type and the bytes.
pub async fn fetch_in_page(page: &Page, url: &str) -> Result<(String, Vec<u8>)> {
    use base64::Engine;
    let script = r#"(async () => {
      const response = await fetch(__URL__, { credentials: "include" });
      if (!response.ok) return { error: response.status + " " + response.statusText };
      const blob = await response.blob();
      const base64 = await new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result).split(",")[1] || "");
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(blob);
      });
      return { contentType: response.headers.get("content-type") || "image/png", base64 };
    })()"#
        .replace("__URL__", &Value::String(url.to_string()).to_string());
    let answer = page.eval(&script).await?;
    if let Some(error) = answer["error"].as_str() {
        return Err(BrowserError::page(format!("Direct image save failed: {error}")));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(answer["base64"].as_str().unwrap_or("")).map_err(|e| BrowserError::page(format!("The picture came back unreadable: {e}")))?;
    if bytes.is_empty() {
        return Err(BrowserError::page("Direct image save failed: no data"));
    }
    Ok((answer["contentType"].as_str().unwrap_or("image/png").to_string(), bytes))
}

/// Save the picture at `url`: through a temporary tab and through the page's own fetch, in the order `tab_first` says.
pub async fn save_image_url(browser: &Browser, page: &Page, url: &str, dir: &Path, prompt: &str, fallback: &str, tab_first: bool) -> Result<PathBuf> {
    let (content_type, bytes) = if tab_first {
        match browser.fetch_image(url, Duration::from_secs(60)).await {
            Ok(done) => done,
            Err(tab_error) => fetch_in_page(page, url).await.map_err(|e| BrowserError::page(format!("Direct image save failed. Browser tab: {tab_error}. In-page request: {e}")))?,
        }
    } else {
        match fetch_in_page(page, url).await {
            Ok(done) => done,
            Err(page_error) => browser.fetch_image(url, Duration::from_secs(60)).await.map_err(|e| BrowserError::page(format!("Direct image save failed. In-page request: {page_error}. Browser tab: {e}")))?,
        }
    };
    write_picture(dir, prompt, fallback, &content_type, &bytes)
}

/// Close the site's image viewer if one is open: its Close button, else Escape.
pub async fn close_viewer(page: &Page) {
    let controls = [
        Loc::role("button", Some(re("^close(?: (?:image(?: viewer)?|viewer|preview|dialog))?$", "i"))),
        Loc::css("button[title='Close' i], button[title='Close image' i], button[title='Close image viewer' i]"),
    ];
    for loc in &controls {
        if let Ok(Some(button)) = page.find(loc, false).await {
            if button.click().await.is_ok() {
                for _ in 0..12 {
                    if !button.is_visible().await {
                        return;
                    }
                    sleep(250).await;
                }
            }
        }
    }
    let _ = page.press_key("Escape", 0, &[]).await;
}

/// Wait until the chat's prompt box can take the next request again (a viewer over it blocks it). False after `limit`.
pub async fn wait_for_chat(page: &Page, composer: &[Loc], limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(found)) = page.find_first(composer, false).await {
            if element_receives_clicks(&found).await {
                return true;
            }
        }
        sleep(250).await;
    }
    false
}

/// Is the middle of the element the thing a click there would reach (nothing covers it)?
pub async fn element_receives_clicks(element: &Element) -> bool {
    element
        .call(
            "function() { const r = this.getBoundingClientRect(); if (r.width <= 0 || r.height <= 0) return false; const top = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2); return !!top && (top === this || this.contains(top) || top.contains(this)); }",
            &[],
        )
        .await
        .map(|v| v == true)
        .unwrap_or(false)
}

/// Click the viewer's download control: one named Download, else (when `share_heuristic`) the control to the right of Share in the top bar.
/// False when none was found.
pub async fn click_download_button(page: &Page, share_heuristic: bool) -> Result<bool> {
    let labeled = [
        Loc::role("button", Some(re("download", "i"))),
        Loc::role("link", Some(re("download", "i"))),
        Loc::css("button[aria-label*='Download' i]"),
        Loc::css("a[aria-label*='Download' i]"),
        Loc::css("button[title*='Download' i]"),
        Loc::css("a[title*='Download' i]"),
        Loc::css("a[download]"),
    ];
    if page.click_first(&labeled).await? {
        return Ok(true);
    }
    let script = r#"(() => {
      const isVisible = (el) => { const rect = el.getBoundingClientRect(); const style = getComputedStyle(el); return rect.width > 0 && rect.height > 0 && style.display !== "none" && style.visibility !== "hidden"; };
      const nameOf = (el) => [el.getAttribute("aria-label"), el.getAttribute("title"), el.getAttribute("data-testid"), el.textContent].filter(Boolean).join(" ").trim();
      const controls = Array.from(document.querySelectorAll("button,a,[role='button']")).filter((el) => isVisible(el)).map((el) => ({ el, rect: el.getBoundingClientRect(), name: nameOf(el), svg: (el.querySelector("svg") || {}).outerHTML || "" }));
      const named = controls.find((item) => /download/i.test(item.name) || item.el.hasAttribute("download"));
      if (named) { named.el.click(); return true; }
      const icon = controls.find((item) => { const svg = item.svg.toLowerCase(); return svg.includes("download") || svg.includes("m21 15v4") || svg.includes("m7 10l5 5") || svg.includes("arrow-down"); });
      if (icon) { icon.el.click(); return true; }
      if (!__SHARE__) return false;
      const share = controls.filter((item) => /^share$/i.test(item.name.trim()) || /\bshare\b/i.test(item.name)).sort((a, b) => b.rect.width * b.rect.height - a.rect.width * a.rect.height)[0];
      if (!share) return false;
      const right = controls.filter((item) => {
        const x = item.rect.left + item.rect.width / 2; const y = item.rect.top + item.rect.height / 2; const name = item.name.trim();
        return x > share.rect.right && y < 140 && !/more|menu|overflow|ellipsis|^\.\.\.$/i.test(name);
      }).sort((a, b) => a.rect.left - b.rect.left);
      if (!right[0]) return false;
      right[0].el.click(); return true;
    })()"#
        .replace("__SHARE__", if share_heuristic { "true" } else { "false" });
    Ok(page.eval(&script).await.map(|v| v == true).unwrap_or(false))
}

/// Click the viewer's download control and wait for the file Chrome saves. `None` when there is no control or nothing was downloaded.
pub async fn download_from_viewer(page: &Page, dir: &Path, share_heuristic: bool, wait: Duration) -> Option<PathBuf> {
    page.wait_download(dir, wait, || async {
        if click_download_button(page, share_heuristic).await? {
            Ok(())
        } else {
            Err(BrowserError::page("No download button was found."))
        }
    })
    .await
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_one_line_with_single_spaces() {
        assert_eq!(normalize_prompt("  Add the title\r\n  HEAT.\t\tBig   and bold  "), "Add the title HEAT. Big and bold");
        assert_eq!(normalize_prompt(""), "");
    }

    #[test]
    fn saved_files_are_named_after_the_prompt_and_the_content_type() {
        assert_eq!(safe_filename("Add the title: HEAT!!", "x"), "add-the-title-heat");
        assert_eq!(safe_filename("   ", "chatgpt-image"), "chatgpt-image");
        assert_eq!(safe_filename(&"word ".repeat(40), "x").len(), 79);
        assert_eq!((extension_for("image/jpeg"), extension_for("image/webp"), extension_for("image/png"), extension_for("")), (".jpg", ".webp", ".png", ".png"));
        let first = unique_name("thumb", ".png");
        assert!(first.starts_with("thumb-") && first.ends_with(".png"));
        assert_eq!(escape_regex("a.b-c(d)"), "a\\.b\\-c\\(d\\)");
    }

    #[test]
    fn a_picture_from_the_page_is_written_named_after_the_prompt_and_nothing_empty_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_picture(dir.path(), "Add the title HEAT", "flow-output", "image/jpeg", b"jpegdata").unwrap();
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("add-the-title-heat-") && path.extension().unwrap() == "jpg");
        assert_eq!(std::fs::read(&path).unwrap(), b"jpegdata");
        assert_eq!(write_picture(dir.path(), "x", "flow-output", "image/png", b"").unwrap_err().to_string(), "Direct image save failed: no data");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
