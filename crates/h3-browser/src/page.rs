//! Tabs and the things a person does in one: find a button or a box, click, type, choose a file, wait for a download.
//!
//! The scripts this replaces used Playwright's locators (`getByRole`, `getByText`, `:has-text`). Here a locator is a small JSON spec
//! ([`Loc`]) that a page-side toolkit ([`TOOLKIT`]) turns into elements. The toolkit works from the end of the list to the start and
//! keeps the first element that is visible (and, when asked, not disabled), as the scripts did with `locator.nth(count - 1)` down to 0.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::cdp::Cdp;
use crate::error::{BrowserError, Result};

/// The page side of the locators. It is put in front of every call that needs it and defines `window.__h3` once per page.
const TOOLKIT: &str = r#"if (!window.__h3) { window.__h3 = (() => {
  const norm = (t) => String(t == null ? "" : t).replace(/\s+/g, " ").trim();
  const textOf = (el) => norm(el.innerText || el.textContent);
  const visible = (el) => {
    const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.display !== "none" && s.visibility !== "hidden";
  };
  const rx = (o) => new RegExp(o.src, o.flags || "");
  const implicitRole = (el) => {
    const tag = el.tagName.toLowerCase();
    if (tag === "button" || tag === "summary") return "button";
    if (tag === "a" && el.hasAttribute("href")) return "link";
    if (tag === "textarea") return "textbox";
    if (tag === "option") return "option";
    if (tag === "input") {
      const t = (el.getAttribute("type") || "text").toLowerCase();
      if (["button", "submit", "reset"].includes(t)) return "button";
      if (t === "checkbox") return "checkbox";
      if (t === "radio") return "radio";
      if (["text", "search", "email", "url", "tel", "password"].includes(t)) return "textbox";
    }
    return "";
  };
  const roleOf = (el) => (el.getAttribute("role") || implicitRole(el)).split(/\s+/)[0];
  const nameOf = (el) => {
    const label = el.getAttribute("aria-label"); if (label) return norm(label);
    const by = el.getAttribute("aria-labelledby");
    if (by) {
      const t = norm(by.split(/\s+/).map((id) => { const n = document.getElementById(id); return n ? textOf(n) : ""; }).join(" "));
      if (t) return t;
    }
    if (el.tagName === "INPUT" && ["button", "submit", "reset"].includes((el.type || "").toLowerCase())) return norm(el.value);
    if (el.id) { const l = Array.from(document.querySelectorAll("label")).find((x) => x.htmlFor === el.id); if (l) return textOf(l); }
    const own = textOf(el); if (own) return own;
    return norm(el.getAttribute("title") || el.getAttribute("placeholder") || el.getAttribute("alt") || "");
  };
  const roleSelector = (role) => ({
    button: "button,summary,[role=button],input[type=button],input[type=submit],input[type=reset]",
    link: "a[href],[role=link]",
    textbox: "textarea,input,[role=textbox]",
    option: "option,[role=option]",
    checkbox: "input[type=checkbox],[role=checkbox]",
    radio: "input[type=radio],[role=radio]",
  })[role] || '[role="' + role + '"]';
  const matches = (spec) => {
    let list = [];
    if (spec.css) list = Array.from(document.querySelectorAll(spec.css));
    else if (spec.role) list = Array.from(document.querySelectorAll(roleSelector(spec.role))).filter((el) => roleOf(el) === spec.role);
    else if (spec.text) {
      const r = rx(spec.text);
      const found = Array.from(document.body.querySelectorAll("*")).filter((el) => !["SCRIPT", "STYLE", "NOSCRIPT"].includes(el.tagName) && r.test(textOf(el)));
      const set = new Set(found);
      list = found.filter((el) => !Array.from(el.children).some((c) => set.has(c)));
    }
    else if (spec.placeholder) { const r = rx(spec.placeholder); list = Array.from(document.querySelectorAll("[placeholder]")).filter((el) => r.test(el.getAttribute("placeholder"))); }
    else if (spec.icon) {
      list = Array.from(document.querySelectorAll(spec.icon.tag || "button"))
        .filter((el) => Array.from(el.querySelectorAll(spec.icon.selector || "i.google-symbols")).some((i) => norm(i.textContent) === spec.icon.name));
    }
    if (spec.role && spec.name) { const r = rx(spec.name); list = list.filter((el) => r.test(nameOf(el))); }
    if (spec.hasText) { const r = rx(spec.hasText); list = list.filter((el) => r.test(textOf(el))); }
    return list;
  };
  const usable = (el, enabled) => visible(el) && !(enabled && (el.getAttribute("aria-disabled") === "true" || el.disabled));
  const pick = (spec, enabled) => { const list = matches(spec); for (let i = list.length - 1; i >= 0; i--) if (usable(list[i], enabled)) return list[i]; return null; };
  return { norm, textOf, visible, nameOf, roleOf, matches, pick, count: (spec) => matches(spec).length, first: (spec) => matches(spec)[0] || null };
})(); }"#;

