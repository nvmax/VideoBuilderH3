//! The page layer against a scripted stand-in for Chrome: the sequences of DevTools calls and events for opening a tab, choosing a file
//! in a file dialog, waiting for a download and reading a picture's bytes. The stand-in follows the DevTools Protocol as documented, so
//! these tests show the code asks in the right order and reads the answers right, not that a real Chrome agrees.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use h3_browser::page::Browser;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A stand-in Chrome on a debug port. Returns the port and the list of calls it received.
async fn fake_chrome(download_dir: std::path::PathBuf) -> (u16, Calls) {
    let calls: Calls = Arc::default();
    let sockets = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}/devtools/browser/fake", sockets.local_addr().unwrap());
    let app = axum::Router::new().route("/json/version", get(move || {
        let ws_url = ws_url.clone();
        async move { axum::Json(json!({"Browser": "Chrome/fake", "webSocketDebuggerUrl": ws_url})) }
    }));
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = http.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(http, app).await.unwrap() });

    let seen = Arc::clone(&calls);
    tokio::spawn(async move {
        let (stream, _) = sockets.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let call: Value = serde_json::from_str(text.as_str()).unwrap();
            let method = call["method"].as_str().unwrap().to_string();
            let params = call["params"].clone();
            seen.lock().unwrap().push((method.clone(), params.clone()));
            let session = call["sessionId"].clone();
            let mut events: Vec<Value> = Vec::new();
            let result = match method.as_str() {
                "Target.createTarget" => json!({"targetId": "T1"}),
                "Target.attachToTarget" => json!({"sessionId": "S1"}),
                "Page.navigate" => {
                    events.push(json!({"method": "Network.responseReceived", "sessionId": "S1", "params": {"requestId": "R1", "type": "Document", "response": {"status": 200, "mimeType": "image/png"}}}));
                    events.push(json!({"method": "Network.loadingFinished", "sessionId": "S1", "params": {"requestId": "R1"}}));
                    json!({"frameId": "F1"})
                }
                "Network.getResponseBody" => json!({"body": "UE5HREFUQQ==", "base64Encoded": true}),
                "DOM.resolveNode" => json!({"object": {"objectId": "O1"}}),
                "Runtime.evaluate" => {
                    match params["expression"].as_str().unwrap() {
                        "OPEN_DIALOG" => events.push(json!({"method": "Page.fileChooserOpened", "sessionId": "S1", "params": {"frameId": "F1", "mode": "selectSingle", "backendNodeId": 77}})),
                        "START_DOWNLOAD" => {
                            std::fs::write(download_dir.join("flow-image.png"), b"png").unwrap();
                            events.push(json!({"method": "Browser.downloadWillBegin", "params": {"guid": "G1", "url": "https://x/y", "suggestedFilename": "flow-image.png"}}));
                            events.push(json!({"method": "Browser.downloadProgress", "params": {"guid": "G1", "state": "completed"}}));
                        }
                        _ => {}
                    }
                    json!({"result": {"type": "boolean", "value": true}})
                }
                _ => json!({}),
            };
            socket.send(Message::text(json!({"id": call["id"], "result": result, "sessionId": session}).to_string())).await.unwrap();
            for event in events {
                socket.send(Message::text(event.to_string())).await.unwrap();
            }
        }
    });
    (port, calls)
}

fn names(calls: &Calls) -> Vec<String> {
    calls.lock().unwrap().iter().map(|(m, _)| m.clone()).collect()
}

#[tokio::test]
async fn a_new_tab_is_attached_with_a_flat_session_and_gets_page_events() {
    let dir = tempfile::tempdir().unwrap();
    let (port, calls) = fake_chrome(dir.path().to_path_buf()).await;
    let browser = Browser::connect(port).await.unwrap();
    let page = browser.new_tab("about:blank").await.unwrap();
    assert_eq!(page.target_id(), "T1");
    assert_eq!(names(&calls), ["Target.createTarget", "Target.attachToTarget", "Page.enable"]);
    let attach = calls.lock().unwrap()[1].1.clone();
    assert_eq!(attach, json!({"targetId": "T1", "flatten": true}));
}

