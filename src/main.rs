//! AlertWim — a tiny local log window for BoardDo scenarios.
//!
//! On start it listens on `127.0.0.1:47821` (`ALERTWIM_PORT`), protected by a
//! token kept in `~/.alertwim/config.json`, and writes
//! `~/.alertwim/endpoint.json` so BoardDo can find it and create its
//! connection automatically. A second launch only brings the running window up.
//!
//! HTTP API (JSON):
//!   GET  /health                      — no auth: `{app, version, pid}`
//!   POST /v1/alerts   (Bearer token)  — `{message, level?, title?, source?, data?, show?, focus?}`
//!   GET  /v1/alerts   (Bearer token)  — recent alerts
//!   POST /v1/show     (Bearer token)  — bring the window to front
//!   POST /v1/clear    (Bearer token)  — clear the log

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

const APP: &str = "alertwim";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PORT: u16 = 47821;
const MAX_ALERTS: usize = 2000;
const MAX_BODY: usize = 1 << 20;

#[derive(Clone, Serialize)]
struct Alert {
    id: String,
    ts: u64,
    level: String,
    title: Option<String>,
    message: String,
    source: Option<String>,
    data: Option<Value>,
}

#[derive(Deserialize)]
struct AlertIn {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    data: Option<Value>,
    /// Show the window if it is hidden or minimized (default true).
    #[serde(default)]
    show: Option<bool>,
    /// Also steal focus (default false).
    #[serde(default)]
    focus: Option<bool>,
}

#[derive(Default)]
struct Shared {
    alerts: VecDeque<Alert>,
    /// Last time a client identified as BoardDo talked to us (ms).
    boarddo_seen: Option<u64>,
}

#[derive(Clone)]
struct AppState {
    shared: Arc<Mutex<Shared>>,
    url: String,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct Config {
    token: String,
    #[serde(default)]
    port: Option<u16>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn home_dir() -> PathBuf {
    std::env::var_os("ALERTWIM_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".alertwim")))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".alertwim")))
        .unwrap_or_else(|| PathBuf::from(".alertwim"))
}

fn load_config() -> Config {
    let dir = home_dir();
    let path = dir.join("config.json");
    if let Some(cfg) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Config>(&b).ok()) {
        if !cfg.token.is_empty() {
            return cfg;
        }
    }
    let cfg = Config { token: uuid::Uuid::new_v4().simple().to_string(), port: None };
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&cfg).unwrap_or_default());
    restrict(&path);
    cfg
}

#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict(_: &std::path::Path) {}

fn endpoint_path() -> PathBuf {
    home_dir().join("endpoint.json")
}

fn write_endpoint(url: &str, token: &str) {
    let path = endpoint_path();
    let body = json!({
        "app": APP,
        "version": VERSION,
        "url": url,
        "token": token,
        "pid": std::process::id(),
        "started_at": now_ms(),
        "exe": std::env::current_exe().ok(),
    });
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap_or_default());
    restrict(&path);
}

fn remove_endpoint() {
    let path = endpoint_path();
    let ours = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("pid").and_then(Value::as_u64))
        == Some(std::process::id() as u64);
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

/// Minimal blocking HTTP call to an already running instance.
fn call_running(port: u16, method: &str, path: &str, token: &str) -> Option<String> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

fn show_window(app: &AppHandle, focus: bool) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        if focus {
            let _ = w.set_focus();
        }
    }
}

fn respond(req: tiny_http::Request, code: u16, body: Value) {
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let resp = tiny_http::Response::from_string(body.to_string()).with_status_code(code).with_header(header);
    let _ = req.respond(resp);
}

fn authorized(req: &tiny_http::Request, token: &str) -> bool {
    req.headers().iter().any(|h| {
        h.field.equiv("Authorization") && h.value.as_str().trim() == format!("Bearer {token}")
    })
}

fn from_boarddo(req: &tiny_http::Request) -> bool {
    req.headers().iter().any(|h| {
        (h.field.equiv("X-AlertWim-Client") || h.field.equiv("User-Agent"))
            && h.value.as_str().to_ascii_lowercase().contains("boarddo")
    })
}

fn normalize_level(level: Option<String>) -> String {
    match level.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "warn" | "warning" => "warn",
        "error" | "err" | "fail" | "failed" | "critical" => "error",
        "success" | "ok" | "done" => "success",
        "debug" | "trace" => "debug",
        _ => "info",
    }
    .to_string()
}