/// The locator toolkit's script, for a driver that writes its own page script around it (`window.__h3` is defined after it runs).
pub fn toolkit() -> &'static str {
    TOOLKIT
}

/// A regular expression for the page: JavaScript syntax, as Playwright took it.
pub fn re(source: &str, flags: &str) -> Value {
    json!({"src": source, "flags": flags})
}

/// What to look for in a page. See [`TOOLKIT`] for how each kind is matched.
#[derive(Clone, Debug)]
pub struct Loc(pub Value);

impl Loc {
    /// A CSS selector.
    pub fn css(selector: &str) -> Loc {
        Loc(json!({"css": selector}))
    }

    /// A CSS selector whose element text also matches (Playwright's `:has-text`, so give it the `i` flag for the same case rule).
    pub fn css_with_text(selector: &str, text: Value) -> Loc {
        Loc(json!({"css": selector, "hasText": text}))
    }

    /// An element with an ARIA role (explicit, or the one the tag has) and, when given, a matching accessible name.
    pub fn role(role: &str, name: Option<Value>) -> Loc {
        let mut spec = json!({"role": role});
        if let Some(name) = name {
            spec["name"] = name;
        }
        Loc(spec)
    }

    /// The deepest elements whose text matches.
    pub fn text(pattern: Value) -> Loc {
        Loc(json!({"text": pattern}))
    }

    pub fn placeholder(pattern: Value) -> Loc {
        Loc(json!({"placeholder": pattern}))
    }

    /// A `tag` (a button, by default) that holds an icon glyph with this name, such as Google's `<i class="google-symbols">add_2</i>`.
    pub fn icon(tag: &str, name: &str) -> Loc {
        Loc(json!({"icon": {"tag": tag, "name": name}}))
    }
}

/// The browser: one connection, any number of tabs.
#[derive(Clone)]
pub struct Browser {
    pub cdp: Arc<Cdp>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabInfo {
    pub id: String,
    pub url: String,
    pub title: String,
}

impl Browser {
    pub async fn connect(port: u16) -> Result<Browser> {
        Ok(Browser { cdp: Cdp::connect_port(port).await? })
    }

