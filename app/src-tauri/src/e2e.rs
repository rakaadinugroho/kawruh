//! Debug-only automation hook for end-to-end tests on macOS (tauri-driver
//! does not support macOS). With `KAWRUH_E2E_PORT` set, a localhost HTTP
//! endpoint accepts `POST /eval` with a JS body; the script runs in the main
//! webview (async allowed, `return` a JSON-serializable value) and the
//! response is `{ ok, value }`. `POST /snapshot` with an output path writes a
//! TIFF of the webview (WKWebView.takeSnapshot; no screen-recording permission
//! needed). Compiled out of release builds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

type Pending = Mutex<HashMap<u64, oneshot::Sender<(bool, String)>>>;

fn pending() -> &'static Pending {
    static P: OnceLock<Pending> = OnceLock::new();
    P.get_or_init(Default::default)
}

#[tauri::command]
pub fn e2e_result(id: u64, ok: bool, value: String) {
    if let Some(tx) = pending().lock().unwrap().remove(&id) {
        let _ = tx.send((ok, value));
    }
}

pub fn start(app: AppHandle) {
    let Some(port) = std::env::var("KAWRUH_E2E_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let listener = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("e2e: cannot bind {port}: {e}");
                return;
            }
        };
        tracing::info!("e2e automation listening on 127.0.0.1:{port}");
        let next = AtomicU64::new(1);
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                continue;
            };
            let id = next.fetch_add(1, Ordering::Relaxed);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let body = match read_body(&mut sock).await {
                    Some(b) => b,
                    None => return,
                };
                let (ok, value) = if body.starts_with("SNAPSHOT ") {
                    snapshot(&app, body.trim_start_matches("SNAPSHOT ").trim()).await
                } else {
                    eval(&app, id, &body).await
                };
                let json = serde_json::json!({ "ok": ok, "value": value }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
}

async fn read_body(sock: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(head_end) = text.find("\r\n\r\n") {
            let len = text[..head_end]
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + len {
                return Some(
                    String::from_utf8_lossy(&buf[head_end + 4..head_end + 4 + len]).to_string(),
                );
            }
        }
    }
}

async fn eval(app: &AppHandle, id: u64, js: &str) -> (bool, String) {
    let Some(win) = app.get_webview_window("main") else {
        return (false, "no main window".into());
    };
    let (tx, rx) = oneshot::channel();
    pending().lock().unwrap().insert(id, tx);
    let script = format!(
        r#"(async () => {{
  const send = (ok, value) => window.__TAURI_INTERNALS__.invoke('e2e_result', {{ id: {id}, ok, value }});
  try {{
    const v = await (async () => {{ {js} }})();
    send(true, JSON.stringify(v === undefined ? null : v));
  }} catch (e) {{
    send(false, String(e && e.stack || e));
  }}
}})();"#
    );
    if let Err(e) = win.eval(&script) {
        pending().lock().unwrap().remove(&id);
        return (false, e.to_string());
    }
    match tokio::time::timeout(Duration::from_secs(60), rx).await {
        Ok(Ok(r)) => r,
        _ => {
            pending().lock().unwrap().remove(&id);
            (false, "timeout".into())
        }
    }
}

#[cfg(target_os = "macos")]
async fn snapshot(app: &AppHandle, path: &str) -> (bool, String) {
    use block2::RcBlock;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSError;
    use objc2_web_kit::WKWebView;

    let Some(win) = app.get_webview_window("main") else {
        return (false, "no main window".into());
    };
    let (tx, rx) = oneshot::channel::<Option<Vec<u8>>>();
    let tx = Mutex::new(Some(tx));
    let res = win.with_webview(move |wv| unsafe {
        let wk = &*(wv.inner() as *const WKWebView);
        let block = RcBlock::new(move |img: *mut NSImage, _err: *mut NSError| {
            let Some(tx) = tx.lock().unwrap().take() else {
                return;
            };
            let bytes = img
                .as_ref()
                .and_then(|img| img.TIFFRepresentation())
                .map(|d| d.to_vec());
            let _ = tx.send(bytes);
        });
        wk.takeSnapshotWithConfiguration_completionHandler(None, &block);
    });
    if let Err(e) = res {
        return (false, e.to_string());
    }
    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(Some(bytes))) => match std::fs::write(path, bytes) {
            Ok(()) => (true, path.to_string()),
            Err(e) => (false, e.to_string()),
        },
        _ => (false, "snapshot failed".into()),
    }
}

#[cfg(not(target_os = "macos"))]
async fn snapshot(_app: &AppHandle, _path: &str) -> (bool, String) {
    (false, "snapshots are only implemented on macOS".into())
}
