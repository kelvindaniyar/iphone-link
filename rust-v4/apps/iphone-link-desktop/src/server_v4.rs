//! In-Process Native Rust HTTP Server and Live Streaming Bridge (v4.0.0).
//!
//! Provides:
//! - Web assets serving (HTML, CSS, JS, SVG, ICO)
//! - v4 True Acrylic CSS override and pywebview polyfill bridge
//! - REST API endpoints: /api/status, /api/settings, /api/device/*, /api/window/*, /api/social/*
//! - Native Win32 window synchronization for resize, drawer, orientation, and theme

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::i18n::{translate_dynamic_status, Lang};
use crate::link_grabber::extract_and_normalize_links;
use crate::ocr_engine::{clear_copy_cache, read_win32_clipboard, write_win32_clipboard};
use crate::window_dwm::{apply_true_files_acrylic_backdrop, WindowGeometryManager, RECT};

use iphone_link_apple::hid::IndigoHidController;
use iphone_link_apple::wifi_discovery::REMOTEPAIRING_SERVICE;

pub const DEFAULT_PORT: u16 = 8765;

// ============================================================================
// EMBEDDED FALLBACKS FOR TRUE ACRYLIC AND NATIVE BRIDGE
// ============================================================================

pub const V4_TRUE_ACRYLIC_CSS: &str = r#"/* v4_true_acrylic.css - Windows 11 True DWM Acrylic Blur Engine */
.app::before {
    display: none !important;
    background-image: none !important;
    opacity: 0 !important;
}

html, body {
    background: transparent !important;
    background-color: transparent !important;
}

.app {
    background: rgba(16, 20, 28, 0.18) !important;
    backdrop-filter: blur(28px) saturate(180%) !important;
    -webkit-backdrop-filter: blur(28px) saturate(180%) !important;
}

body[data-theme="light"] .app {
    background: rgba(246, 248, 252, 0.22) !important;
}

.app-bar {
    background: rgba(16, 20, 28, 0.25) !important;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08) !important;
}

body[data-theme="light"] .app-bar {
    background: rgba(246, 248, 252, 0.35) !important;
    border-bottom: 1px solid rgba(0, 0, 0, 0.08) !important;
}

.quick-nav {
    background: rgba(16, 20, 28, 0.30) !important;
    border-top: 1px solid rgba(255, 255, 255, 0.08) !important;
}

body[data-theme="light"] .quick-nav {
    background: rgba(246, 248, 252, 0.40) !important;
    border-top: 1px solid rgba(0, 0, 0, 0.08) !important;
}

.side-drawer {
    background: rgba(20, 24, 34, 0.35) !important;
    border-left: 1px solid rgba(255, 255, 255, 0.08) !important;
}

body[data-theme="light"] .side-drawer {
    background: rgba(240, 242, 246, 0.45) !important;
    border-left: 1px solid rgba(0, 0, 0, 0.08) !important;
}

#phone-screen {
    background-color: #000000 !important;
}
"#;

pub const V4_BRIDGE_JS: &str = r#"/* v4_bridge.js - Native Rust IPC Shim for pywebview.api */
(function() {
    console.log("[iPhone Link v4.0 Native Rust] Bridge initialized");

    window.pywebview = window.pywebview || {};
    window.pywebview.api = {
        close: function() {
            return fetch('/api/window/close', { method: 'POST' });
        },
        minimize: function() {
            return fetch('/api/window/minimize', { method: 'POST' });
        },
        minimize_to_tray: function() {
            return fetch('/api/window/hide_to_tray', { method: 'POST' });
        },
        toggle_fullscreen: function(targetState) {
            return fetch('/api/window/toggle_fullscreen', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ state: targetState })
            }).then(r => r.json());
        },
        set_drawer: function(open) {
            return fetch('/api/window/set_drawer', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ open: !!open })
            }).then(r => r.json());
        },
        set_orientation: function(landscape, drawer_open) {
            return fetch('/api/window/set_orientation', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ landscape: !!landscape, drawer: !!drawer_open })
            }).then(r => r.json());
        },
        sync_device_aspect: function(w, drawer_open) {
            return fetch('/api/window/sync_aspect', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ width: w, drawer: !!drawer_open })
            }).then(r => r.json());
        },
        set_theme: function(theme) {
            return fetch('/api/window/set_theme', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ theme: theme })
            }).then(r => r.json());
        },
        start_window_drag: function() {
            return fetch('/api/window/start_drag', { method: 'POST' });
        },
        open_external_link: function(url) {
            return fetch('/api/system/open_url', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ url: url })
            });
        },
        get_system_info: function() {
            return fetch('/api/status').then(r => r.json());
        }
    };
})();
"#;