    /// The open tabs (not the extension and service worker targets).
    pub async fn tabs(&self) -> Result<Vec<TabInfo>> {
        let answer = self.cdp.send(None, "Target.getTargets", json!({})).await?;
        Ok(answer["targetInfos"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter(|t| t["type"] == "page")
                    .map(|t| TabInfo { id: t["targetId"].as_str().unwrap_or("").into(), url: t["url"].as_str().unwrap_or("").into(), title: t["title"].as_str().unwrap_or("").into() })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Work in an open tab.
    pub async fn attach(&self, tab: &TabInfo) -> Result<Page> {
        let answer = self.cdp.send(None, "Target.attachToTarget", json!({"targetId": tab.id, "flatten": true})).await?;
        let session = answer["sessionId"].as_str().ok_or_else(|| BrowserError::Chrome("Chrome gave no session for the tab.".into()))?.to_string();
        let page = Page { cdp: Arc::clone(&self.cdp), session, target_id: tab.id.clone() };
        page.cdp.send(Some(&page.session), "Page.enable", json!({})).await?;
        Ok(page)
    }

    /// A new tab on `url`.
    pub async fn new_tab(&self, url: &str) -> Result<Page> {
        let answer = self.cdp.send(None, "Target.createTarget", json!({"url": url})).await?;
        let id = answer["targetId"].as_str().ok_or_else(|| BrowserError::Chrome("Chrome did not open a tab.".into()))?.to_string();
        self.attach(&TabInfo { id, url: url.into(), title: String::new() }).await
    }

    /// Chrome saves downloads into `dir` without asking, and reports them (`Browser.downloadWillBegin` and `downloadProgress`).
    pub async fn allow_downloads(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).map_err(|e| BrowserError::File(format!("Could not create {}: {e}", dir.display())))?;
        self.cdp.send(None, "Browser.setDownloadBehavior", json!({"behavior": "allow", "downloadPath": dir.display().to_string(), "eventsEnabled": true})).await?;
        Ok(())
    }

    /// The tab to work in: a tab already on one of `prefixes` (and not a sign-in page), else the first tab that is not blank, else a new one.
    pub async fn working_tab(&self, prefixes: &[&str], avoid: &str, url: &str) -> Result<Page> {
        let tabs = self.tabs().await?;
        let chosen = tabs
            .iter()
            .find(|t| prefixes.iter().any(|p| t.url.starts_with(p)) && (avoid.is_empty() || !t.url.contains(avoid)))
            .or_else(|| tabs.iter().find(|t| t.url != "about:blank" && !t.url.starts_with("chrome://") && !t.url.starts_with("devtools://")))
            .or_else(|| tabs.first());
        match chosen {
            Some(tab) => self.attach(tab).await,
            None => self.new_tab(url).await,
        }
    }

    /// Fetch a picture's bytes the way the person's Chrome would: a temporary tab that opens the address, so cookies and certificates
    /// apply and a top-level request is not held back by CORS. Returns the content type and the bytes.
    pub async fn fetch_image(&self, url: &str, limit: Duration) -> Result<(String, Vec<u8>)> {
        let tab = self.new_tab("about:blank").await?;
        let result = tab.fetch_main_resource(url, limit).await;
        let _ = tab.close().await;
        result
    }
}

/// One tab, reached through its own session.
#[derive(Clone)]
pub struct Page {
    cdp: Arc<Cdp>,
    session: String,
    target_id: String,
}

/// A thing in a page that was found, kept by Chrome under an object id.
#[derive(Clone)]
pub struct Element {
    page: Page,
    object_id: String,
}

fn script_error(details: &Value) -> BrowserError {
    let text = details["exception"]["description"].as_str().or_else(|| details["text"].as_str()).unwrap_or("script error");
    BrowserError::page(format!("The page script failed: {text}"))
}

impl Page {
    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp.send(Some(&self.session), method, params).await
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Run a JavaScript expression in the page and return its value (a promise is awaited).
    pub async fn eval(&self, expression: &str) -> Result<Value> {
        let answer = self.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "awaitPromise": true})).await?;
        if answer.get("exceptionDetails").is_some() {
            return Err(script_error(&answer["exceptionDetails"]));
        }
        Ok(answer["result"].get("value").cloned().unwrap_or(Value::Null))
    }

    /// Run an expression that gives an element and keep that element. `None` when it gives null.
    pub async fn eval_element(&self, expression: &str) -> Result<Option<Element>> {
        let answer = self.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": false})).await?;
        if answer.get("exceptionDetails").is_some() {
            return Err(script_error(&answer["exceptionDetails"]));
        }
        let result = &answer["result"];
        if result["subtype"] == "null" || result["type"] == "undefined" {
            return Ok(None);
        }
        Ok(result["objectId"].as_str().map(|id| Element { page: self.clone(), object_id: id.to_string() }))
    }

    pub async fn url(&self) -> String {
        self.eval("location.href").await.ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }

    pub async fn bring_to_front(&self) {
        let _ = self.call("Page.bringToFront", json!({})).await;
    }

    pub async fn close(&self) -> Result<()> {
        self.cdp.send(None, "Target.closeTarget", json!({"targetId": self.target_id})).await.map(|_| ())
    }

    /// Open `url` and wait for the page to load (or for `limit`: a slow page is not an error, the automation looks for what it needs).
    pub async fn goto(&self, url: &str, limit: Duration) -> Result<()> {
        let mut events = self.cdp.subscribe();
        let answer = self.call("Page.navigate", json!({"url": url})).await?;
        if let Some(error) = answer["errorText"].as_str() {
            return Err(BrowserError::page(format!("Chrome could not open {url}: {error}")));
        }
        let session = self.session.clone();
        Cdp::wait_event(&mut events, limit, |e| e.session.as_deref() == Some(session.as_str()) && e.method == "Page.loadEventFired").await;
        Ok(())
    }