fn serve(server: tiny_http::Server, app: AppHandle, state: AppState) {
    for mut req in server.incoming_requests() {
        let path = req.url().split('?').next().unwrap_or("").to_string();
        let method = req.method().as_str().to_ascii_uppercase();

        if method == "GET" && path == "/health" {
            respond(req, 200, json!({ "app": APP, "version": VERSION, "pid": std::process::id() }));
            continue;
        }
        if !authorized(&req, &state.token) {
            respond(req, 401, json!({ "error": "unauthorized" }));
            continue;
        }
        if from_boarddo(&req) {
            let seen = now_ms();
            state.shared.lock().unwrap().boarddo_seen = Some(seen);
            let _ = app.emit("boarddo-seen", seen);
        }

        match (method.as_str(), path.as_str()) {
            ("POST", "/v1/alerts") => {
                let mut raw = Vec::new();
                let _ = req.as_reader().take(MAX_BODY as u64).read_to_end(&mut raw);
                let input: AlertIn = match serde_json::from_slice(&raw) {
                    Ok(v) => v,
                    Err(e) => {
                        respond(req, 400, json!({ "error": format!("invalid JSON: {e}") }));
                        continue;
                    }
                };
                let message = input.message.unwrap_or_default();
                if message.trim().is_empty() && input.title.is_none() {
                    respond(req, 400, json!({ "error": "`message` is required" }));
                    continue;
                }
                let alert = Alert {
                    id: uuid::Uuid::new_v4().simple().to_string(),
                    ts: now_ms(),
                    level: normalize_level(input.level),
                    title: input.title.filter(|t| !t.trim().is_empty()),
                    message,
                    source: input.source.filter(|s| !s.trim().is_empty()),
                    data: input.data.filter(|d| !d.is_null()),
                };
                {
                    let mut sh = state.shared.lock().unwrap();
                    sh.alerts.push_back(alert.clone());
                    while sh.alerts.len() > MAX_ALERTS {
                        sh.alerts.pop_front();
                    }
                }
                let _ = app.emit("alert", &alert);
                if input.show.unwrap_or(true) {
                    show_window(&app, input.focus.unwrap_or(false));
                }
                respond(req, 200, json!({ "ok": true, "id": alert.id, "ts": alert.ts }));
            }
            ("GET", "/v1/alerts") => {
                let list: Vec<Alert> = state.shared.lock().unwrap().alerts.iter().cloned().collect();
                respond(req, 200, json!({ "alerts": list }));
            }
            ("POST", "/v1/show") => {
                show_window(&app, true);
                respond(req, 200, json!({ "ok": true }));
            }
            ("POST", "/v1/clear") => {
                state.shared.lock().unwrap().alerts.clear();
                let _ = app.emit("cleared", ());
                respond(req, 200, json!({ "ok": true }));
            }
            _ => respond(req, 404, json!({ "error": "not found" })),
        }
    }
}

#[tauri::command]
fn get_alerts(state: tauri::State<AppState>) -> Vec<Alert> {
    state.shared.lock().unwrap().alerts.iter().cloned().collect()
}

#[tauri::command]
fn clear_alerts(state: tauri::State<AppState>) {
    state.shared.lock().unwrap().alerts.clear();
}

#[tauri::command]
fn get_info(state: tauri::State<AppState>) -> Value {
    json!({
        "version": VERSION,
        "url": state.url,
        "endpoint_file": endpoint_path(),
        "boarddo_seen": state.shared.lock().unwrap().boarddo_seen,
    })
}

fn main() {
    let cfg = load_config();
    let port = std::env::var("ALERTWIM_PORT").ok().and_then(|p| p.parse().ok()).or(cfg.port).unwrap_or(DEFAULT_PORT);

    // Already running → bring that window up and quit.
    if let Some(resp) = call_running(port, "GET", "/health", &cfg.token) {
        if resp.contains("\"alertwim\"") {
            let _ = call_running(port, "POST", "/v1/show", &cfg.token);
            return;
        }
    }

    // Preferred port, or any free one if it is taken by something else.
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .or_else(|_| tiny_http::Server::http(("127.0.0.1", 0)))
        .expect("AlertWim: cannot bind a loopback port");
    let bound = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);
    let url = format!("http://127.0.0.1:{bound}");

    let state = AppState { shared: Arc::new(Mutex::new(Shared::default())), url: url.clone(), token: cfg.token.clone() };
    write_endpoint(&url, &cfg.token);

    let welcome = Alert {
        id: uuid::Uuid::new_v4().simple().to_string(),
        ts: now_ms(),
        level: "success".into(),
        title: Some("AlertWim запущен".into()),
        message: format!("Слушаю {url} — BoardDo подключится автоматически."),
        source: Some("alertwim".into()),
        data: None,
    };
    state.shared.lock().unwrap().alerts.push_back(welcome);

    let srv_state = state.clone();
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![get_alerts, clear_alerts, get_info])
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || serve(server, handle, srv_state));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("AlertWim: failed to start")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                remove_endpoint();
            }
        });
}