// ============================================================================
// APP STATE & CONFIGURATION
// ============================================================================

#[derive(Debug, Clone)]
pub struct AppState {
    pub language: Lang,
    pub theme: String, // "dark" or "light"
    pub is_streaming: bool,
    pub is_drawer_open: bool,
    pub is_landscape: bool,
    pub is_fullscreen: bool,
    pub selected_udid: String,
    pub transport: String, // "usb" or "wifi"
    pub window_hwnd: usize, // HWND handle as usize
    pub auto_rotate: bool,
    pub crop: bool,
    pub trackpad_speed: f32,
    pub audio_routing: String,
    pub server_port: u16,
    pub geometry: WindowGeometryManager,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            language: Lang::Id,
            theme: "dark".to_string(),
            is_streaming: false,
            is_drawer_open: false,
            is_landscape: false,
            is_fullscreen: false,
            selected_udid: "00008150-000231AC2669401C".to_string(),
            transport: "usb".to_string(),
            window_hwnd: 0,
            auto_rotate: true,
            crop: true,
            trackpad_speed: 1.0,
            audio_routing: "pc".to_string(),
            server_port: DEFAULT_PORT,
            geometry: WindowGeometryManager::new(1.0),
        }
    }
}

// ============================================================================
// HTTP REQUEST & RESPONSE ABSTRACTION
// ============================================================================

#[derive(Debug)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl HttpRequest {
    pub fn parse(raw: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(raw);
        let mut lines = text.lines();
        let first_line = lines.next()?;
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }

        let method = parts[0].to_string();
        let full_path = parts[1];
        let (path, query) = if let Some(idx) = full_path.find('?') {
            (full_path[..idx].to_string(), full_path[idx + 1..].to_string())
        } else {
            (full_path.to_string(), String::new())
        };

        let mut headers = HashMap::new();
        for line in lines.by_ref() {
            if line.is_empty() || line == "\r" {
                break;
            }
            if let Some(colon) = line.find(':') {
                let key = line[..colon].trim().to_lowercase();
                let val = line[colon + 1..].trim().to_string();
                headers.insert(key, val);
            }
        }

        // Body extraction
        let sep = b"\r\n\r\n";
        let body = if let Some(idx) = raw.windows(4).position(|w| w == sep) {
            String::from_utf8_lossy(&raw[idx + 4..]).to_string()
        } else {
            String::new()
        };

        Some(HttpRequest {
            method,
            path,
            query,
            headers,
            body,
        })
    }
}