    /// [`Page::eval_element`] for an expression that uses the locator toolkit (`window.__h3`).
    pub async fn eval_element_with_toolkit(&self, expression: &str) -> Result<Option<Element>> {
        self.eval_element(&format!("(() => {{ {TOOLKIT}\nreturn {expression}; }})()")).await
    }

    /// The first element for the spec that is visible, looking from the end of the page's list (`enabled`: and not disabled).
    pub async fn find(&self, loc: &Loc, enabled: bool) -> Result<Option<Element>> {
        let expression = format!("(() => {{ {TOOLKIT}\nreturn window.__h3.pick({}, {enabled}); }})()", loc.0);
        self.eval_element(&expression).await
    }

    /// The first element for the spec in document order, visible or not (a file box is usually hidden).
    pub async fn find_existing(&self, loc: &Loc) -> Result<Option<Element>> {
        let expression = format!("(() => {{ {TOOLKIT}\nreturn window.__h3.first({}); }})()", loc.0);
        self.eval_element(&expression).await
    }

    /// How many elements match, visible or not.
    pub async fn count(&self, loc: &Loc) -> Result<usize> {
        let expression = format!("(() => {{ {TOOLKIT}\nreturn window.__h3.count({}); }})()", loc.0);
        Ok(self.eval(&expression).await?.as_u64().unwrap_or(0) as usize)
    }

    /// The first visible element among the locators, in their order.
    pub async fn find_first(&self, locs: &[Loc], enabled: bool) -> Result<Option<Element>> {
        for loc in locs {
            if let Some(element) = self.find(loc, enabled).await? {
                return Ok(Some(element));
            }
        }
        Ok(None)
    }