#[tokio::test]
async fn a_file_dialog_is_filled_with_the_chosen_files() {
    let dir = tempfile::tempdir().unwrap();
    let (port, calls) = fake_chrome(dir.path().to_path_buf()).await;
    let browser = Browser::connect(port).await.unwrap();
    let page = browser.new_tab("about:blank").await.unwrap();
    let file = dir.path().join("frame.png");
    page.choose_files(std::slice::from_ref(&file), Duration::from_secs(3), || async { page.eval("OPEN_DIALOG").await.map(|_| ()) }).await.unwrap();
    let log = calls.lock().unwrap().clone();
    let set = log.iter().find(|(m, _)| m == "DOM.setFileInputFiles").expect("the files were set");
    assert_eq!(set.1, json!({"files": [file.display().to_string()], "objectId": "O1"}));
    let resolve = log.iter().find(|(m, _)| m == "DOM.resolveNode").unwrap();
    assert_eq!(resolve.1, json!({"backendNodeId": 77}));
    // Interception is switched on before the click and off again after.
    let toggles: Vec<Value> = log.iter().filter(|(m, _)| m == "Page.setInterceptFileChooserDialog").map(|(_, p)| p["enabled"].clone()).collect();
    assert_eq!(toggles, [json!(true), json!(false)]);
}

#[tokio::test]
async fn a_dialog_that_never_opens_is_an_error_that_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _) = fake_chrome(dir.path().to_path_buf()).await;
    let page = Browser::connect(port).await.unwrap().new_tab("about:blank").await.unwrap();
    let error = page.choose_files(&[], Duration::from_millis(200), || async { Ok(()) }).await.unwrap_err();
    assert_eq!(error.to_string(), "The page did not open a file dialog.");
}

#[tokio::test]
async fn a_download_is_waited_for_until_chrome_says_it_is_complete() {
    let dir = tempfile::tempdir().unwrap();
    let (port, calls) = fake_chrome(dir.path().to_path_buf()).await;
    let browser = Browser::connect(port).await.unwrap();
    browser.allow_downloads(dir.path()).await.unwrap();
    let page = browser.new_tab("about:blank").await.unwrap();
    let saved = page.wait_download(dir.path(), Duration::from_secs(3), || async { page.eval("START_DOWNLOAD").await.map(|_| ()) }).await.unwrap();
    assert_eq!(saved, dir.path().join("flow-image.png"));
    let behavior = calls.lock().unwrap().iter().find(|(m, _)| m == "Browser.setDownloadBehavior").unwrap().1.clone();
    assert_eq!((behavior["behavior"].clone(), behavior["eventsEnabled"].clone()), (json!("allow"), json!(true)));
}

#[tokio::test]
async fn a_picture_is_read_the_way_the_browser_loaded_it_and_the_temporary_tab_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (port, calls) = fake_chrome(dir.path().to_path_buf()).await;
    let browser = Browser::connect(port).await.unwrap();
    let (content_type, bytes) = browser.fetch_image("https://flow-content.google/image/abc", Duration::from_secs(3)).await.unwrap();
    assert_eq!((content_type.as_str(), bytes.as_slice()), ("image/png", b"PNGDATA".as_slice()));
    let log = names(&calls);
    for (earlier, later) in [("Network.enable", "Page.navigate"), ("Page.navigate", "Network.getResponseBody"), ("Network.getResponseBody", "Target.closeTarget")] {
        let a = log.iter().position(|m| m == earlier).unwrap();
        let b = log.iter().position(|m| m == later).unwrap();
        assert!(a < b, "{earlier} before {later}: {log:?}");
    }
    let navigate = calls.lock().unwrap().iter().find(|(m, _)| m == "Page.navigate").unwrap().1.clone();
    assert_eq!(navigate["url"], "https://flow-content.google/image/abc");
}