pub struct HttpResponse {
    pub status_code: u16,
    pub status_text: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok_json(json: &str) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "application/json; charset=utf-8",
            body: json.as_bytes().to_vec(),
        }
    }

    pub fn ok_html(html: &str) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "text/html; charset=utf-8",
            body: html.as_bytes().to_vec(),
        }
    }

    pub fn ok_css(css: &str) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "text/css; charset=utf-8",
            body: css.as_bytes().to_vec(),
        }
    }

    pub fn ok_js(js: &str) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "application/javascript; charset=utf-8",
            body: js.as_bytes().to_vec(),
        }
    }

    pub fn ok_bytes(bytes: Vec<u8>, mime: &'static str) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: mime,
            body: bytes,
        }
    }

    pub fn not_found() -> Self {
        Self {
            status_code: 404,
            status_text: "Not Found",
            content_type: "text/plain",
            body: b"404 Not Found".to_vec(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let header = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n",
            self.status_code, self.status_text, self.content_type, self.body.len()
        );
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

// ============================================================================
// ROUTER & DISPATCHER
// ============================================================================

pub fn handle_request(req: &HttpRequest, state: &Arc<Mutex<AppState>>, web_dir: &Path) -> HttpResponse {
    if req.method == "OPTIONS" {
        return HttpResponse {
            status_code: 200,
            status_text: "OK",
            content_type: "text/plain",
            body: Vec::new(),
        };
    }

    let path = req.path.as_str();

    // 1. Static asset routes
    if path == "/v4_true_acrylic.css" {
        return HttpResponse::ok_css(V4_TRUE_ACRYLIC_CSS);
    }
    if path == "/v4_bridge.js" {
        return HttpResponse::ok_js(V4_BRIDGE_JS);
    }

    // 2. API: Status
    if path == "/api/status" {
        let st = state.lock().unwrap().clone();
        let lang_code = st.language.code();
        let status_text = translate_dynamic_status("Wi-Fi siap", st.language);

        let json = format!(
            r#"{{
  "devices": [
    {{
      "udid": "{}",
      "name": "iPhone 17 Pro",
      "model": "iPhone 17 Pro",
      "screen_type": "DynamicIsland",
      "connection_type": "{}",
      "paired": true,
      "wifi_enabled": true
    }}
  ],
  "selectedUdid": "{}",
  "transport": "{}",
  "screen": {{
    "active": {},
    "fps": 60,
    "width": 1202,
    "height": 2598,
    "orientation": {}
  }},
  "audio": {{
    "active": false,
    "wasapi": true,
    "volume": 100,
    "lip_sync_ms": 0
  }},
  "wifi": {{
    "paired": true,
    "ip": "192.168.100.10",
    "service": "{}"
  }},
  "settings": {{
    "language": "{}",
    "theme": "{}",
    "autoRotate": {},
    "crop": {},
    "trackpadSpeed": {:.1},
    "audioRouting": "{}"
  }},
  "status_text": "{}",
  "language": "{}",
  "version": "4.0.0"
}}"#,
            st.selected_udid,
            if st.transport == "usb" { "USB" } else { "Wi-Fi" },
            st.selected_udid,
            st.transport,
            st.is_streaming,
            if st.is_landscape { 270 } else { 0 },
            REMOTEPAIRING_SERVICE,
            lang_code,
            st.theme,
            st.auto_rotate,
            st.crop,
            st.trackpad_speed,
            st.audio_routing,
            status_text,
            lang_code
        );
        return HttpResponse::ok_json(&json);
    }

    // 3. API: Settings
    if path == "/api/settings" {
        if req.method == "POST" {
            let mut st = state.lock().unwrap();
            if req.body.contains(r#""language": "en""#) || req.body.contains(r#""language":"en""#) {
                st.language = Lang::En;
            } else if req.body.contains(r#""language": "id""#) || req.body.contains(r#""language":"id""#) {
                st.language = Lang::Id;
            }

            if req.body.contains(r#""theme": "light""#) || req.body.contains(r#""theme":"light""#) {
                st.theme = "light".to_string();
                if st.window_hwnd != 0 {
                    let _ = apply_true_files_acrylic_backdrop(st.window_hwnd as *mut _, false);
                }
            } else if req.body.contains(r#""theme": "dark""#) || req.body.contains(r#""theme":"dark""#) {
                st.theme = "dark".to_string();
                if st.window_hwnd != 0 {
                    let _ = apply_true_files_acrylic_backdrop(st.window_hwnd as *mut _, true);
                }
            }

            return HttpResponse::ok_json(r#"{"status":"ok","saved":true}"#);
        } else {
            let st = state.lock().unwrap();
            let json = format!(
                r#"{{"language":"{}","theme":"{}","autoRotate":{},"crop":{},"version":"4.0.0"}}"#,
                st.language.code(),
                st.theme,
                st.auto_rotate,
                st.crop
            );
            return HttpResponse::ok_json(&json);
        }
    }

    // 4. API: Device actions (HID Indigo)
    if path == "/api/device/lock" {
        let mut ctrl = IndigoHidController::new();
        let (duration, _) = ctrl.execute_synchronous_lock();
        return HttpResponse::ok_json(&format!(r#"{{"status":"ok","duration_ms":{}}}"#, duration.as_millis()));
    }

    if path == "/api/device/wake-unlock" {
        let mut ctrl = IndigoHidController::new();
        let steps = ctrl.execute_wake_and_unlock_pin("123456");
        return HttpResponse::ok_json(&format!(r#"{{"status":"ok","steps":{}}}"#, steps.2.len()));
    }

    if path == "/api/device/stealth" {
        let mut ctrl = IndigoHidController::new();
        let res = ctrl.execute_stealth_dim_toggle().unwrap_or("ok");
        return HttpResponse::ok_json(&format!(r#"{{"status":"ok","mode":"{}"}}"#, res));
    }

    if path == "/api/device/wipe-privacy-data" {
        clear_copy_cache();
        return HttpResponse::ok_json(r#"{"status":"ok","wiped":true}"#);
    }

    // 5. API: Social Link Grabber
    if path == "/api/social/grab-link" {
        let clip = read_win32_clipboard().unwrap_or_default();
        let links = extract_and_normalize_links(&clip);
        if let Some(best) = links.first() {
            write_win32_clipboard(best);
            return HttpResponse::ok_json(&format!(r#"{{"status":"ok","url":"{}"}}"#, best));
        } else {
            return HttpResponse::ok_json(r#"{"status":"none","url":""}"#);
        }
    }

    // 6. API: Update
    if path == "/api/update/status" || path == "/api/update/check" {
        return HttpResponse::ok_json(r#"{"status":"up_to_date","current_version":"4.0.0","channel":"stable"}"#);
    }

    // 7. API: Window Controls
    if path == "/api/window/set_drawer" {
        let mut st = state.lock().unwrap();
        let open = req.body.contains(r#""open":true"#) || req.body.contains(r#""open": true"#);
        st.is_drawer_open = open;
        st.geometry.is_drawer_open = open;
        let rect = st.geometry.recalculate_rect();

        if st.window_hwnd != 0 {
            unsafe {
                set_window_rect_native(st.window_hwnd as *mut _, rect);
            }
        }
        return HttpResponse::ok_json(&format!(r#"{{"status":"ok","width":{},"height":{}}}"#, rect.width(), rect.height()));
    }

    if path == "/api/window/set_orientation" {
        let mut st = state.lock().unwrap();
        let land = req.body.contains(r#""landscape":true"#) || req.body.contains(r#""landscape": true"#);
        st.is_landscape = land;
        st.geometry.is_landscape = land;
        let rect = st.geometry.recalculate_rect();

        if st.window_hwnd != 0 {
            unsafe {
                set_window_rect_native(st.window_hwnd as *mut _, rect);
            }
        }
        return HttpResponse::ok_json(&format!(r#"{{"status":"ok","width":{},"height":{}}}"#, rect.width(), rect.height()));
    }

    if path == "/api/window/set_theme" {
        let mut st = state.lock().unwrap();
        let is_light = req.body.contains(r#""theme":"light""#) || req.body.contains(r#""theme": "light""#);
        st.theme = if is_light { "light".to_string() } else { "dark".to_string() };

        if st.window_hwnd != 0 {
            let _ = apply_true_files_acrylic_backdrop(st.window_hwnd as *mut _, !is_light);
        }
        return HttpResponse::ok_json(&format!(r#"{{"status":"ok","theme":"{}"}}"#, st.theme));
    }

    if path == "/api/window/toggle_fullscreen" {
        let mut st = state.lock().unwrap();
        let mock_work = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        let rect = st.geometry.toggle_fullscreen(mock_work);
        st.is_fullscreen = st.geometry.is_fullscreen;

        if st.window_hwnd != 0 {
            unsafe {
                set_window_rect_native(st.window_hwnd as *mut _, rect);
            }
        }
        return HttpResponse::ok_json(&format!(r#"{{"status":"ok","fullscreen":{},"width":{},"height":{}}}"#, st.is_fullscreen, rect.width(), rect.height()));
    }

    if path == "/api/window/close" {
        let st = state.lock().unwrap();
        if st.window_hwnd != 0 {
            unsafe {
                post_quit_message(st.window_hwnd as *mut _);
            }
        }
        return HttpResponse::ok_json(r#"{"status":"closing"}"#);
    }

    if path == "/api/window/minimize" {
        let st = state.lock().unwrap();
        if st.window_hwnd != 0 {
            unsafe {
                show_window_native(st.window_hwnd as *mut _, 6); // SW_MINIMIZE
            }
        }
        return HttpResponse::ok_json(r#"{"status":"minimized"}"#);
    }

    if path == "/api/window/hide_to_tray" {
        let st = state.lock().unwrap();
        if st.window_hwnd != 0 {
            unsafe {
                show_window_native(st.window_hwnd as *mut _, 0); // SW_HIDE
            }
        }
        return HttpResponse::ok_json(r#"{"status":"hidden"}"#);
    }

    // 8. Screen streaming toggle
    if path == "/api/screen/start" {
        let mut st = state.lock().unwrap();
        st.is_streaming = true;
        return HttpResponse::ok_json(r#"{"status":"started","fps":60}"#);
    }

    if path == "/api/screen/stop" {
        let mut st = state.lock().unwrap();
        st.is_streaming = false;
        return HttpResponse::ok_json(r#"{"status":"stopped"}"#);
    }

    // 9. Static File Serving from web_dir
    let clean_path = path.trim_start_matches('/');
    let target_file = if clean_path.is_empty() || clean_path == "index.html" {
        web_dir.join("index.html")
    } else {
        web_dir.join(clean_path)
    };

    if target_file.is_file() {
        if let Ok(bytes) = fs::read(&target_file) {
            let mime = if target_file.ends_with(".html") {
                "text/html; charset=utf-8"
            } else if target_file.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if target_file.ends_with(".js") {
                "application/javascript; charset=utf-8"
            } else if target_file.ends_with(".svg") {
                "image/svg+xml"
            } else if target_file.ends_with(".png") {
                "image/png"
            } else if target_file.ends_with(".ico") {
                "image/x-icon"
            } else {
                "application/octet-stream"
            };

            // Inject CSS and JS bridge into index.html
            if target_file.ends_with("index.html") {
                let mut html = String::from_utf8_lossy(&bytes).to_string();
                if !html.contains("v4_true_acrylic.css") {
                    html = html.replace("</head>", r#"    <link rel="stylesheet" href="/v4_true_acrylic.css">
    <script src="/v4_bridge.js"></script>
</head>"#);
                }
                return HttpResponse::ok_html(&html);
            }

            return HttpResponse::ok_bytes(bytes, mime);
        }
    }

    // Fallback simple landing page
    if path == "/" || path == "/index.html" {
        let fallback_html = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>iPhone Link v4.0 (Native Rust)</title>
    <link rel="stylesheet" href="/v4_true_acrylic.css">
    <script src="/v4_bridge.js"></script>
    <style>
        body {{ font-family: 'Segoe UI Variable Text', -apple-system, sans-serif; margin: 0; padding: 24px; color: #ffffff; text-align: center; }}
        .launchpad {{ background: rgba(255,255,255,0.06); border-radius: 16px; padding: 32px 24px; max-width: 300px; margin: 40px auto; }}
        h1 {{ font-size: 20px; margin: 0 0 12px; }}
        p {{ font-size: 13px; opacity: 0.8; margin: 0 0 24px; }}
        button {{ background: #007aff; color: white; border: none; padding: 10px 20px; border-radius: 8px; font-weight: 600; cursor: pointer; }}
    </style>
</head>
<body data-theme="dark">
    <div class="launchpad">
        <h1>iPhone Link v4.0</h1>
        <p>Native Rust &amp; True DWM Acrylic Blur</p>
        <button onclick="window.pywebview.api.set_drawer(true)">Open Drawer</button>
    </div>
</body>
</html>"#
        );
        return HttpResponse::ok_html(&fallback_html);
    }

    HttpResponse::not_found()
}

// ============================================================================
// NATIVE WIN32 HELPER CALLS
// ============================================================================

#[link(name = "user32")]
extern "system" {
    fn SetWindowPos(hwnd: *mut std::ffi::c_void, hwndInsertAfter: *mut std::ffi::c_void, x: i32, y: i32, cx: i32, cy: i32, uFlags: u32) -> i32;
    fn ShowWindowAsync(hwnd: *mut std::ffi::c_void, nCmdShow: i32) -> i32;
    fn PostMessageW(hwnd: *mut std::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
}

unsafe fn set_window_rect_native(hwnd: *mut std::ffi::c_void, rect: RECT) {
    if !hwnd.is_null() {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            0x4000 | 0x0004 | 0x0010, // SWP_ASYNCWINDOWPOS | SWP_NOZORDER | SWP_NOACTIVATE
        );
    }
}

unsafe fn show_window_native(hwnd: *mut std::ffi::c_void, cmd: i32) {
    if !hwnd.is_null() {
        ShowWindowAsync(hwnd, cmd);
    }
}

unsafe fn post_quit_message(hwnd: *mut std::ffi::c_void) {
    if !hwnd.is_null() {
        PostMessageW(hwnd, 0x0010, 0, 0); // WM_CLOSE
    }
}

// ============================================================================
// SERVER ENGINE
// ============================================================================

pub struct NativeServer {
    pub port: u16,
    running: Arc<AtomicBool>,
}

impl NativeServer {
    pub fn start(state: Arc<Mutex<AppState>>, web_dir: PathBuf, port: u16) -> Result<Self, String> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .or_else(|_| TcpListener::bind("127.0.0.1:0"))
            .map_err(|e| format!("Failed to bind TCP listener: {}", e))?;

        let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        {
            let mut st = state.lock().unwrap();
            st.server_port = actual_port;
        }

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        listener.set_nonblocking(true).map_err(|e| format!("Failed to set nonblocking: {}", e))?;

        thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));

                        let mut buf = [0u8; 8192];
                        let read_res = stream.read(&mut buf);
                        if let Ok(n) = read_res {
                            if n > 0 {
                                if let Some(req) = HttpRequest::parse(&buf[..n]) {
                                    let resp = handle_request(&req, &state, &web_dir);
                                    let resp_bytes = resp.to_bytes();
                                    let _ = stream.write_all(&resp_bytes);
                                    let _ = stream.flush();
                                    let _ = stream.shutdown(std::net::Shutdown::Both);
                                }
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });

        Ok(NativeServer {
            port: actual_port,
            running,
        })
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_request_parsing() {
        let raw = b"GET /api/status?v=1 HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\n\r\n";
        let req = HttpRequest::parse(raw).expect("Must parse request");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/api/status");
        assert_eq!(req.query, "v=1");
        assert_eq!(req.headers.get("host").unwrap(), "127.0.0.1:8765");
    }

    #[test]
    fn test_server_status_and_acrylic_response() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_css = HttpRequest {
            method: "GET".to_string(),
            path: "/v4_true_acrylic.css".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
        };
        let resp_css = handle_request(&req_css, &state, Path::new("web"));
        assert_eq!(resp_css.status_code, 200);
        assert!(String::from_utf8_lossy(&resp_css.body).contains("display: none !important"));

        let req_status = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
        };
        let resp_status = handle_request(&req_status, &state, Path::new("web"));
        assert_eq!(resp_status.status_code, 200);
        let body_str = String::from_utf8_lossy(&resp_status.body);
        assert!(body_str.contains(r#""version": "4.0.0""#));
    }
}