    /// Wait until one of the locators has a visible element. `None` after `limit`.
    pub async fn wait_for_any(&self, locs: &[Loc], limit: Duration) -> Result<Option<Element>> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if let Some(element) = self.find_first(locs, false).await? {
                return Ok(Some(element));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            sleep(500).await;
        }
    }

    /// Click the first visible, enabled element among the locators. False when there is none.
    pub async fn click_first(&self, locs: &[Loc]) -> Result<bool> {
        match self.find_first(locs, true).await? {
            Some(element) => {
                element.click().await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// [`Page::click_first`] again and again until it clicks something or `limit` passes.
    pub async fn wait_and_click_first(&self, locs: &[Loc], limit: Duration) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if self.click_first(locs).await? {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(500).await;
        }
    }

    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<()> {
        self.call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x, "y": y})).await.map(|_| ())
    }

    pub async fn mouse_click(&self, x: f64, y: f64, button: &str) -> Result<()> {
        self.mouse_move(x, y).await?;
        let pressed = if button == "right" { 2 } else { 1 };
        self.call("Input.dispatchMouseEvent", json!({"type": "mousePressed", "x": x, "y": y, "button": button, "buttons": pressed, "clickCount": 1})).await?;
        self.call("Input.dispatchMouseEvent", json!({"type": "mouseReleased", "x": x, "y": y, "button": button, "buttons": 0, "clickCount": 1})).await?;
        Ok(())
    }

    /// One key press. `modifiers` are DevTools' bit flags: 1 Alt, 2 Ctrl, 4 Meta, 8 Shift. `commands` are editing commands such as
    /// `selectAll` and `paste`, which is how Chrome's own shortcuts work in a page.
    pub async fn press_key(&self, key: &str, modifiers: u32, commands: &[&str]) -> Result<()> {
        let (code, vk, text) = match key {
            "Enter" => ("Enter", 13, "\r"),
            "Backspace" => ("Backspace", 8, ""),
            "Escape" => ("Escape", 27, ""),
            "Tab" => ("Tab", 9, ""),
            "a" => ("KeyA", 65, ""),
            "v" => ("KeyV", 86, ""),
            other => return Err(BrowserError::page(format!("The key {other} is not supported."))),
        };
        let base = json!({"key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": modifiers});
        let mut down = base.clone();
        down["type"] = json!(if text.is_empty() { "rawKeyDown" } else { "keyDown" });
        if !text.is_empty() {
            down["text"] = json!(text);
        }
        if !commands.is_empty() {
            down["commands"] = json!(commands);
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        let mut up = base;
        up["type"] = json!("keyUp");
        self.call("Input.dispatchKeyEvent", up).await.map(|_| ())
    }

    /// The Ctrl (Windows, Linux) or Meta (Mac) flag for shortcuts.
    pub fn shortcut_modifier() -> u32 {
        if cfg!(target_os = "macos") {
            4
        } else {
            2
        }
    }

    pub async fn select_all(&self) -> Result<()> {
        self.press_key("a", Self::shortcut_modifier(), &["selectAll"]).await
    }

    /// Put text where the cursor is, the way a paste or an input method does (no key events per letter).
    pub async fn insert_text(&self, text: &str) -> Result<()> {
        self.call("Input.insertText", json!({"text": text})).await.map(|_| ())
    }

    /// Run `action` (it should click what opens a file dialog) and fill the dialog with `files`.
    pub async fn choose_files<F, Fut>(&self, files: &[PathBuf], limit: Duration, action: F) -> Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        self.call("Page.setInterceptFileChooserDialog", json!({"enabled": true})).await?;
        let mut events = self.cdp.subscribe();
        let outcome = async {
            action().await?;
            let session = self.session.clone();
            let opened = Cdp::wait_event(&mut events, limit, |e| e.session.as_deref() == Some(session.as_str()) && e.method == "Page.fileChooserOpened")
                .await
                .ok_or_else(|| BrowserError::timeout("The page did not open a file dialog."))?;
            let node = opened.params["backendNodeId"].as_i64().ok_or_else(|| BrowserError::page("Chrome did not say which file box opened."))?;
            let resolved = self.cdp.send(Some(&self.session), "DOM.resolveNode", json!({"backendNodeId": node})).await?;
            let object_id = resolved["object"]["objectId"].as_str().ok_or_else(|| BrowserError::page("Chrome could not reach the file box."))?;
            let paths: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
            self.cdp.send(Some(&self.session), "DOM.setFileInputFiles", json!({"files": paths, "objectId": object_id})).await?;
            Ok(())
        }
        .await;
        let _ = self.call("Page.setInterceptFileChooserDialog", json!({"enabled": false})).await;
        outcome
    }

    /// Run `action` (it should start a download) and wait until Chrome has saved the file. Returns the saved file's path.
    /// [`Browser::allow_downloads`] must have been called with `dir`.
    pub async fn wait_download<F, Fut>(&self, dir: &Path, limit: Duration, action: F) -> Result<PathBuf>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let mut events = self.cdp.subscribe();
        action().await?;
        let begun = Cdp::wait_event(&mut events, limit, |e| e.method == "Browser.downloadWillBegin").await.ok_or_else(|| BrowserError::timeout("Chrome did not start a download."))?;
        let guid = begun.params["guid"].as_str().unwrap_or("").to_string();
        let suggested = begun.params["suggestedFilename"].as_str().unwrap_or("").to_string();
        let finished = Cdp::wait_event(&mut events, limit, |e| e.method == "Browser.downloadProgress" && e.params["guid"] == guid.as_str() && matches!(e.params["state"].as_str(), Some("completed" | "canceled")))
            .await
            .ok_or_else(|| BrowserError::timeout("Chrome did not finish the download in time."))?;
        if finished.params["state"] == "canceled" {
            return Err(BrowserError::page("Chrome canceled the download."));
        }
        newest_download(dir, &suggested).ok_or_else(|| BrowserError::File(format!("Chrome said the download finished, but no file was found in {}.", dir.display())))
    }

    /// Open `url` in this tab and read the response of that request: the content type and the bytes.
    async fn fetch_main_resource(&self, url: &str, limit: Duration) -> Result<(String, Vec<u8>)> {
        self.call("Network.enable", json!({})).await?;
        let mut events = self.cdp.subscribe();
        let answer = self.call("Page.navigate", json!({"url": url})).await?;
        if let Some(error) = answer["errorText"].as_str() {
            return Err(BrowserError::page(format!("Chrome could not open the picture: {error}")));
        }
        let session = self.session.clone();
        let response = Cdp::wait_event(&mut events, limit, |e| {
            e.session.as_deref() == Some(session.as_str()) && e.method == "Network.responseReceived" && e.params["type"] == "Document"
        })
        .await
        .ok_or_else(|| BrowserError::timeout("The picture did not load in time."))?;
        let status = response.params["response"]["status"].as_i64().unwrap_or(0);
        let content_type = response.params["response"]["mimeType"].as_str().unwrap_or("").to_string();
        if !(200..300).contains(&status) || !content_type.to_lowercase().starts_with("image/") {
            return Err(BrowserError::page(format!("Direct image save failed: {status} {content_type}")));
        }
        let request_id = response.params["requestId"].as_str().unwrap_or("").to_string();
        Cdp::wait_event(&mut events, limit, |e| e.method == "Network.loadingFinished" && e.params["requestId"] == request_id.as_str())
            .await
            .ok_or_else(|| BrowserError::timeout("The picture did not finish loading in time."))?;
        let body = self.call("Network.getResponseBody", json!({"requestId": request_id})).await?;
        let text = body["body"].as_str().unwrap_or("");
        let bytes = if body["base64Encoded"] == true {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(text).map_err(|e| BrowserError::page(format!("The picture came back unreadable: {e}")))?
        } else {
            text.as_bytes().to_vec()
        };
        if bytes.is_empty() {
            return Err(BrowserError::page("Direct image save failed: no data"));
        }
        Ok((content_type, bytes))
    }
}

/// The file a download produced: the one named like the suggested name (Chrome adds ` (1)` when it exists), else the newest picture.
fn newest_download(dir: &Path, suggested: &str) -> Option<PathBuf> {
    let wanted = Path::new(suggested).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut best: Option<(std::time::SystemTime, bool, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        if !path.is_file() || name.ends_with(".crdownload") || name.ends_with(".tmp") {
            continue;
        }
        let picture = [".png", ".jpg", ".jpeg", ".webp"].iter().any(|ext| name.ends_with(ext));
        if !picture {
            continue;
        }
        let time = entry.metadata().ok()?.modified().ok()?;
        let named = !wanted.is_empty() && name.starts_with(&wanted);
        // A file named like the download beats a newer one that is not; among equals the newest wins.
        if best.as_ref().is_none_or(|(t, n, _)| (named, time) > (*n, *t)) {
            best = Some((time, named, path));
        }
    }
    best.map(|(_, _, path)| path)
}

impl Element {
    /// Call a function with this element as `this`. The arguments are JSON values.
    pub async fn call(&self, function: &str, args: &[Value]) -> Result<Value> {
        let arguments: Vec<Value> = args.iter().map(|v| json!({"value": v})).collect();
        let answer = self
            .page
            .call("Runtime.callFunctionOn", json!({"objectId": self.object_id, "functionDeclaration": function, "arguments": arguments, "returnByValue": true, "awaitPromise": true}))
            .await?;
        if answer.get("exceptionDetails").is_some() {
            return Err(script_error(&answer["exceptionDetails"]));
        }
        Ok(answer["result"].get("value").cloned().unwrap_or(Value::Null))
    }

    /// Where the middle of the element is in the tab, after scrolling it into view.
    pub async fn center(&self) -> Result<(f64, f64)> {
        let rect = self
            .call("function() { if (this.scrollIntoViewIfNeeded) this.scrollIntoViewIfNeeded(true); const r = this.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2, r.width, r.height]; }", &[])
            .await?;
        let number = |i: usize| rect[i].as_f64().unwrap_or(0.0);
        if number(2) <= 0.0 || number(3) <= 0.0 {
            return Err(BrowserError::page("The element to click has no size on the page."));
        }
        Ok((number(0), number(1)))
    }

    pub async fn click(&self) -> Result<()> {
        let (x, y) = self.center().await?;
        self.page.mouse_click(x, y, "left").await
    }

    /// The element's box in the tab: x, y, width and height.
    pub async fn rect(&self) -> Result<(f64, f64, f64, f64)> {
        let rect = self.call("function() { const r = this.getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; }", &[]).await?;
        let number = |i: usize| rect[i].as_f64().unwrap_or(0.0);
        Ok((number(0), number(1), number(2), number(3)))
    }

    /// Put files in a file box (`<input type="file">`), as choosing them in its dialog would.
    pub async fn set_files(&self, files: &[PathBuf]) -> Result<()> {
        let paths: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
        self.page.call("DOM.setFileInputFiles", json!({"files": paths, "objectId": self.object_id})).await.map(|_| ())
    }

    pub async fn right_click(&self) -> Result<()> {
        let (x, y) = self.center().await?;
        self.page.mouse_click(x, y, "right").await
    }

    pub async fn hover(&self) -> Result<()> {
        let (x, y) = self.center().await?;
        self.page.mouse_move(x, y).await
    }

    pub async fn focus(&self) -> Result<()> {
        self.call("function() { this.focus(); }", &[]).await.map(|_| ())
    }

    pub async fn is_visible(&self) -> bool {
        self.call("function() { const r = this.getBoundingClientRect(); const s = getComputedStyle(this); return this.isConnected && r.width > 0 && r.height > 0 && s.display !== 'none' && s.visibility !== 'hidden'; }", &[])
            .await
            .map(|v| v == true)
            .unwrap_or(false)
    }

    pub async fn is_enabled(&self) -> bool {
        self.call("function() { return !this.disabled && this.getAttribute('aria-disabled') !== 'true'; }", &[]).await.map(|v| v == true).unwrap_or(false)
    }

    /// The text the person would read in it: a box's value, else its visible text.
    pub async fn text(&self) -> String {
        self.call("function() { return String(this.value || this.innerText || this.textContent || '').replace(/[\\r\\n\\t]+/g, ' ').replace(/\\s{2,}/g, ' ').trim(); }", &[])
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }
}

pub async fn sleep(milliseconds: u64) {
    tokio::time::sleep(Duration::from_millis(milliseconds)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locators_are_plain_specs_the_page_toolkit_reads() {
        assert_eq!(Loc::css("textarea").0, json!({"css": "textarea"}));
        assert_eq!(Loc::role("button", Some(re("^start generation$", "i"))).0, json!({"role": "button", "name": {"src": "^start generation$", "flags": "i"}}));
        assert_eq!(Loc::role("option", None).0, json!({"role": "option"}));
        assert_eq!(Loc::css_with_text("button", re("New project", "i")).0, json!({"css": "button", "hasText": {"src": "New project", "flags": "i"}}));
        assert_eq!(Loc::icon("button", "add_2").0, json!({"icon": {"tag": "button", "name": "add_2"}}));
        assert_eq!(Loc::text(re("upload media", "i")).0["text"]["src"], "upload media");
        assert_eq!(Loc::placeholder(re("what do you want to create", "i")).0["placeholder"]["flags"], "i");
    }

    #[test]
    fn the_toolkit_is_valid_script_text_that_defines_the_finders_once() {
        // The page side is JavaScript in a string: at least keep its pieces in place and its braces balanced.
        for piece in ["window.__h3 = ", "const pick = ", "const matches = ", "count: (spec)", "roleSelector"] {
            assert!(TOOLKIT.contains(piece), "{piece}");
        }
        for (open, close) in [('{', '}'), ('(', ')'), ('[', ']')] {
            assert_eq!(TOOLKIT.matches(open).count(), TOOLKIT.matches(close).count(), "{open}{close}");
        }
    }

    #[test]
    fn a_finished_download_is_the_file_named_like_the_suggestion_else_the_newest_picture() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, age: u64| {
            let path = dir.path().join(name);
            std::fs::write(&path, b"x").unwrap();
            let time = std::time::SystemTime::now() - Duration::from_secs(age);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
            path
        };
        let old = write("flow-image.png", 100);
        let newer_other = write("other.png", 10);
        write("notes.txt", 1);
        write("partial.png.crdownload", 1);
        // Named like the suggestion wins over a newer picture of another name, and Chrome's " (1)" suffix still counts as that name.
        assert_eq!(newest_download(dir.path(), "flow-image.png"), Some(old.clone()));
        let renamed = write("flow-image (1).png", 50);
        assert_eq!(newest_download(dir.path(), "flow-image.png"), Some(renamed));
        // No suggestion: the newest picture.
        assert_eq!(newest_download(dir.path(), ""), Some(newer_other));
        assert_eq!(newest_download(Path::new("no such folder"), "x.png"), None);
    }
}
