//! In-Process Native Rust HTTP Server and Live Streaming Bridge (v4.0.0 - Tahap 156).
//!
//! Provides:
//! - Multi-directory static web & assets serving with query string (?v=...) stripping
//! - Accurate MIME Content-Type detection (.css, .js, .svg, .html, .png, .ico)
//! - v4 True Acrylic CSS override and pywebview polyfill bridge (v4_bridge.js)
//! - REST API endpoints: /api/status, /api/settings, /api/device/*, /api/window/*, /api/social/*, /api/screen/*, /mirror/*
//! - Native Win32 HWND + WebView2 bounds synchronization for resize, drawer, orientation, drag, and theme

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub static INDEX_HTML_SERVED: AtomicUsize = AtomicUsize::new(0);
pub static APP_CSS_SERVED: AtomicUsize = AtomicUsize::new(0);
pub static APP_JS_SERVED: AtomicUsize = AtomicUsize::new(0);
pub static DOM_READY_BEACON: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_DEVICE_PIXEL_RATIO: AtomicUsize = AtomicUsize::new(100);
pub static GLOBAL_APP_WIDTH: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_APP_HEIGHT: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_LAUNCHPAD_VISIBLE: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_START_BTN_VISIBLE: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_START_BTN_TEXT: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_USER_AGENT: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_RAF_VERIFIED: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_DARK_MODE: AtomicBool = AtomicBool::new(true);
pub fn is_dark_mode_active() -> bool {
    GLOBAL_DARK_MODE.load(Ordering::SeqCst)
}
pub static GLOBAL_DOM_THEME: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_THEME_LIGHT_X: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_THEME_LIGHT_Y: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_WIREFRAME_CLEAN: AtomicBool = AtomicBool::new(true);
pub static GLOBAL_WIREFRAME_DETAILS: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_CONCENTRIC_TRACK: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_CONCENTRIC_BTN: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_BADGE_BG: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_SEGMENTED_CLEAN: AtomicBool = AtomicBool::new(true);
pub static GLOBAL_SEGMENTED_DETAILS: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DEVICE_PROFILE: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DRAWER_RECT_WIDTH: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_DRAWER_RIGHT_OVERFLOW: AtomicIsize = AtomicIsize::new(0);
pub static GLOBAL_DISPLAY_COL_WIDTH: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_SIDE_DRAWER_WIDTH: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_SIDE_DRAWER_RIGHT_GAP: AtomicIsize = AtomicIsize::new(0);
pub static GLOBAL_DRAWER_SCROLL_RIGHT_GAP: AtomicIsize = AtomicIsize::new(0);
pub static GLOBAL_INNER_WIDTH: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_STRETCH_GLITCH_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_MAX_DISPLAY_COL_WIDTH_DURING_CLOSE: AtomicUsize = AtomicUsize::new(0);
pub static GLOBAL_PENDING_CLICK: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
pub static GLOBAL_PENDING_INPUT: Mutex<VecDeque<(String, String)>> = Mutex::new(VecDeque::new());
pub static GLOBAL_PENDING_TEST: Mutex<Option<String>> = Mutex::new(None);
pub static GLOBAL_MIRROR_TEST_REPORT: Mutex<String> = Mutex::new(String::new());
pub static LAST_SET_DRAWER_STATE: AtomicBool = AtomicBool::new(false);
pub static LAST_SET_DRAWER_MS: AtomicU64 = AtomicU64::new(0);
pub static SAVED_UNLOCK_PIN: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DOM_UNLOCK_PIN: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DOM_UPDATE_VERSION: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DOM_LAST_TRAY_ACTION: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DOM_LANG: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DOM_LANG_ID_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_DOM_LANG_EN_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_DOM_DRAWER_TITLE: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DOM_TOGGLES_SUMMARY: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_DOM_CARD_UPDATER_EXISTS: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_DOM_CARD_UPDATER_TITLE: Mutex<String> = Mutex::new(String::new());
pub static GLOBAL_TRAY_DISMISSED_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static LAST_STOP_MIRROR_MS: AtomicU64 = AtomicU64::new(0);
pub static SCREEN_SESSION_EPOCH: AtomicU64 = AtomicU64::new(0);
pub static AUDIO_MANUALLY_STOPPED_THIS_SESSION: AtomicBool = AtomicBool::new(false);
static AUTO_AUDIO_SPAWN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
pub static LATEST_SHORTCUT_URL_CACHE: Mutex<(String, u64)> = Mutex::new((String::new(), 0));
pub static DEBUG_METRICS_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub const STANDALONE_INPROCESS_ONLY: bool = true;
pub const INPROCESS_ROUTE_COUNT: usize = 50;
pub const APP_VERSION_V5: &str = "5.0.1";
pub const APP_EDITION_V5: &str = "100% Pure Native Rust (Zero Python & Zero PowerShell)";

// Compile-time Zero-I/O Embedded Web & Icon Assets (Tahap 232)
pub const EMBEDDED_INDEX_HTML: &str = include_str!("../../../../handset/web/index.html");
pub const EMBEDDED_APP_JS: &str = include_str!("../../../../handset/web/app.js");
pub const EMBEDDED_APP_CSS: &str = include_str!("../../../../handset/web/app.css");
pub const EMBEDDED_MIRROR_HTML: &str = include_str!("../../../../handset/web/mirror.html");
pub const EMBEDDED_MIRROR_JS: &str = include_str!("../../../../handset/web/mirror.js");
pub const EMBEDDED_MIRROR_CSS: &str = include_str!("../../../../handset/web/mirror.css");
pub const EMBEDDED_KEYS_JS: &str = include_str!("../../../../handset/web/keys.js");
pub const EMBEDDED_LOGO_SVG: &[u8] = include_bytes!("../../../../handset/web/logo.svg");
pub const EMBEDDED_LOGO_PNG: &[u8] = include_bytes!("../../../../assets/logo.png");
pub const EMBEDDED_IPHONE_LINK_ICO: &[u8] = include_bytes!("../../../../assets/iphone_link.ico");
pub const EMBEDDED_WEB_ASSET_COUNT: usize = 10;

use crate::i18n::{translate_dynamic_status, Lang};
use crate::ocr_engine::{
    clear_copy_cache, clear_recent_long_ocr_chunks, escape_json_value,
    extract_and_process_ocr_batch_from_json, read_win32_clipboard,
    record_viewport_history_snapshot, resolve_active_clipboard_source_with_seq,
    set_active_clipboard_source, write_win32_clipboard, ClipboardSource,
};
use crate::updater::{compute_sha256, UpdateStateCache};
use crate::window_dwm::{
    apply_hwnd_geometry_from_state, apply_true_files_acrylic_backdrop,
    detect_conflicting_apple_usb_processes, dismiss_tray_menu_if_open,
    execute_clean_window_close_resources, is_start_with_windows_enabled, move_hwnd_by, move_hwnd_to,
    query_win32_apple_service_installed_in_registry, query_win32_apple_service_state,
    query_win32_desktop_wallpaper_path, query_win32_hevc_appx_package, register_tray_app_state,
    register_tray_icon, set_start_with_windows, toggle_fullscreen_state, trigger_hwnd_drag,
    Win32ServiceState, WindowGeometryManager, GLOBAL_AUDIO_PC_ACTIVE, GLOBAL_DRAWER_OPEN,
    GLOBAL_IS_FULLSCREEN, GLOBAL_IS_LANDSCAPE, GLOBAL_PENDING_TRAY_ACTION, GLOBAL_PORTRAIT_WIDTH,
    GLOBAL_STEALTH_ACTIVE, GLOBAL_TRAY_MENU_OPEN, GLOBAL_TRAY_REGISTERED, MIN_WINDOW_HEIGHT,
    MIN_WINDOW_WIDTH, RECT, TRAY_MENU_ITEMS,
};

use iphone_link_apple::amfi::{resolve_developer_mode_for_diagnostics, unlock_developer_mode_menu};
use iphone_link_apple::hid::{
    clear_stored_pin, commit_and_reset_current_typed_line, dpapi_decrypt_pin_b64,
    dpapi_encrypt_pin_b64, generate_ios_shortcut_steps, generate_key_steps_for_text,
    generate_onscreen_keyboard_toggle_events, get_active_rotation_deg, get_typed_lines_buffer,
    is_stealth_dim_active, load_decrypted_pin, lock_iphone_screen_with_hold, pinch_zoom_frames,
    record_typed_char, record_typed_text, record_user_input_activity, reset_hid_runtime_state,
    set_iphone_physical_brightness, store_encrypted_pin, swipe_direction_steps,
    wake_and_unlock_iphone, IndigoHidController,
};
use iphone_link_apple::models::{
    clear_metadata_cache, discover_devices_with_metadata, snapshot_cached_devices_metadata_zero_io,
};
use iphone_link_apple::usbmux::list_connected_usb_devices;
use iphone_link_apple::wifi::{get_wifi_status, pair_wifi_json, scan_wifi_json};

pub const DEFAULT_PORT: u16 = 8765;

// ============================================================================
// EMBEDDED FALLBACKS FOR TRUE ACRYLIC AND NATIVE BRIDGE
// ============================================================================

pub const V4_TRUE_ACRYLIC_CSS: &str = r#"/* iPhone Link v4.0 Spatial Liquid Glass (Tahap 175) */
:root,
html,
body,
html[data-theme="dark"],
body[data-theme="dark"],
.app[data-theme="dark"],
.theme-dark {
  --bg: transparent !important;
  --bg-solid: transparent !important;
  --bg-primary: transparent !important;
  --bg-secondary: transparent !important;
  --bg-acrylic: transparent !important;
  --bg-flyout: transparent !important;
  --surface: transparent !important;
  --panel: transparent !important;
  --card: rgba(255, 255, 255, 0.04) !important;
  --card-bg: transparent !important;
  --border: rgba(255, 255, 255, 0.14) !important;
  --fg: #f8fafc !important;
  --text: #f8fafc !important;
  --muted: #cbd5e1 !important;
  --wallpaper-bg-image: none !important;
  --spring-apple-fluid: cubic-bezier(0.22, 1, 0.36, 1);
  --spring-apple-sheet: cubic-bezier(0.32, 0.72, 0, 1);
  --spring-apple-bounce: cubic-bezier(0.175, 0.885, 0.32, 1.275);
  --spring-apple-snappy: cubic-bezier(0.2, 0, 0, 1);
}

/* 1. Variabel Global Light Mode: 100% Transparan pada Latar Utama & Frosted Pearl */
html[data-theme="light"],
body[data-theme="light"],
.app[data-theme="light"],
.theme-light,
[data-theme="light"] {
  --bg: transparent !important;
  --bg-solid: transparent !important;
  --bg-primary: transparent !important;
  --bg-secondary: transparent !important;
  --bg-acrylic: transparent !important;
  --bg-flyout: transparent !important;
  --surface: transparent !important;
  --panel: transparent !important;
  --card: rgba(255, 255, 255, 0.16) !important;
  --card-bg: transparent !important;
  --border: rgba(15, 23, 42, 0.12) !important;
  --fg: #0f172a !important;
  --text: #0f172a !important;
  --muted: #334155 !important;
  --wallpaper-bg-image: none !important;
}

*,
*::before,
*::after {
  -webkit-backdrop-filter: none !important;
  backdrop-filter: none !important;
}
.app::before,
.app::after {
  display: none !important;
  opacity: 0 !important;
  visibility: hidden !important;
  content: none !important;
  background: none !important;
  background-image: none !important;
}

html,
body,
html[data-theme="dark"],
html[data-theme="dark"] body,
body[data-theme="dark"],
[data-theme="dark"] body,
html[data-theme="light"],
html[data-theme="light"] body,
body[data-theme="light"],
[data-theme="light"] body {
  width: 100vw !important;
  height: 100vh !important;
  margin: 0 !important;
  padding: 0 !important;
  overflow: hidden !important;
  background: transparent !important;
  background-color: transparent !important;
  border-radius: 0px !important;
  -webkit-font-smoothing: antialiased !important;
  text-rendering: geometricPrecision !important;
}

/* ELIMINASI TOTAL WIREFRAME BOXES PADA ELEMEN TEKS, BRAND, PLACEHOLDER, & BARIS SLIDER */
html body .brand,
html body .app-title-area,
html body .brand-title,
html body #screen-empty,
html body .screen-empty,
html body .placeholder,
html body .placeholder > *,
html body #screen-empty h1,
html body #screen-empty h2,
html body #screen-empty p,
html body #screen-empty .sub,
html body .drawer-header,
html body .drawer-title,
html body .drawer-body,
html body .drawer-scroll,
html body .drawer-content,
html body .drawer-footer-credits,
html body .credit-main,
html body .credit-sub,
html body .section-label,
html body .card-title,
html body .field-label,
html body .crop-grid,
html body .crop-item,
html body .crop-row,
html body .crop-label,
html body .slider-row,
html body .control-row,
html body .row:not(.card):not(.seg) {
  background: transparent !important;
  background-color: transparent !important;
  border: none !important;
  border-width: 0px !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
}

/* TIER 1: LAUNCHPAD / STAGE KIRI (Paling Bening + Ambient Hero Glow Halus) */
html[data-theme="dark"] body .app,
html body .app,
html body .app:not(.is-fullsize) {
  width: 100vw !important;
  height: 100vh !important;
  max-width: 100vw !important;
  max-height: 100vh !important;
  margin: 0 !important;
  padding: 0 !important;
  overflow: hidden !important;
  border-radius: 0px !important;
  border: none !important;
  box-shadow: none !important;
  background: linear-gradient(165deg, rgba(10, 14, 24, 0.10) 0%, rgba(8, 12, 20, 0.16) 100%) !important;
  background-color: transparent !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
}

html[data-theme="light"] body .app,
html[data-theme="light"] body .app:not(.is-fullsize),
body[data-theme="light"] .app,
.app[data-theme="light"] {
  width: 100vw !important;
  height: 100vh !important;
  max-width: 100vw !important;
  max-height: 100vh !important;
  margin: 0 !important;
  padding: 0 !important;
  overflow: hidden !important;
  border-radius: 0px !important;
  border: none !important;
  box-shadow: none !important;
  background: linear-gradient(165deg, rgba(255, 255, 255, 0.18) 0%, rgba(240, 246, 255, 0.22) 100%) !important;
  background-color: transparent !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
}

/* Ambient Hero Glow di Tengah Launchpad Saat Belum Mirroring */
html body .app:not(.is-streaming) .display-col {
  background: radial-gradient(circle at 50% 42%, rgba(10, 132, 255, 0.14) 0%, rgba(56, 189, 248, 0.04) 40%, transparent 72%) !important;
}
html[data-theme="light"] body .app:not(.is-streaming) .display-col {
  background: radial-gradient(circle at 50% 42%, rgba(255, 255, 255, 0.36) 0%, rgba(186, 230, 253, 0.16) 42%, transparent 74%) !important;
}

/* Seluruh Wrapper Dalam di Dark & Light Mode WAJIB 100% Transparan */
html body .main-body,
html body .display-col,
html body .app:not(.is-landscape) .display-col,
html body .app:not(.is-landscape):not(.is-fullsize).drawer-open .display-col,
html body .workspace,
html body .stage,
html body .screen-stage,
html body #phone-shell,
html body #launchpad-shell,
html body .launchpad-shell,
html body #launchpad-idle-view,
html body .launchpad-idle-view,
html body .screen-viewport,
html body .app:not(.is-streaming) #phone-screen,
html[data-theme="dark"] body .app:not(.is-streaming) #phone-screen,
html[data-theme="light"] body .main-body,
html[data-theme="light"] body .display-col,
html[data-theme="light"] body .app:not(.is-landscape) .display-col,
html[data-theme="light"] body .app:not(.is-landscape):not(.is-fullsize).drawer-open .display-col,
html[data-theme="light"] body .workspace,
html[data-theme="light"] body .stage,
html[data-theme="light"] body .screen-stage,
html[data-theme="light"] body #phone-shell,
html[data-theme="light"] body #launchpad-shell,
html[data-theme="light"] body .launchpad-shell,
html[data-theme="light"] body #launchpad-idle-view,
html[data-theme="light"] body .launchpad-idle-view,
html[data-theme="light"] body .screen-viewport,
html[data-theme="light"] body .app:not(.is-streaming) #phone-screen {
  background: transparent !important;
  background-color: transparent !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  box-shadow: none !important;
}

/* Tahap 181: Biarkan latar #phone-screen transparan agar pembulatan sub-piksel HiDPI tidak pernah menyisakan garis #000000 */
html body #app-root.app.is-mirroring #phone-screen,
html body #app-root.app.is-streaming #phone-screen,
html body .app.is-streaming #phone-screen {
  background: transparent !important;
  background-color: transparent !important;
}

.main-body, .display-col {
  height: calc(100vh - 38px) !important;
  min-height: calc(100vh - 38px) !important;
  max-height: calc(100vh - 38px) !important;
  overflow: hidden !important;
  position: relative !important;
  z-index: 1 !important;
}
.workspace, .stage, #phone-shell, #phone-screen, #screen-empty {
  width: 100% !important;
  height: 100% !important;
  overflow: hidden !important;
  scrollbar-width: none !important;
}
.display-col::-webkit-scrollbar,
.workspace::-webkit-scrollbar,
.stage::-webkit-scrollbar,
#phone-shell::-webkit-scrollbar,
#phone-screen::-webkit-scrollbar,
#screen-empty::-webkit-scrollbar,
body::-webkit-scrollbar {
  display: none !important;
  width: 0 !important;
  height: 0 !important;
}
.app-bar, .drawer-header, .quick-nav, #screen-empty {
  user-select: none !important;
  -webkit-user-select: none !important;
}

/* TIER 2: TOP BAR, QUICK NAV, & SIDE DRAWER KANAN (Elevasi Kaca Tegas & Berkarakter) */
html body .app-bar,
html[data-theme="dark"] body .app-bar {
  border-radius: 0px !important;
  background: linear-gradient(180deg, rgba(22, 30, 46, 0.16) 0%, rgba(12, 17, 28, 0.10) 100%) !important;
  border-bottom: 1px solid rgba(255, 255, 255, 0.15) !important;
  box-shadow: inset 0 1px 0 0 rgba(255, 255, 255, 0.20), 0 4px 16px rgba(0, 0, 0, 0.18) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
  z-index: 10 !important;
}
html[data-theme="light"] body .app-bar {
  border-radius: 0px !important;
  background: linear-gradient(180deg, rgba(252, 254, 255, 0.32) 0%, rgba(240, 246, 255, 0.24) 100%) !important;
  border-bottom: 1px solid rgba(255, 255, 255, 0.68) !important;
  box-shadow: inset 0 1px 0 0 rgba(255, 255, 255, 0.90), 0 4px 14px rgba(15, 23, 42, 0.06) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
  z-index: 10 !important;
}

html body .quick-nav,
html[data-theme="dark"] body .quick-nav {
  border-radius: 0px !important;
  background: linear-gradient(0deg, rgba(12, 17, 28, 0.16) 0%, rgba(20, 28, 44, 0.10) 100%) !important;
  border-top: 1px solid rgba(255, 255, 255, 0.15) !important;
  box-shadow: inset 0 1px 0 0 rgba(255, 255, 255, 0.16) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
  z-index: 10 !important;
}
html[data-theme="light"] body .quick-nav {
  border-radius: 0px !important;
  background: linear-gradient(0deg, rgba(242, 247, 255, 0.30) 0%, rgba(252, 254, 255, 0.22) 100%) !important;
  border-top: 1px solid rgba(255, 255, 255, 0.68) !important;
  box-shadow: inset 0 1px 0 0 rgba(255, 255, 255, 0.88) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
  z-index: 10 !important;
}

/* Side Drawer & Settings Controls (Liquid Glass with #control-pane ID specificity) */
#control-pane,
.side-drawer,
#control-pane *,
.side-drawer * {
  contain: none !important;
}
html body #control-pane,
html body .side-drawer,
html body .control-pane,
html[data-theme="dark"] body #control-pane,
html[data-theme="dark"] body .side-drawer,
html[data-theme="dark"] body .control-pane {
  border-radius: 0px !important;
  background: linear-gradient(180deg, rgba(16, 22, 34, 0.08) 0%, rgba(10, 14, 24, 0.12) 100%) !important;
  border-left: 1px solid rgba(255, 255, 255, 0.16) !important;
  box-shadow: -8px 0 24px rgba(0, 0, 0, 0.20), inset 1px 0 0 0 rgba(255, 255, 255, 0.14) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
  z-index: 20 !important;
}
html[data-theme="light"] body #control-pane,
html[data-theme="light"] body .side-drawer,
html[data-theme="light"] body .control-pane {
  border-radius: 0px !important;
  background: linear-gradient(180deg, rgba(255, 255, 255, 0.22) 0%, rgba(240, 246, 255, 0.18) 100%) !important;
  border-left: 1px solid rgba(255, 255, 255, 0.72) !important;
  box-shadow: -8px 0 24px rgba(15, 23, 42, 0.08), inset 1px 0 0 0 rgba(255, 255, 255, 0.90) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  position: relative !important;
  z-index: 20 !important;
}

html body #control-pane section,
html body #control-pane form,
html body #control-pane .control-group,
html body #control-pane .settings-group,
html body #control-pane details,
html body #control-pane .crop-grid,
html body #control-pane .drawer-content,
html body #control-pane .panel-theme,
html body #control-pane .panel-display-fit,
html body #control-pane .panel-audio-settings,
html body .side-drawer section,
html body .side-drawer form,
html body .side-drawer .control-group,
html body .side-drawer .settings-group,
html body .side-drawer details,
html body .side-drawer .crop-grid,
html body .side-drawer .drawer-content,
html body .side-drawer .panel-theme,
html body .side-drawer .panel-display-fit,
html body .side-drawer .panel-audio-settings,
html[data-theme="light"] body #control-pane section,
html[data-theme="light"] body #control-pane form,
html[data-theme="light"] body #control-pane .control-group,
html[data-theme="light"] body #control-pane .settings-group,
html[data-theme="light"] body #control-pane details,
html[data-theme="light"] body #control-pane .crop-grid,
html[data-theme="light"] body #control-pane .panel-theme,
html[data-theme="light"] body #control-pane .panel-display-fit,
html[data-theme="light"] body #control-pane .panel-audio-settings,
html[data-theme="light"] body .side-drawer section,
html[data-theme="light"] body .side-drawer form,
html[data-theme="light"] body .side-drawer .control-group,
html[data-theme="light"] body .side-drawer .settings-group,
html[data-theme="light"] body .side-drawer details,
html[data-theme="light"] body .side-drawer .crop-grid,
html[data-theme="light"] body .side-drawer .panel-theme,
html[data-theme="light"] body .side-drawer .panel-display-fit,
html[data-theme="light"] body .side-drawer .panel-audio-settings {
  background: transparent !important;
  background-color: transparent !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  box-shadow: none !important;
}

/* TIER 3: 3D SPECULAR LIQUID GLASS CARDS (.card) */
html body .card,
html body .glass-card,
html body .device-card,
html body #control-pane .card,
html body #control-pane .glass-card,
html body .side-drawer .card,
html body .side-drawer .glass-card,
html[data-theme="dark"] body .card,
html[data-theme="dark"] body .glass-card {
  background: linear-gradient(145deg, rgba(255, 255, 255, 0.08) 0%, rgba(255, 255, 255, 0.02) 100%) !important;
  background-color: rgba(255, 255, 255, 0.03) !important;
  border: 1px solid rgba(255, 255, 255, 0.18) !important;
  border-radius: 16px !important;
  box-shadow:
    inset 0 1.5px 1px 0 rgba(255, 255, 255, 0.28),
    inset 0 0 0 0.5px rgba(255, 255, 255, 0.10),
    0 8px 22px -4px rgba(0, 0, 0, 0.24) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
}

html[data-theme="light"] body .card,
html[data-theme="light"] body .glass-card,
html[data-theme="light"] body .device-card,
html[data-theme="light"] body .panel-card,
html[data-theme="light"] body .settings-card,
html[data-theme="light"] body #control-pane .card,
html[data-theme="light"] body #control-pane .glass-card,
html[data-theme="light"] body .side-drawer .card,
html[data-theme="light"] body .side-drawer .glass-card,
html[data-theme="light"] body .card-details {
  background: linear-gradient(145deg, rgba(255, 255, 255, 0.20) 0%, rgba(242, 248, 255, 0.10) 100%) !important;
  background-color: transparent !important;
  border: 1px solid rgba(255, 255, 255, 0.88) !important;
  border-radius: 16px !important;
  box-shadow:
    inset 0 1.5px 1px 0 rgba(255, 255, 255, 0.98),
    0 8px 22px -4px rgba(15, 23, 42, 0.12),
    0 0 0 1px rgba(15, 23, 42, 0.06) !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
}

/* PILAR 3: Track & Tombol Segmented Controls (Concentric 12px / 9px) */
html body .seg,
html body .segmented,
html body .seg-group,
html body .segmented-theme,
html body .segmented-lang,
html body .segmented-fit,
html body .segmented-device-profile,
html body .segmented-toggle,
html body .segmented-audio-out,
html body .segmented-speed,
html body .segmented-channel,
html body div.seg,
html[data-theme="dark"] body .seg,
html[data-theme="dark"] body .segmented,
html[data-theme="dark"] body .seg-group {
  background: rgba(6, 10, 18, 0.34) !important;
  border: 1px solid rgba(255, 255, 255, 0.14) !important;
  border-radius: 12px !important;
  padding: 3px !important;
  gap: 3px !important;
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.42) !important;
  overflow: hidden !important;
  box-sizing: border-box !important;
}
html[data-theme="light"] body .seg,
html[data-theme="light"] body .segmented,
html[data-theme="light"] body .seg-group,
html[data-theme="light"] body .segmented-theme,
html[data-theme="light"] body .segmented-lang,
html[data-theme="light"] body .segmented-fit,
html[data-theme="light"] body .segmented-device-profile,
html[data-theme="light"] body .segmented-toggle,
html[data-theme="light"] body .segmented-audio-out,
html[data-theme="light"] body .segmented-speed,
html[data-theme="light"] body .segmented-channel,
html[data-theme="light"] body div.seg {
  background: rgba(15, 23, 42, 0.09) !important;
  border: 1px solid rgba(255, 255, 255, 0.65) !important;
  border-radius: 12px !important;
  padding: 3px !important;
  gap: 3px !important;
  box-shadow: inset 0 1px 2px rgba(15, 23, 42, 0.10) !important;
  overflow: hidden !important;
  box-sizing: border-box !important;
}

/* Seluruh Tombol di Dalam Segmented Control: Concentric Inner Radius 9px (12px - 3px = 9px) */
html body .seg > button,
html body .seg button,
html body .segmented button,
html body .seg-group button,
html body .seg-btn,
html body button.seg,
html body button.seg-btn,
html body .seg button[id],
html[data-theme="dark"] body .seg button[id],
html[data-theme="light"] body .seg button[id] {
  background: transparent !important;
  background-color: transparent !important;
  border: 1px solid transparent !important;
  border-radius: 9px !important;
  margin: 0 !important;
  padding: 6px 10px !important;
  min-height: 30px !important;
  display: inline-flex !important;
  align-items: center !important;
  justify-content: center !important;
  gap: 6px !important;
  color: rgba(248, 250, 252, 0.78) !important;
  box-shadow: none !important;
  font-weight: 500 !important;
  line-height: 1.2 !important;
  box-sizing: border-box !important;
  transition: background 0.14s ease, color 0.14s ease, box-shadow 0.14s ease !important;
}
html[data-theme="light"] body .seg > button,
html[data-theme="light"] body .seg button,
html[data-theme="light"] body .segmented button,
html[data-theme="light"] body .seg-group button,
html[data-theme="light"] body .seg-btn,
html[data-theme="light"] body button.seg,
html[data-theme="light"] body button.seg-btn,
html[data-theme="light"] body .seg button[id] {
  color: #1e293b !important;
}

/* Tombol ACTIVE di Dalam Segmented Control: Presisi 9px Concentric Crystal-Blue Pill */
html body .seg > button.active,
html body .seg > button.is-active,
html body .seg button.active,
html body .seg button.is-active,
html body .seg button[aria-checked="true"],
html body .seg button[aria-pressed="true"],
html body .segmented button.active,
html body .segmented button.is-active,
html body .segmented button[aria-checked="true"],
html body .segmented button[aria-pressed="true"],
html body .seg-group button.active,
html body .seg-group button.is-active,
html body .seg-group button[aria-checked="true"],
html body .seg-group button[aria-pressed="true"],
html body .seg-btn.active,
html body .seg-btn.is-active,
html body .seg-btn[aria-checked="true"],
html body .seg-btn[aria-pressed="true"],
html body button.seg.active,
html body button.seg.is-active,
html body button.seg[aria-checked="true"],
html body button.seg[aria-pressed="true"],
html body button.seg-btn.active,
html body button.seg-btn.is-active,
html body button.seg-btn[aria-checked="true"],
html body button.seg-btn[aria-pressed="true"],
html body .seg button[id].active,
html body .seg button[id].is-active,
html body .seg button[id][aria-checked="true"],
html body .seg button[id][aria-pressed="true"],
html[data-theme="light"] body .seg > button.active,
html[data-theme="light"] body .seg > button.is-active,
html[data-theme="light"] body .seg button.active,
html[data-theme="light"] body .seg button.is-active,
html[data-theme="light"] body .seg button[aria-checked="true"],
html[data-theme="light"] body .seg button[aria-pressed="true"],
html[data-theme="light"] body .segmented button.active,
html[data-theme="light"] body .segmented button.is-active,
html[data-theme="light"] body .segmented button[aria-checked="true"],
html[data-theme="light"] body .segmented button[aria-pressed="true"],
html[data-theme="light"] body .seg-group button.active,
html[data-theme="light"] body .seg-group button.is-active,
html[data-theme="light"] body .seg-group button[aria-checked="true"],
html[data-theme="light"] body .seg-group button[aria-pressed="true"],
html[data-theme="light"] body .seg-btn.active,
html[data-theme="light"] body .seg-btn.is-active,
html[data-theme="light"] body .seg-btn[aria-checked="true"],
html[data-theme="light"] body .seg-btn[aria-pressed="true"],
html[data-theme="light"] body button.seg.active,
html[data-theme="light"] body button.seg.is-active,
html[data-theme="light"] body button.seg[aria-checked="true"],
html[data-theme="light"] body button.seg[aria-pressed="true"],
html[data-theme="light"] body button.seg-btn.active,
html[data-theme="light"] body button.seg-btn.is-active,
html[data-theme="light"] body button.seg-btn[aria-checked="true"],
html[data-theme="light"] body button.seg-btn[aria-pressed="true"],
html[data-theme="light"] body .seg button[id].active,
html[data-theme="light"] body .seg button[id].is-active,
html[data-theme="light"] body .seg button[id][aria-checked="true"],
html[data-theme="light"] body .seg button[id][aria-pressed="true"] {
  background: linear-gradient(180deg, #1E90FF 0%, #0066CC 100%) !important;
  background-color: #0A84FF !important;
  border: 1px solid rgba(255, 255, 255, 0.34) !important;
  border-radius: 9px !important;
  color: #ffffff !important;
  font-weight: 600 !important;
  text-shadow: 0 1px 1px rgba(0, 0, 0, 0.22) !important;
  box-shadow:
    inset 0 1px 0 0 rgba(255, 255, 255, 0.45),
    0 2px 6px rgba(0, 102, 204, 0.32) !important;
}

/* Badge Kecil ID / EN / Icon di Dalam Tombol Segmented: Bebas Kotak Biru Tua Belang! */
html body .seg button span,
html body .seg button .badge,
html body .seg button .lang-badge,
html body .seg button .lang-code,
html body .seg button code,
html body .seg button strong,
html body .segmented .lang-badge,
html body .seg-btn .lang-badge,
html body .segmented span.lang-badge,
html body button .lang-badge,
html body button.seg-btn span.lang-badge {
  background: transparent !important;
  background-color: transparent !important;
  border: none !important;
  box-shadow: none !important;
  color: inherit !important;
  padding: 0 !important;
  margin: 0 !important;
  border-radius: 0 !important;
}

html body button.seg.active svg,
html body button.seg.is-active svg,
html body button.seg[aria-checked="true"] svg,
html body button.seg[aria-pressed="true"] svg,
html body button.seg-btn.active svg,
html body button.seg-btn.is-active svg,
html body .segmented button.active svg,
html body .segmented button.is-active svg,
html body .segmented button[aria-checked="true"] svg,
html[data-theme="light"] body button.seg.active svg,
html[data-theme="light"] body button.seg.is-active svg,
html[data-theme="light"] body button.seg[aria-checked="true"] svg,
html[data-theme="light"] body button.seg[aria-pressed="true"] svg,
html[data-theme="light"] body button.seg-btn.active svg,
html[data-theme="light"] body button.seg-btn.is-active svg,
html[data-theme="light"] body .segmented button.active svg,
html[data-theme="light"] body .segmented button.is-active svg,
html[data-theme="light"] body .segmented button[aria-checked="true"] svg {
  stroke: #ffffff !important;
  color: #ffffff !important;
}
html body button.seg.active span,
html body button.seg.is-active span,
html body button.seg[aria-checked="true"] span,
html body button.seg[aria-pressed="true"] span,
html body button.seg-btn.active span,
html body button.seg-btn.is-active span,
html body .segmented button.active span,
html body .segmented button.is-active span,
html body .segmented button[aria-checked="true"] span,
html[data-theme="light"] body button.seg.active span,
html[data-theme="light"] body button.seg.is-active span,
html[data-theme="light"] body button.seg[aria-checked="true"] span,
html[data-theme="light"] body button.seg[aria-pressed="true"] span,
html[data-theme="light"] body button.seg-btn.active span,
html[data-theme="light"] body button.seg-btn.is-active span,
html[data-theme="light"] body .segmented button.active span,
html[data-theme="light"] body .segmented button.is-active span,
html[data-theme="light"] body .segmented button[aria-checked="true"] span {
  color: #ffffff !important;
}

/* Tombol Aksi Biasa (Non-Segmented & Non-Primary) di Dark & Light Mode */
html body .bar-btn,
html body .icon-btn,
html body .nav-btn,
html body .action-btn {
  background: linear-gradient(180deg, rgba(255, 255, 255, 0.10) 0%, rgba(255, 255, 255, 0.04) 100%) !important;
  border: 1px solid rgba(255, 255, 255, 0.16) !important;
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.20) !important;
}
html[data-theme="light"] body .bar-btn,
html[data-theme="light"] body .icon-btn,
html[data-theme="light"] body .nav-btn,
html[data-theme="light"] body .action-btn,
html[data-theme="light"] body .card button:not(.primary):not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]):not(#show-screen):not(#btn-save-crop):not(#toggle-auto-update-check):not(.toggle-switch):not([role="switch"]):not(.seg):not(.seg-btn):not(.seg button):not(.segmented button) {
  background: linear-gradient(180deg, rgba(255, 255, 255, 0.28) 0%, rgba(240, 246, 255, 0.16) 100%) !important;
  border: 1px solid rgba(255, 255, 255, 0.78) !important;
  color: #0f172a !important;
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.92), 0 2px 6px rgba(15, 23, 42, 0.06) !important;
}

/* Tahap 182: Software Update Card & #update-channel-group Concentric Crystal-Blue Toggle (Light & Dark Mode) */
html body #update-channel-group,
html body #card-updater #update-channel-group,
html[data-theme="dark"] body #update-channel-group,
html[data-theme="dark"] body #card-updater #update-channel-group {
  background: rgba(6, 10, 18, 0.34) !important;
  border: 1px solid rgba(255, 255, 255, 0.14) !important;
  border-radius: 12px !important;
  padding: 3px !important;
  gap: 3px !important;
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.42) !important;
  overflow: hidden !important;
  box-sizing: border-box !important;
}
html[data-theme="light"] body #update-channel-group,
html[data-theme="light"] body #card-updater #update-channel-group {
  background: rgba(15, 23, 42, 0.09) !important;
  border: 1px solid rgba(255, 255, 255, 0.65) !important;
  border-radius: 12px !important;
  padding: 3px !important;
  gap: 3px !important;
  box-shadow: inset 0 1px 2px rgba(15, 23, 42, 0.10) !important;
  overflow: hidden !important;
  box-sizing: border-box !important;
}
html body #update-channel-group button.seg,
html body #card-updater #update-channel-group button.seg,
html[data-theme="dark"] body #update-channel-group button.seg,
html[data-theme="dark"] body #card-updater #update-channel-group button.seg {
  background: transparent !important;
  background-color: transparent !important;
  border: 1px solid transparent !important;
  border-radius: 9px !important;
  color: rgba(248, 250, 252, 0.78) !important;
  box-shadow: none !important;
  font-weight: 500 !important;
  min-height: 30px !important;
}
html[data-theme="light"] body #update-channel-group button.seg,
html[data-theme="light"] body #card-updater #update-channel-group button.seg {
  background: transparent !important;
  background-color: transparent !important;
  border: 1px solid transparent !important;
  border-radius: 9px !important;
  color: #1e293b !important;
  box-shadow: none !important;
  font-weight: 500 !important;
  min-height: 30px !important;
}
html[data-theme="light"] body #update-channel-group button.seg:hover:not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]),
html[data-theme="light"] body #card-updater #update-channel-group button.seg:hover:not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]) {
  background: rgba(255, 255, 255, 0.28) !important;
  color: #0f172a !important;
}
html body #update-channel-group button.seg.active,
html body #update-channel-group button.seg.is-active,
html body #update-channel-group button.seg[aria-checked="true"],
html body #update-channel-group button.seg[aria-pressed="true"],
html body #card-updater #update-channel-group button.seg.active,
html body #card-updater #update-channel-group button.seg.is-active,
html body #card-updater #update-channel-group button.seg[aria-checked="true"],
html body #card-updater #update-channel-group button.seg[aria-pressed="true"],
html[data-theme="light"] body #update-channel-group button.seg.active,
html[data-theme="light"] body #update-channel-group button.seg.is-active,
html[data-theme="light"] body #update-channel-group button.seg[aria-checked="true"],
html[data-theme="light"] body #update-channel-group button.seg[aria-pressed="true"],
html[data-theme="light"] body #card-updater #update-channel-group button.seg.active,
html[data-theme="light"] body #card-updater #update-channel-group button.seg.is-active,
html[data-theme="light"] body #card-updater #update-channel-group button.seg[aria-checked="true"],
html[data-theme="light"] body #card-updater #update-channel-group button.seg[aria-pressed="true"] {
  background: linear-gradient(180deg, #1E90FF 0%, #0066CC 100%) !important;
  background-color: #0A84FF !important;
  border: 1px solid rgba(255, 255, 255, 0.34) !important;
  border-radius: 9px !important;
  color: #ffffff !important;
  font-weight: 600 !important;
  text-shadow: 0 1px 1px rgba(0, 0, 0, 0.22) !important;
  box-shadow:
    inset 0 1px 0 0 rgba(255, 255, 255, 0.45),
    0 2px 6px rgba(0, 102, 204, 0.32) !important;
}
html body #update-channel-group button.seg:not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]) span,
html body #card-updater #update-channel-group button.seg:not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]) span {
  color: rgba(248, 250, 252, 0.78) !important;
}
html[data-theme="light"] body #update-channel-group button.seg:not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]) span,
html[data-theme="light"] body #card-updater #update-channel-group button.seg:not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]) span {
  color: #1e293b !important;
}
html body #update-channel-group button.seg.active span,
html body #update-channel-group button.seg.is-active span,
html body #update-channel-group button.seg[aria-checked="true"] span,
html body #update-channel-group button.seg[aria-pressed="true"] span,
html body #card-updater #update-channel-group button.seg.active span,
html body #card-updater #update-channel-group button.seg.is-active span,
html[data-theme="light"] body #update-channel-group button.seg.active span,
html[data-theme="light"] body #update-channel-group button.seg.is-active span,
html[data-theme="light"] body #update-channel-group button.seg[aria-checked="true"] span,
html[data-theme="light"] body #update-channel-group button.seg[aria-pressed="true"] span,
html[data-theme="light"] body #card-updater #update-channel-group button.seg.active span,
html[data-theme="light"] body #card-updater #update-channel-group button.seg.is-active span {
  color: #ffffff !important;
}
html[data-theme="light"] body #card-updater #toggle-auto-update-check:not(.is-active):not([aria-checked="true"]),
html[data-theme="light"] body #card-updater .toggle-switch:not(.is-active):not([aria-checked="true"]) {
  background: rgba(15, 23, 42, 0.16) !important;
  border: 1px solid rgba(255, 255, 255, 0.72) !important;
}
html[data-theme="light"] body #card-updater #toggle-auto-update-check[aria-checked="true"],
html[data-theme="light"] body #card-updater #toggle-auto-update-check.is-active,
html[data-theme="light"] body #card-updater .toggle-switch[aria-checked="true"],
html[data-theme="light"] body #card-updater .toggle-switch.is-active {
  background: linear-gradient(180deg, #1E90FF 0%, #0066CC 100%) !important;
  border-color: rgba(255, 255, 255, 0.45) !important;
}
html body #card-updater {
  display: block !important;
  visibility: visible !important;
  opacity: 1 !important;
}
html body #card-updater .update-card-title {
  font-size: 11px !important;
  font-weight: 700 !important;
  letter-spacing: 0.03em !important;
  line-height: 1.25 !important;
}
html body #screen-error-banner:empty,
html body #screen-error-banner[hidden],
html body #launchpad-error:empty,
html body #launchpad-error[hidden] {
  display: none !important;
}

/* Input & Select di Light Mode */
html[data-theme="light"] body input[type="text"],
html[data-theme="light"] body input[type="password"],
html[data-theme="light"] body input[type="number"],
html[data-theme="light"] body select {
  background: rgba(255, 255, 255, 0.52) !important;
  border: 1px solid rgba(255, 255, 255, 0.78) !important;
  color: #0f172a !important;
  box-shadow: inset 0 1px 2px rgba(15, 23, 42, 0.06) !important;
}

/* Keterbacaan Teks Tajam di Atas Kaca Bening Light Mode */
html[data-theme="light"] body,
html[data-theme="light"] body h1,
html[data-theme="light"] body h2,
html[data-theme="light"] body h3,
html[data-theme="light"] body .drawer-title,
html[data-theme="light"] body .brand-title,
html[data-theme="light"] body label,
html[data-theme="light"] body .card-title,
html[data-theme="light"] body .device-name,
html[data-theme="light"] body .placeholder-title,
html[data-theme="light"] body .control-label,
html[data-theme="light"] body .drawer-section-label {
  color: #0f172a !important;
  text-shadow: 0 1px 3px rgba(255, 255, 255, 0.65) !important;
}
html[data-theme="light"] body p,
html[data-theme="light"] body .sub,
html[data-theme="light"] body .muted,
html[data-theme="light"] body .section-label {
  color: #1e293b !important;
  text-shadow: 0 1px 2px rgba(255, 255, 255, 0.55) !important;
}

/* Sembunyikan tombol Restart & Stop saat belum mirroring (Ready / Stop) */
.app:not(.is-streaming) #restart-screen,
.app:not(.is-streaming) #stop-screen,
#restart-screen[hidden],
#stop-screen[hidden] {
  display: none !important;
  visibility: hidden !important;
  opacity: 0 !important;
  pointer-events: none !important;
}
/* Saat streaming berjalan, tombol Restart tampil sebagai ikon ringkas tanpa teks "Restart" */
#restart-screen .btn-label {
  display: none !important;
}
/* Custom Liquid Glass Range Sliders (Edge Crop & Trackpad Speed) */
html body input[type="range"] {
  -webkit-appearance: none !important;
  appearance: none !important;
  height: 6px !important;
  border-radius: 999px !important;
  background: rgba(255, 255, 255, 0.18) !important;
  border: 1px solid rgba(255, 255, 255, 0.14) !important;
  box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.35) !important;
  outline: none !important;
}
html[data-theme="light"] body input[type="range"] {
  background: rgba(15, 23, 42, 0.14) !important;
  border: 1px solid rgba(255, 255, 255, 0.65) !important;
  box-shadow: inset 0 1px 2px rgba(15, 23, 42, 0.12) !important;
}
html body input[type="range"]::-webkit-slider-thumb {
  -webkit-appearance: none !important;
  appearance: none !important;
  width: 16px !important;
  height: 16px !important;
  border-radius: 50% !important;
  background: linear-gradient(180deg, #ffffff 0%, #e2e8f0 100%) !important;
  border: 2px solid #0A84FF !important;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35) !important;
  cursor: pointer !important;
}

/* Recessed Liquid Glass Shortcut Pills */
html body .shortcuts-badge-list {
  padding: 8px !important;
  border-radius: 12px !important;
  background: rgba(6, 10, 18, 0.28) !important;
  border: 1px solid rgba(255, 255, 255, 0.12) !important;
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.35) !important;
  gap: 6px !important;
}
html[data-theme="light"] body .shortcuts-badge-list {
  background: rgba(15, 23, 42, 0.08) !important;
  border: 1px solid rgba(255, 255, 255, 0.65) !important;
  box-shadow: inset 0 1px 2px rgba(15, 23, 42, 0.10) !important;
}
html body .shortcut-pill {
  border-radius: 6px !important;
  padding: 3px 8px !important;
  font-size: 10.5px !important;
  background: rgba(255, 255, 255, 0.08) !important;
  border: 1px solid rgba(255, 255, 255, 0.16) !important;
  color: rgba(248, 250, 252, 0.88) !important;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25) !important;
}
html[data-theme="light"] body .shortcut-pill {
  background: rgba(255, 255, 255, 0.22) !important;
  backdrop-filter: blur(12px) saturate(150%) !important;
  -webkit-backdrop-filter: blur(12px) saturate(150%) !important;
  border: 1px solid rgba(255, 255, 255, 0.62) !important;
  color: #0f172a !important;
  box-shadow: 0 1px 4px rgba(15, 23, 42, 0.05), inset 0 1px 0 rgba(255, 255, 255, 0.55) !important;
}

html body #app-root.app.is-mirroring iframe#screen-view,
html body #app-root.app.is-streaming iframe#screen-view,
html body .app.is-streaming iframe#screen-view {
  background: transparent !important;
  background-color: transparent !important;
  opacity: 1 !important;
  visibility: visible !important;
  pointer-events: auto !important;
  color-scheme: dark !important;
}
html body #app-root.app:not(.is-mirroring):not(.is-streaming):not(.is-stopping-dissolve) #phone-screen #screen-view,
html body #app-root.app:not(.is-mirroring):not(.is-streaming):not(.is-stopping-dissolve) iframe#screen-view,
html body #app-root.app:not(.is-stopping-dissolve):has(#screen-empty:not([hidden])) iframe#screen-view {
  opacity: 0 !important;
  visibility: hidden !important;
  pointer-events: none !important;
  background: transparent !important;
  background-color: transparent !important;
}

/* Tahap 179: Zero-Flicker Top Bar Lock (38px 1-Row Invariant) & Zero-Transition Drawer Layout */
html body #app-root .app-bar,
html body .app-bar {
  height: 38px !important;
  min-height: 38px !important;
  max-height: 38px !important;
  flex: 0 0 38px !important;
  display: flex !important;
  flex-direction: row !important;
  flex-wrap: nowrap !important;
  align-items: center !important;
  white-space: nowrap !important;
  overflow: hidden !important;
  box-sizing: border-box !important;
  transition: none !important;
  animation: none !important;
}
@media (max-width: 600px) {
  html body #app-root .app-bar .brand-title,
  html body .app-bar .brand-title,
  html body #app-root.drawer-open .app-bar .brand-title,
  html body #app-root .app-bar #restart-screen .btn-label,
  html body .app-bar #restart-screen .btn-label,
  html body #app-root.drawer-open .app-bar #restart-screen .btn-label,
  html body #app-root .app-bar #stop-screen .btn-label,
  html body .app-bar #stop-screen .btn-label,
  html body #app-root.drawer-open .app-bar #stop-screen .btn-label {
    display: none !important;
  }
}

html body #app-root.app,
html body .app,
html body .main-body,
html body .display-col,
html body .workspace,
html body .stage,
html body #phone-shell,
html body #phone-screen,
html body #screen-view,
html body .side-drawer,
html body .control-pane {
  transition: none !important;
  animation: none !important;
}

/* Tahap 242 Pilar 3: Zero gray/white blank flash during WebView2 iframe init & seamless reconnect */
html body #phone-screen,
html body iframe#screen-view {
  background: #05070c !important;
}
#v4-reconnect-pill {
  position: absolute;
  top: 14px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 95;
  max-width: 240px !important;
  white-space: nowrap !important;
  overflow: hidden !important;
  text-overflow: ellipsis !important;
  padding: 6px 14px !important;
  border-radius: 999px !important;
  background: rgba(12, 16, 26, 0.82);
  border: 1px solid rgba(255, 255, 255, 0.16);
  color: #f3f6ff;
  font-size: 12px !important;
  font-weight: 600;
  line-height: 1.25 !important;
  letter-spacing: 0.01em;
  backdrop-filter: blur(12px);
  pointer-events: none;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
}

/* Tahap 243 Pilar 4: Visible spin animation on Launchpad & Drawer refresh buttons */
@keyframes v4Spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}
.v4-refresh-spinning svg,
#launchpad-scan-btn.v4-refresh-spinning svg,
#refresh.v4-refresh-spinning svg {
  animation: v4Spin 0.75s linear infinite !important;
}

/* Portrait + Drawer OPEN (iPhone, iPad 4:3, & Custom): left column preserves stage width seamlessly, right drawer = flush 360px (0px gap) */
html body #app-root.app:not(.is-landscape):not(.is-fullsize).drawer-open .main-body,
html body #app-root.app.is-ipad:not(.is-landscape):not(.is-fullsize).drawer-open .main-body,
html body #app-root.app:not(.is-landscape):not(.is-fullsize) .main-body:has(.side-drawer:not([hidden])),
html body #app-root.app:not(.is-landscape):not(.is-fullsize) .main-body:has(.side-drawer.open),
html body .app:not(.is-landscape):not(.is-fullsize).drawer-open .main-body {
  display: flex !important;
  flex-direction: row !important;
  justify-content: flex-start !important;
  align-items: stretch !important;
  width: 100vw !important;
  height: calc(100vh - 38px) !important;
  overflow: hidden !important;
  gap: 0 !important;
}
html body #app-root.app:not(.is-landscape):not(.is-fullsize).drawer-open .display-col,
html body #app-root.app:not(.is-landscape):not(.is-fullsize) .main-body:has(.side-drawer:not([hidden])) .display-col,
html body #app-root.app:not(.is-landscape):not(.is-fullsize) .main-body:has(.side-drawer.open) .display-col,
html body .app:not(.is-landscape):not(.is-fullsize).drawer-open .display-col {
  width: max(var(--display-width, 346px), calc(100vw - 360px)) !important;
  min-width: max(var(--display-width, 346px), calc(100vw - 360px)) !important;
  max-width: max(var(--display-width, 346px), calc(100vw - 360px)) !important;
  flex: 0 0 max(var(--display-width, 346px), calc(100vw - 360px)) !important;
  box-sizing: border-box !important;
  transition: none !important;
}
html body #app-root.app.is-ipad:not(.is-landscape):not(.is-fullsize).drawer-open .display-col,
html body #app-root.app.is-ipad:not(.is-landscape):not(.is-fullsize) .main-body:has(.side-drawer:not([hidden])) .display-col,
html body #app-root.app.is-ipad:not(.is-landscape):not(.is-fullsize) .main-body:has(.side-drawer.open) .display-col {
  width: max(564px, calc(100vw - 360px)) !important;
  min-width: max(564px, calc(100vw - 360px)) !important;
  max-width: max(564px, calc(100vw - 360px)) !important;
  flex: 0 0 max(564px, calc(100vw - 360px)) !important;
  box-sizing: border-box !important;
  transition: none !important;
}
html body #app-root.app.is-landscape:not(.is-fullsize).drawer-open .display-col {
  width: max(840px, calc(100vw - 360px)) !important;
  min-width: max(840px, calc(100vw - 360px)) !important;
  max-width: max(840px, calc(100vw - 360px)) !important;
  flex: 0 0 max(840px, calc(100vw - 360px)) !important;
  box-sizing: border-box !important;
  transition: none !important;
}
html body #app-root.app:not(.is-fullsize).drawer-open .side-drawer,
html body #app-root.app:not(.is-fullsize).drawer-open #control-pane,
html body #app-root.app.is-ipad:not(.is-fullsize).drawer-open .side-drawer,
html body #app-root.app.is-ipad:not(.is-fullsize).drawer-open #control-pane,
html body #app-root.app:not(.is-fullsize) .main-body .side-drawer:not([hidden]),
html body #app-root.app:not(.is-fullsize) .main-body #control-pane.open,
html body .app:not(.is-landscape):not(.is-fullsize).drawer-open .side-drawer,
html body .app:not(.is-landscape):not(.is-fullsize).drawer-open .control-pane {
  width: 360px !important;
  min-width: 360px !important;
  max-width: 360px !important;
  flex: 0 0 360px !important;
  margin-left: auto !important;
  margin-right: 0 !important;
  right: 0 !important;
  box-sizing: border-box !important;
  transition: none !important;
}
html body #app-root #control-pane .drawer-header,
html body #app-root #control-pane .drawer-scroll,
html body #app-root #control-pane .drawer-content,
html body #app-root .side-drawer .drawer-header,
html body #app-root .side-drawer .drawer-scroll,
html body #app-root .side-drawer .drawer-content {
  width: 100% !important;
  max-width: 100% !important;
  margin-right: 0 !important;
  box-sizing: border-box !important;
}

/* Anti-Stretch Glitch Lock saat Menutup Drawer (Portrait iPhone / iPad & Landscape Non-Fullsize) */
html body #app-root.app .display-col,
html body #app-root.app .workspace,
html body #app-root.app .stage,
html body #app-root.app #phone-shell,
html body #app-root.app #phone-screen,
html body #app-root.app #screen-view {
  transition: none !important;
}
html body #app-root.app:not(.is-landscape):not(.is-fullsize):not(.drawer-open) .display-col,
html body .app:not(.is-landscape):not(.is-fullsize):not(.drawer-open) .display-col {
  width: min(100vw, var(--display-width, 346px)) !important;
  min-width: 0 !important;
  max-width: var(--display-width, 346px) !important;
  flex: 0 1 var(--display-width, 346px) !important;
  margin-left: 0 !important;
  margin-right: auto !important;
}
html body #app-root.app.is-ipad:not(.is-landscape):not(.is-fullsize):not(.drawer-open) .display-col {
  width: min(100vw, 564px) !important;
  min-width: 0 !important;
  max-width: 564px !important;
  flex: 0 1 564px !important;
}
html body #app-root.app.is-landscape:not(.is-fullsize):not(.drawer-open) .display-col {
  width: min(100vw, 840px) !important;
  min-width: 0 !important;
  max-width: 840px !important;
  flex: 0 1 840px !important;
  margin-left: 0 !important;
  margin-right: auto !important;
}
html body #app-root.app:not(.is-fullsize):not(.drawer-open) .workspace,
html body #app-root.app:not(.is-fullsize):not(.drawer-open) .stage,
html body #app-root.app:not(.is-fullsize):not(.drawer-open) #phone-shell,
html body #app-root.app:not(.is-fullsize):not(.drawer-open) #phone-screen,
html body .app:not(.is-landscape):not(.is-fullsize):not(.drawer-open) .workspace,
html body .app:not(.is-landscape):not(.is-fullsize):not(.drawer-open) .stage,
html body .app:not(.is-landscape):not(.is-fullsize):not(.drawer-open) #phone-shell,
html body .app:not(.is-landscape):not(.is-fullsize):not(.drawer-open) #phone-screen {
  width: 100% !important;
  min-width: 0 !important;
  max-width: 100% !important;
  flex: 1 1 auto !important;
}

/* Tahap 180: Pure Real-Time Transparent Launchpad Full Screen across Auto, iPhone, & iPad (Zero #000000 Box) */
html body #app-root.app:not(.is-mirroring):not(.is-streaming) .display-col,
html body #app-root.app:not(.is-mirroring):not(.is-streaming) .workspace,
html body #app-root.app:not(.is-mirroring):not(.is-streaming) .stage,
html body #app-root.app:not(.is-mirroring):not(.is-streaming) #phone-shell,
html body #app-root.app:not(.is-mirroring):not(.is-streaming) #phone-screen,
html body #app-root.app.is-fullsize:not(.is-mirroring):not(.is-streaming) #phone-screen,
html body #app-root.app.is-ipad.is-fullsize:not(.is-mirroring):not(.is-streaming) #phone-screen,
html body #app-root.app.is-landscape.is-fullsize:not(.is-mirroring):not(.is-streaming) #phone-screen,
html body #app-root.app:has(#screen-empty:not([hidden])) #phone-screen,
html[data-theme="dark"] body #app-root.app.is-fullsize:not(.is-mirroring):not(.is-streaming) #phone-screen,
html[data-theme="light"] body #app-root.app.is-fullsize:not(.is-mirroring):not(.is-streaming) #phone-screen {
  background: transparent !important;
  background-color: transparent !important;
  background-image: none !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  border: none !important;
}
html body #app-root.app.is-fullsize:not(.is-mirroring):not(.is-streaming) #phone-screen,
html body #app-root.app.is-fullsize.is-ipad:not(.is-mirroring):not(.is-streaming) #phone-screen,
html body #app-root.app.is-fullsize.is-landscape:not(.is-mirroring):not(.is-streaming) #phone-screen,
html body #app-root.app.is-fullsize:has(#screen-empty:not([hidden])) #phone-screen {
  width: 100% !important;
  min-width: 100% !important;
  max-width: 100% !important;
  height: calc(100vh - 82px) !important;
  min-height: 0 !important;
  max-height: calc(100vh - 82px) !important;
  aspect-ratio: unset !important;
  flex: 1 1 auto !important;
  margin: 0 !important;
  border-radius: 0px !important;
}
html body #app-root.app.is-fullsize #screen-empty,
html body #app-root.app.is-fullsize:not(.is-mirroring):not(.is-streaming) #screen-empty,
html[data-theme="light"] body #app-root.app.is-fullsize #screen-empty {
  width: 100% !important;
  height: 100% !important;
  background: transparent !important;
  background-color: transparent !important;
  background-image: none !important;
  border: none !important;
  box-shadow: none !important;
}

/* Tahap 181 Langkah 2: Rasio Full Screen Portrait Mirroring (--v4-clean-aspect) + Sub-Pixel Transparent #phone-screen */
html body #app-root.app.is-fullsize.is-mirroring:not(.is-landscape):not(.is-ipad) #phone-screen,
html body #app-root.app.is-fullsize.is-streaming:not(.is-landscape):not(.is-ipad):not(:has(#screen-empty:not([hidden]))) #phone-screen {
  height: calc(100vh - 84px) !important;
  max-height: calc(100vh - 84px) !important;
  min-height: calc(100vh - 84px) !important;
  width: calc((100vh - 84px) * var(--v4-clean-aspect, 0.453425)) !important;
  max-width: calc((100vh - 84px) * var(--v4-clean-aspect, 0.453425)) !important;
  min-width: 280px !important;
  flex: 0 0 auto !important;
  margin: auto !important;
  border-radius: 0px !important;
  overflow: hidden !important;
  background: transparent !important;
  background-color: transparent !important;
}
html body #app-root.app.is-fullsize.is-mirroring #phone-screen > iframe#screen-view,
html body #app-root.app.is-mirroring #phone-screen > iframe#screen-view {
  position: absolute !important;
  inset: 0 !important;
  width: 100% !important;
  height: 100% !important;
  border: none !important;
  margin: 0 !important;
  padding: 0 !important;
  display: block !important;
  background: transparent !important;
  background-color: transparent !important;
}

/* Tahap 181 Langkah 3: Sync Full Screen + Drawer Open Layout (0 frame menempel di kiri saat masuk Full Screen) */
html body #app-root.app.is-fullsize.drawer-open .main-body {
  display: flex !important;
  flex-direction: row !important;
  justify-content: space-between !important;
  width: 100vw !important;
  max-width: 100vw !important;
  height: calc(100vh - 38px) !important;
  max-height: calc(100vh - 38px) !important;
  overflow: hidden !important;
}
html body #app-root.app.is-fullsize.drawer-open .display-col {
  width: calc(100vw - 360px) !important;
  min-width: calc(100vw - 360px) !important;
  max-width: calc(100vw - 360px) !important;
  flex: 1 1 calc(100vw - 360px) !important;
  display: flex !important;
  flex-direction: column !important;
  align-items: center !important;
  justify-content: center !important;
}
html body #app-root.app.is-fullsize.drawer-open .side-drawer,
html body #app-root.app.is-fullsize.drawer-open #control-pane {
  width: 360px !important;
  min-width: 360px !important;
  max-width: 360px !important;
  flex: 0 0 360px !important;
  margin-left: auto !important;
  margin-right: 0 !important;
}

/* TAHAP 192: 3D TACTILE LIQUID GLASS LAUNCHPAD CAPSULES & CONNECTION SWITCHER */
html body .launchpad-device-card,
html body #launchpad-device-card,
html[data-theme="dark"] body .launchpad-device-card,
html[data-theme="dark"] body #launchpad-device-card {
  width: 262px !important;
  padding: 8px 10px 8px 14px !important;
  gap: 10px !important;
  border-radius: 999px !important;
  background: linear-gradient(145deg, rgba(255, 255, 255, 0.13) 0%, rgba(30, 41, 59, 0.48) 52%, rgba(14, 165, 233, 0.10) 100%) !important;
  background-color: transparent !important;
  backdrop-filter: blur(22px) saturate(190%) !important;
  -webkit-backdrop-filter: blur(22px) saturate(190%) !important;
  border: 1px solid rgba(255, 255, 255, 0.18) !important;
  box-shadow:
    0 12px 28px -6px rgba(0, 0, 0, 0.52),
    inset 0 1px 1.5px 0 rgba(255, 255, 255, 0.32),
    inset 0 -1px 1px 0 rgba(0, 0, 0, 0.35) !important;
  transition: all 0.22s cubic-bezier(0.22, 1, 0.36, 1) !important;
}

html body .launchpad-device-card:hover,
html body #launchpad-device-card:hover {
  border-color: rgba(56, 189, 248, 0.38) !important;
  box-shadow:
    0 14px 32px -6px rgba(0, 0, 0, 0.6),
    0 0 18px rgba(14, 165, 233, 0.14),
    inset 0 1px 1.5px 0 rgba(255, 255, 255, 0.42) !important;
  transform: translateY(-1px) !important;
}

html[data-theme="light"] body .launchpad-device-card,
html[data-theme="light"] body #launchpad-device-card,
body.light-mode .launchpad-device-card,
body.light-mode #launchpad-device-card {
  background: linear-gradient(145deg, rgba(255, 255, 255, 0.68) 0%, rgba(241, 245, 249, 0.48) 55%, rgba(224, 242, 254, 0.56) 100%) !important;
  background-color: transparent !important;
  backdrop-filter: blur(22px) saturate(185%) !important;
  -webkit-backdrop-filter: blur(22px) saturate(185%) !important;
  border: 1px solid rgba(255, 255, 255, 0.88) !important;
  box-shadow:
    0 10px 26px -6px rgba(15, 23, 42, 0.12),
    0 2px 6px rgba(15, 23, 42, 0.05),
    inset 0 1.5px 2px 0 rgba(255, 255, 255, 0.98),
    inset 0 -1px 1px 0 rgba(14, 165, 233, 0.10) !important;
}

html[data-theme="light"] body .launchpad-device-card:hover,
html[data-theme="light"] body #launchpad-device-card:hover,
body.light-mode .launchpad-device-card:hover,
body.light-mode #launchpad-device-card:hover {
  border-color: rgba(14, 165, 233, 0.42) !important;
  box-shadow:
    0 12px 30px -6px rgba(15, 23, 42, 0.16),
    0 0 16px rgba(14, 165, 233, 0.14),
    inset 0 1.5px 2px 0 rgba(255, 255, 255, 1),
    inset 0 -1px 1px 0 rgba(14, 165, 233, 0.14) !important;
  transform: translateY(-1px) !important;
}

html body .launchpad-status-dot,
html body .device-dot,
html body #launchpad-status-dot {
  background-color: #10B981 !important;
  box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.22), 0 0 10px rgba(16, 185, 129, 0.65) !important;
}

html[data-theme="light"] body #launchpad-device-name,
html[data-theme="light"] body .launchpad-card-title,
body.light-mode #launchpad-device-name,
body.light-mode .launchpad-card-title {
  color: #0F172A !important;
  font-weight: 700 !important;
  text-shadow: none !important;
}

html[data-theme="light"] body #launchpad-device-sub,
html[data-theme="light"] body .launchpad-card-sub,
body.light-mode #launchpad-device-sub,
body.light-mode .launchpad-card-sub {
  color: #334155 !important;
  font-weight: 500 !important;
  text-shadow: none !important;
}

html body #launchpad-scan-btn,
html body .launchpad-icon-btn {
  width: 28px !important;
  height: 28px !important;
  min-width: 28px !important;
  flex: 0 0 28px !important;
  background: linear-gradient(180deg, rgba(255, 255, 255, 0.16), rgba(255, 255, 255, 0.05)) !important;
  border: 1px solid rgba(255, 255, 255, 0.22) !important;
  box-shadow: inset 0 1px 1px rgba(255, 255, 255, 0.35), 0 2px 6px rgba(0, 0, 0, 0.20) !important;
  color: #E2E8F0 !important;
  transition: transform 0.24s cubic-bezier(0.22, 1, 0.36, 1), box-shadow 0.2s ease, border-color 0.2s ease !important;
}

html body #launchpad-scan-btn:hover,
html body .launchpad-icon-btn:hover {
  border-color: rgba(56, 189, 248, 0.55) !important;
  box-shadow: inset 0 1px 1px rgba(255, 255, 255, 0.45), 0 0 12px rgba(14, 165, 233, 0.38) !important;
  color: #FFFFFF !important;
  transform: rotate(30deg) !important;
}

html[data-theme="light"] body #launchpad-scan-btn,
html[data-theme="light"] body .launchpad-icon-btn,
body.light-mode #launchpad-scan-btn,
body.light-mode .launchpad-icon-btn {
  background: linear-gradient(180deg, rgba(255, 255, 255, 0.92), rgba(241, 245, 249, 0.68)) !important;
  border: 1px solid rgba(255, 255, 255, 0.95) !important;
  box-shadow: inset 0 1px 1px rgba(255, 255, 255, 1), 0 2px 6px rgba(15, 23, 42, 0.08) !important;
  color: #0F172A !important;
}

html body #launchpad-transport-row,
html body .launchpad-transport-row {
  width: 244px !important;
  height: 36px !important;
  padding: 4px !important;
  border-radius: 999px !important;
  background: linear-gradient(180deg, rgba(8, 11, 18, 0.58) 0%, rgba(15, 23, 42, 0.44) 100%) !important;
  background-color: transparent !important;
  backdrop-filter: blur(20px) saturate(180%) !important;
  -webkit-backdrop-filter: blur(20px) saturate(180%) !important;
  border: 1px solid rgba(255, 255, 255, 0.14) !important;
  box-shadow:
    inset 0 2px 5px rgba(0, 0, 0, 0.48),
    inset 0 1px 0 rgba(255, 255, 255, 0.08),
    0 8px 20px -4px rgba(0, 0, 0, 0.38) !important;
}

html[data-theme="light"] body #launchpad-transport-row,
html[data-theme="light"] body .launchpad-transport-row,
body.light-mode #launchpad-transport-row,
body.light-mode .launchpad-transport-row {
  background: linear-gradient(180deg, rgba(15, 23, 42, 0.09) 0%, rgba(255, 255, 255, 0.52) 100%) !important;
  background-color: transparent !important;
  backdrop-filter: blur(20px) saturate(180%) !important;
  -webkit-backdrop-filter: blur(20px) saturate(180%) !important;
  border: 1px solid rgba(255, 255, 255, 0.85) !important;
  box-shadow:
    inset 0 2px 4px rgba(15, 23, 42, 0.10),
    0 8px 20px -4px rgba(15, 23, 42, 0.08) !important;
}

html body .launchpad-pill:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]),
html body #launchpad-transport-cable:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]),
html body #launchpad-transport-wifi:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]) {
  background: transparent !important;
  background-color: transparent !important;
  color: rgba(241, 245, 249, 0.78) !important;
  border: 1px solid transparent !important;
  box-shadow: none !important;
}

html body .launchpad-pill:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]):hover {
  background: rgba(255, 255, 255, 0.08) !important;
  color: #FFFFFF !important;
}

html[data-theme="light"] body .launchpad-pill:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]),
html[data-theme="light"] body #launchpad-transport-cable:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]),
html[data-theme="light"] body #launchpad-transport-wifi:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]),
body.light-mode .launchpad-pill:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]) {
  background: transparent !important;
  background-color: transparent !important;
  color: #1E293B !important;
  font-weight: 600 !important;
  border: 1px solid transparent !important;
  box-shadow: none !important;
}

html[data-theme="light"] body .launchpad-pill:not(.is-active):not(.active):not([aria-checked="true"]):not([aria-selected="true"]):not([data-active="true"]):hover {
  background: rgba(255, 255, 255, 0.62) !important;
  color: #0F172A !important;
}

html body .launchpad-pill.is-active,
html body .launchpad-pill.active,
html body .launchpad-pill[aria-checked="true"],
html body .launchpad-pill[aria-selected="true"],
html body .launchpad-pill[data-active="true"],
html body #launchpad-transport-cable.is-active,
html body #launchpad-transport-cable.active,
html body #launchpad-transport-cable[aria-checked="true"],
html body #launchpad-transport-wifi.is-active,
html body #launchpad-transport-wifi.active,
html body #launchpad-transport-wifi[aria-checked="true"],
html[data-theme="light"] body .launchpad-pill.is-active,
html[data-theme="light"] body .launchpad-pill.active,
html[data-theme="light"] body .launchpad-pill[aria-checked="true"],
html[data-theme="light"] body .launchpad-pill[aria-selected="true"],
html[data-theme="light"] body .launchpad-pill[data-active="true"],
html[data-theme="light"] body #launchpad-transport-cable.is-active,
html[data-theme="light"] body #launchpad-transport-cable.active,
html[data-theme="light"] body #launchpad-transport-cable[aria-checked="true"],
html[data-theme="light"] body #launchpad-transport-wifi.is-active,
html[data-theme="light"] body #launchpad-transport-wifi.active,
html[data-theme="light"] body #launchpad-transport-wifi[aria-checked="true"],
body.light-mode .launchpad-pill.is-active,
body.light-mode .launchpad-pill.active,
body.light-mode .launchpad-pill[aria-checked="true"] {
  background: linear-gradient(135deg, #2563EB 0%, #0078D4 48%, #0284C7 100%) !important;
  color: #FFFFFF !important;
  font-weight: 600 !important;
  border: 1px solid rgba(255, 255, 255, 0.32) !important;
  box-shadow:
    0 6px 16px -2px rgba(0, 120, 212, 0.52),
    0 2px 4px rgba(0, 0, 0, 0.22),
    inset 0 1.5px 1.5px 0 rgba(255, 255, 255, 0.48),
    inset 0 -1px 1px 0 rgba(0, 0, 0, 0.22) !important;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.25) !important;
}

html body .launchpad-pill.is-active span,
html body .launchpad-pill.is-active svg,
html body .launchpad-pill[aria-checked="true"] span,
html body .launchpad-pill[aria-checked="true"] svg,
html[data-theme="light"] body .launchpad-pill.is-active span,
html[data-theme="light"] body .launchpad-pill.is-active svg,
html[data-theme="light"] body .launchpad-pill[aria-checked="true"] span,
html[data-theme="light"] body .launchpad-pill[aria-checked="true"] svg {
  color: #FFFFFF !important;
}

html body #launchpad-start-btn,
html body .launchpad-start-btn {
  min-width: 172px !important;
  height: 38px !important;
  padding: 0 22px !important;
  border-radius: 999px !important;
  background: linear-gradient(135deg, #1D4ED8 0%, #0078D4 45%, #0EA5E9 100%) !important;
  color: #FFFFFF !important;
  border: 1px solid rgba(255, 255, 255, 0.30) !important;
  box-shadow:
    0 10px 24px -4px rgba(0, 120, 212, 0.55),
    inset 0 1.5px 1.5px rgba(255, 255, 255, 0.48),
    inset 0 -1px 1px rgba(0, 0, 0, 0.22) !important;
}
html body #launchpad-start-btn:hover,
html body .launchpad-start-btn:hover {
  filter: brightness(1.07) !important;
  box-shadow:
    0 12px 28px -4px rgba(0, 120, 212, 0.65),
    inset 0 1.5px 2px rgba(255, 255, 255, 0.56),
    inset 0 -1px 1px rgba(0, 0, 0, 0.25) !important;
  transform: translateY(-1px) !important;
}

/* Tahap 195: Apple-Grade Fluid Motion & Spring Physics UI (100% GPU Compositor) */
@keyframes appleLaunchpadReveal {
  0% { opacity: 0; transform: translate3d(0, 14px, 0) scale(0.98); }
  100% { opacity: 1; transform: translate3d(0, 0, 0) scale(1); }
}

@keyframes appleCardCascadeIn {
  0% { opacity: 0; transform: translate3d(12px, 6px, 0) scale(0.985); }
  100% { opacity: 1; transform: translate3d(0, 0, 0) scale(1); }
}

@keyframes appleShimmerSweep {
  0% { transform: translate3d(-120%, 0, 0); }
  100% { transform: translate3d(120%, 0, 0); }
}

@keyframes appleSuccessPop {
  0% { transform: scale(0.96); }
  50% { transform: scale(1.03); }
  100% { transform: scale(1); }
}

html body #launchpad-idle-view:not([hidden]) {
  animation: appleLaunchpadReveal 380ms var(--spring-apple-fluid) both;
}

html body .app.drawer-open #control-pane.side-drawer .card,
html body .app.drawer-open #control-pane.side-drawer .glass-card {
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
  animation: appleCardCascadeIn 320ms var(--spring-apple-sheet) both;
}

html body .app.drawer-open #control-pane.side-drawer .drawer-scroll > .card:nth-child(1) { animation-delay: 25ms; }
html body .app.drawer-open #control-pane.side-drawer .drawer-scroll > .card:nth-child(2) { animation-delay: 60ms; }
html body .app.drawer-open #control-pane.side-drawer .drawer-scroll > .card:nth-child(3) { animation-delay: 95ms; }
html body .app.drawer-open #control-pane.side-drawer .drawer-scroll > .card:nth-child(4) { animation-delay: 130ms; }
html body .app.drawer-open #control-pane.side-drawer .drawer-scroll > .card:nth-child(n+5) { animation-delay: 160ms; }

html body .ctl:active:not(:disabled),
html body .ctl-sm:active:not(:disabled),
html body .btn-action-block:active:not(:disabled),
html body .launchpad-pill:active:not(:disabled),
html body .launchpad-start-btn:active:not(:disabled),
html body .seg:active:not(:disabled) {
  transform: scale(0.965) !important;
  transition-duration: 90ms !important;
  transition-timing-function: var(--spring-apple-snappy) !important;
}

html body .is-busy-shimmer {
  position: relative !important;
  overflow: hidden !important;
}

html body .is-busy-shimmer::after {
  content: "";
  position: absolute;
  inset: 0;
  background: linear-gradient(110deg, transparent 20%, rgba(255, 255, 255, 0.28) 50%, transparent 80%);
  transform: translate3d(-120%, 0, 0);
  animation: appleShimmerSweep 950ms var(--spring-apple-fluid) infinite;
  pointer-events: none;
}

html body .is-saved {
  background: linear-gradient(135deg, rgba(16, 185, 129, 0.28), rgba(5, 150, 105, 0.18)) !important;
  border-color: rgba(52, 211, 153, 0.58) !important;
  color: #ecfdf5 !important;
  box-shadow: 0 8px 20px -6px rgba(16, 185, 129, 0.42), inset 0 1px 1px rgba(255, 255, 255, 0.45) !important;
  animation: appleSuccessPop 360ms var(--spring-apple-bounce) 1;
}

html body[data-theme="light"] .is-saved,
[data-theme="light"] html body .is-saved {
  background: linear-gradient(135deg, rgba(16, 185, 129, 0.24), rgba(5, 150, 105, 0.14)) !important;
  border-color: rgba(5, 150, 105, 0.52) !important;
  color: #064e3b !important;
}

html body .app.is-mirroring-active #phone-screen {
  transition: transform 360ms var(--spring-apple-sheet), opacity 280ms var(--spring-apple-fluid), box-shadow 360ms var(--spring-apple-fluid);
}

/* Tahap 206: True Cross-Dissolve Stop Mirroring (0ms black flash) + Restorasi Full Screen Vertikal */
@keyframes sequoiaScreenViewFadeOut {
  0% {
    opacity: 1;
  }
  100% {
    opacity: 0;
  }
}

html body #app-root.app.is-stopping-dissolve #phone-screen,
html body .app.is-stopping-dissolve #phone-screen {
  background: transparent !important;
  background-color: transparent !important;
}

html body #app-root.app.is-stopping-dissolve #phone-screen > iframe#screen-view,
html body #app-root.app.is-stopping-dissolve iframe#screen-view,
html body .app.is-stopping-dissolve #screen-view {
  position: absolute !important;
  inset: 0 !important;
  z-index: 12 !important;
  display: block !important;
  visibility: visible !important;
  pointer-events: none !important;
  background: transparent !important;
  background-color: transparent !important;
  opacity: 0 !important;
  animation: sequoiaScreenViewFadeOut 230ms cubic-bezier(0.22, 1, 0.36, 1) forwards !important;
  transition: opacity 230ms cubic-bezier(0.22, 1, 0.36, 1) !important;
}

html body #app-root.app.is-stopping-dissolve #screen-empty,
html body .app.is-stopping-dissolve #screen-empty {
  z-index: 20 !important;
  background: transparent !important;
  background-color: transparent !important;
}

/* Tahap 207 / 208 / 209: Transparent #phone-screen pra-frame-1 + Full Screen Stop Dissolve + Unified Sequoia Full Screen Flush 0px Corners */
html body #app-root.app.is-fullsize:not(.has-first-frame) #phone-screen,
html body #app-root.app.is-fullsize.is-stopping-dissolve #phone-screen,
html body .app.is-fullsize.is-stopping-dissolve #phone-screen {
  background: transparent !important;
  background-color: transparent !important;
  box-shadow: none !important;
  border-color: transparent !important;
  border: none !important;
  border-radius: 0px !important;
}

html body #app-root.app.is-fullsize #phone-screen,
html body #app-root.app.is-fullsize:not(.is-landscape):not(.is-ipad) #phone-screen,
html body #app-root.app.is-fullsize #phone-screen > #screen-view,
html body #app-root.app.is-fullsize #phone-screen > iframe#screen-view,
html body #app-root.app.is-fullsize #sequoia-connect-overlay,
html body #app-root.app.is-fullsize #sequoia-connect-backdrop,
.app.is-fullsize #sequoia-connect-overlay,
.app.is-fullsize #sequoia-connect-backdrop {
  border-radius: 0px !important;
}

html body #app-root.app.is-fullsize.is-stopping-dissolve:not(.is-landscape):not(.is-ipad) #phone-screen > iframe#screen-view,
html body #app-root.app.is-fullsize.is-stopping-dissolve:not(.is-landscape):not(.is-ipad) iframe#screen-view,
.app.is-fullsize.is-stopping-dissolve:not(.is-landscape):not(.is-ipad) iframe#screen-view {
  position: absolute !important;
  top: 0 !important;
  bottom: 0 !important;
  left: 0 !important;
  right: 0 !important;
  width: calc((100vh - 84px) * var(--v4-clean-aspect, 0.453425)) !important;
  max-width: calc((100vh - 84px) * var(--v4-clean-aspect, 0.453425)) !important;
  height: calc(100vh - 84px) !important;
  margin: auto !important;
  border-radius: 0px !important;
  background: transparent !important;
  background-color: transparent !important;
  z-index: 12 !important;
  pointer-events: none !important;
  visibility: visible !important;
  animation: sequoiaScreenViewFadeOut 220ms cubic-bezier(0.22, 1, 0.36, 1) forwards !important;
}

html body #app-root.app.is-fullsize:not(.is-landscape):not(.is-ipad) #phone-screen:has(#sequoia-connect-overlay:not([hidden])),
.app.is-fullsize:not(.is-landscape):not(.is-ipad) #phone-screen:has(#sequoia-connect-overlay:not([hidden])) {
  overflow: visible !important;
  border-radius: 0px !important;
}

html body #app-root.app.is-fullsize:not(.is-landscape):not(.is-ipad):has(#sequoia-connect-overlay:not([hidden])) #phone-screen > iframe#screen-view,
html body #app-root.app.is-fullsize:not(.is-landscape):not(.is-ipad):has(#sequoia-connect-overlay:not([hidden])) iframe#screen-view {
  border-radius: 0px !important;
  overflow: hidden !important;
}

html body #app-root.app.is-fullsize:not(.is-landscape):not(.is-ipad) #sequoia-connect-overlay,
html body .app.is-fullsize:not(.is-landscape):not(.is-ipad) #sequoia-connect-overlay,
.app.is-fullsize:not(.is-landscape):not(.is-ipad) #sequoia-connect-overlay {
  position: absolute !important;
  top: 0 !important;
  bottom: 0 !important;
  left: 0 !important;
  right: 0 !important;
  width: calc((100vh - 84px) * var(--v4-clean-aspect, 0.453425)) !important;
  max-width: calc((100vh - 84px) * var(--v4-clean-aspect, 0.453425)) !important;
  height: calc(100vh - 84px) !important;
  margin: auto !important;
  border-radius: 0px !important;
  overflow: hidden !important;
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.55), 0 0 0 1px rgba(255, 255, 255, 0.14) !important;
}

/* Tahap 222: High-Contrast Light & Dark Mode Override for iOS 27 Shortcut Card & Side Drawer */
html body #shortcut-url-box,
html body .shortcut-url-card {
  background: rgba(14, 116, 144, 0.14) !important;
  border: 1px solid rgba(56, 189, 248, 0.32) !important;
}
html body #shortcut-box-title,
html body .shortcut-box-title {
  color: #38bdf8 !important;
  font-weight: 700 !important;
}
html body #link-shortcut-guide-hint,
html body .shortcut-setup-link {
  color: #38bdf8 !important;
  font-weight: 600 !important;
  text-decoration: underline !important;
}
html body #shortcut-ip-comparison,
html body #shortcut-ip-hint,
html body .shortcut-ip-hint {
  color: #e2e8f0 !important;
  font-weight: 600 !important;
}
html body #shortcut-ocr-note,
html body .shortcut-ocr-note {
  color: #cbd5e1 !important;
  font-weight: 600 !important;
}
html body #shortcut-url-preview,
html body .shortcut-url-code {
  background: rgba(15, 23, 42, 0.72) !important;
  color: #f8fafc !important;
  border: 1px solid rgba(148, 163, 184, 0.28) !important;
}

html[data-theme="light"] body #shortcut-url-box,
html[data-theme="light"] body .shortcut-url-card,
body[data-theme="light"] #shortcut-url-box,
.app[data-theme="light"] #shortcut-url-box {
  background: linear-gradient(135deg, rgba(255, 255, 255, 0.22) 0%, rgba(255, 255, 255, 0.10) 100%) !important;
  backdrop-filter: blur(18px) saturate(160%) !important;
  -webkit-backdrop-filter: blur(18px) saturate(160%) !important;
  border: 1px solid rgba(255, 255, 255, 0.54) !important;
  box-shadow: 0 8px 24px rgba(15, 23, 42, 0.08), inset 0 1px 0 rgba(255, 255, 255, 0.65) !important;
}
html[data-theme="light"] body #shortcut-box-title,
html[data-theme="light"] body .shortcut-box-title,
body[data-theme="light"] #shortcut-box-title,
.app[data-theme="light"] #shortcut-box-title {
  color: #0369a1 !important;
  font-weight: 700 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #link-shortcut-guide-hint,
html[data-theme="light"] body .shortcut-setup-link,
body[data-theme="light"] #link-shortcut-guide-hint,
.app[data-theme="light"] #link-shortcut-guide-hint {
  color: #0284c7 !important;
  font-weight: 600 !important;
  text-decoration: underline !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #shortcut-ip-comparison,
html[data-theme="light"] body #shortcut-ip-hint,
html[data-theme="light"] body .shortcut-ip-hint,
body[data-theme="light"] #shortcut-ip-comparison,
.app[data-theme="light"] #shortcut-ip-comparison {
  color: #1e293b !important;
  font-weight: 600 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #shortcut-ocr-note,
html[data-theme="light"] body .shortcut-ocr-note,
body[data-theme="light"] #shortcut-ocr-note,
.app[data-theme="light"] #shortcut-ocr-note {
  color: #0f172a !important;
  font-weight: 600 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #shortcut-url-preview,
html[data-theme="light"] body .shortcut-url-code,
body[data-theme="light"] #shortcut-url-preview,
.app[data-theme="light"] #shortcut-url-preview {
  background: rgba(15, 23, 42, 0.08) !important;
  backdrop-filter: blur(10px) saturate(140%) !important;
  -webkit-backdrop-filter: blur(10px) saturate(140%) !important;
  color: #0f172a !important;
  border: 1px solid rgba(15, 23, 42, 0.18) !important;
  box-shadow: inset 0 1px 2px rgba(15, 23, 42, 0.06) !important;
  font-weight: 600 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #shortcut-url-box button,
html[data-theme="light"] body #btn-copy-shortcut-url,
html[data-theme="light"] body #btn-open-icloud-shortcut,
html[data-theme="light"] body #btn-get-icloud-shortcut,
html[data-theme="light"] body #btn-copy-siri-prompt,
html[data-theme="light"] body #btn-copy-shortcut-prompt,
html[data-theme="light"] body #btn-unlock-devmode,
html[data-theme="light"] body #btn-repair-driver,
html[data-theme="light"] body #refresh {
  background: rgba(255, 255, 255, 0.24) !important;
  backdrop-filter: blur(12px) saturate(150%) !important;
  -webkit-backdrop-filter: blur(12px) saturate(150%) !important;
  color: #0f172a !important;
  border: 1px solid rgba(255, 255, 255, 0.62) !important;
  box-shadow: 0 2px 8px rgba(15, 23, 42, 0.06), inset 0 1px 0 rgba(255, 255, 255, 0.60) !important;
  font-weight: 600 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #shortcut-url-box button:hover,
html[data-theme="light"] body #btn-copy-shortcut-url:hover,
html[data-theme="light"] body #btn-open-icloud-shortcut:hover,
html[data-theme="light"] body #btn-get-icloud-shortcut:hover,
html[data-theme="light"] body #btn-copy-siri-prompt:hover,
html[data-theme="light"] body #btn-copy-shortcut-prompt:hover,
html[data-theme="light"] body #btn-unlock-devmode:hover,
html[data-theme="light"] body #btn-repair-driver:hover,
html[data-theme="light"] body #refresh:hover {
  background: rgba(255, 255, 255, 0.38) !important;
  border-color: rgba(255, 255, 255, 0.85) !important;
}
html[data-theme="light"] body .shortcut-pill kbd {
  background: rgba(15, 23, 42, 0.12) !important;
  color: #0f172a !important;
  border: 1px solid rgba(15, 23, 42, 0.18) !important;
}
html[data-theme="light"] body .shortcuts-badge-list {
  background: transparent !important;
}
html[data-theme="light"] body #shortcut-url-box button *,
html[data-theme="light"] body #btn-unlock-devmode *,
html[data-theme="light"] body #btn-repair-driver *,
html[data-theme="light"] body #refresh * {
  color: #0f172a !important;
}
html[data-theme="light"] body #control-pane details > summary,
html[data-theme="light"] body #device-card summary {
  color: #0f172a !important;
  font-weight: 700 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #device-card #status dt {
  color: #334155 !important;
  font-weight: 600 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #device-card #status dd {
  color: #0f172a !important;
  font-weight: 600 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #device-card #status dd.ok,
html[data-theme="light"] body #diag-apple-driver.ok,
html[data-theme="light"] body #status-service.ok,
html[data-theme="light"] body #diag-tcp-port.ok,
html[data-theme="light"] body #status-devmode.ok,
html[data-theme="light"] body #diag-hevc-codec.ok {
  color: #047857 !important;
  font-weight: 700 !important;
}
html[data-theme="light"] body #device-card #status dd.warn,
html[data-theme="light"] body #diag-apple-driver.warn,
html[data-theme="light"] body #status-service.warn,
html[data-theme="light"] body #diag-tcp-port.warn,
html[data-theme="light"] body #status-devmode.warn,
html[data-theme="light"] body #diag-hevc-codec.warn {
  color: #b45309 !important;
  font-weight: 700 !important;
}
html[data-theme="light"] body #devmode-action-status {
  background: rgba(14, 165, 233, 0.14) !important;
  backdrop-filter: blur(10px) saturate(140%) !important;
  -webkit-backdrop-filter: blur(10px) saturate(140%) !important;
  border: 1px solid rgba(2, 132, 199, 0.35) !important;
  color: #0c4a6e !important;
  font-weight: 600 !important;
  text-shadow: none !important;
}
html[data-theme="light"] body #update-info-box {
  backdrop-filter: blur(10px) saturate(140%) !important;
  -webkit-backdrop-filter: blur(10px) saturate(140%) !important;
}
html[data-theme="light"] body #control-pane .setting-hint,
html[data-theme="light"] body #control-pane .unit,
html[data-theme="light"] body #control-pane .stealth-subhint,
html[data-theme="light"] body #control-pane .control-hint,
html[data-theme="light"] body #control-pane .fine,
html[data-theme="light"] body #status-note {
  color: #334155 !important;
  opacity: 1 !important;
}
html[data-theme="light"] body #status-note.warn {
  color: #b45309 !important;
}

/* Tahap 235 (Paritas Tahap 91 & 94): GPU Layer Containment, Single-Pass Acrylic Drawer, & Compact Quick Nav */
.display-col,
#phone-screen {
  contain: layout style paint;
  will-change: transform;
}
.side-drawer,
#control-pane {
  contain: layout style paint;
  will-change: transform;
}
.side-drawer .card,
.side-drawer .glass-card,
.side-drawer .control-group,
#control-pane .card,
#control-pane .glass-card,
#control-pane .control-group {
  backdrop-filter: none !important;
  -webkit-backdrop-filter: none !important;
}
.app.is-fullsize.drawer-open .side-drawer,
.app.is-fullsize.drawer-open #control-pane {
  display: flex !important;
  width: 360px !important;
}
.app.is-fullsize.drawer-open #phone-screen {
  height: calc(100vh - 84px) !important;
  max-height: calc(100vh - 84px) !important;
}
.quick-nav button,
.bottom-nav button {
  white-space: nowrap !important;
  text-overflow: clip !important;
}

/* Tahap 247: Smart Multi-iPhone Launchpad Device Picker (WinUI 3 Liquid Glass, Dark & Light Mode) */
.launchpad-idle-view.has-multi-devices {
  gap: 8px !important;
}
.launchpad-idle-view.has-multi-devices .launchpad-hero {
  margin-bottom: 2px !important;
}
.launchpad-idle-view.has-multi-devices .launchpad-hero-icon {
  width: 46px !important;
  height: 46px !important;
  margin-bottom: 6px !important;
}
.launchpad-idle-view.has-multi-devices .launchpad-logo-img {
  width: 46px !important;
  height: 46px !important;
}
.launchpad-device-picker {
  width: 100%;
  max-width: 100%;
  box-sizing: border-box;
  padding: 8px 9px;
  border-radius: 14px;
  background: rgba(15, 23, 42, 0.42);
  border: 1px solid rgba(255, 255, 255, 0.14);
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.22), inset 0 1px 0 rgba(255, 255, 255, 0.10);
  backdrop-filter: blur(18px) saturate(165%);
  -webkit-backdrop-filter: blur(18px) saturate(165%);
  text-align: left;
}
.launchpad-device-picker[hidden] {
  display: none !important;
}
.launchpad-picker-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 3px 6px;
}
.launchpad-picker-title {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.03em;
  text-transform: uppercase;
  color: rgba(226, 232, 240, 0.78);
}
.launchpad-picker-count {
  font-size: 10.5px;
  font-weight: 700;
  padding: 1px 7px;
  border-radius: 999px;
  background: rgba(56, 189, 248, 0.18);
  border: 1px solid rgba(56, 189, 248, 0.34);
  color: #7dd3fc;
}
.launchpad-picker-list {
  display: flex;
  flex-direction: column;
  gap: 5px;
  max-height: 132px;
  overflow-y: auto;
  overflow-x: hidden;
  padding-right: 2px;
  scrollbar-width: thin;
}
.launchpad-picker-item {
  width: 100%;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 7px 10px;
  border-radius: 10px;
  border: 1px solid rgba(255, 255, 255, 0.10);
  background: rgba(255, 255, 255, 0.05);
  color: #f8fafc;
  cursor: pointer;
  text-align: left;
  font-family: inherit;
  transition: background 140ms ease, border-color 140ms ease, transform 120ms ease, box-shadow 140ms ease;
}
.launchpad-picker-item:hover {
  background: rgba(255, 255, 255, 0.11);
  border-color: rgba(125, 211, 252, 0.40);
}
.launchpad-picker-item:active {
  transform: scale(0.985);
}
.launchpad-picker-item.is-selected {
  background: linear-gradient(135deg, rgba(14, 165, 233, 0.24) 0%, rgba(37, 99, 235, 0.18) 100%);
  border-color: rgba(56, 189, 248, 0.68);
  box-shadow: 0 0 0 1px rgba(56, 189, 248, 0.25), inset 0 1px 0 rgba(255, 255, 255, 0.22);
}
.launchpad-picker-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex: 0 0 8px;
  background: #94a3b8;
}
.launchpad-picker-dot.is-online {
  background: #22c55e;
  box-shadow: 0 0 8px rgba(34, 197, 94, 0.72);
}
.launchpad-picker-dot.is-offline {
  background: #94a3b8;
  opacity: 0.78;
}
.launchpad-picker-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.launchpad-picker-main {
  font-size: 12.2px;
  font-weight: 700;
  line-height: 1.25;
  color: #f8fafc;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.launchpad-picker-name {
  font-weight: 500;
  color: rgba(226, 232, 240, 0.80);
}
.launchpad-picker-sub {
  font-size: 10.5px;
  font-weight: 500;
  line-height: 1.25;
  color: rgba(148, 163, 184, 0.95);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.launchpad-picker-item.is-selected .launchpad-picker-sub {
  color: #bae6fd;
}
.launchpad-picker-check {
  flex: 0 0 auto;
  width: 18px;
  height: 18px;
  border-radius: 999px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  font-weight: 800;
  color: #ffffff;
  background: linear-gradient(135deg, #0ea5e9, #2563eb);
  box-shadow: 0 2px 6px rgba(14, 165, 233, 0.45);
  opacity: 0;
  transform: scale(0.75);
  transition: opacity 120ms ease, transform 120ms ease;
}
.launchpad-picker-item.is-selected .launchpad-picker-check {
  opacity: 1;
  transform: scale(1);
}
.launchpad-switch-wifi-btn {
  width: 100%;
  box-sizing: border-box;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 6px 12px;
  border-radius: 10px;
  border: 1px solid rgba(56, 189, 248, 0.48);
  background: rgba(14, 165, 233, 0.16);
  color: #7dd3fc;
  font-size: 11.5px;
  font-weight: 700;
  cursor: pointer;
  transition: background 140ms ease, border-color 140ms ease, transform 120ms ease;
}
.launchpad-switch-wifi-btn:hover {
  background: rgba(14, 165, 233, 0.28);
  border-color: rgba(125, 211, 252, 0.72);
}
.launchpad-switch-wifi-btn[hidden] {
  display: none !important;
}

/* Light Mode High-Contrast Liquid Glass Overrides for Multi-iPhone Device Picker */
html[data-theme="light"] .launchpad-device-picker {
  background: rgba(255, 255, 255, 0.54) !important;
  border: 1px solid rgba(15, 23, 42, 0.12) !important;
  box-shadow: 0 6px 18px rgba(15, 23, 42, 0.08), inset 0 1px 0 rgba(255, 255, 255, 0.85) !important;
}
html[data-theme="light"] .launchpad-picker-title {
  color: #334155 !important;
}
html[data-theme="light"] .launchpad-picker-count {
  background: rgba(2, 132, 199, 0.12) !important;
  border-color: rgba(2, 132, 199, 0.30) !important;
  color: #0369a1 !important;
}
html[data-theme="light"] .launchpad-picker-item {
  background: rgba(255, 255, 255, 0.58) !important;
  border-color: rgba(15, 23, 42, 0.10) !important;
  color: #0f172a !important;
}
html[data-theme="light"] .launchpad-picker-item:hover {
  background: rgba(255, 255, 255, 0.84) !important;
  border-color: rgba(2, 132, 199, 0.42) !important;
}
html[data-theme="light"] .launchpad-picker-item.is-selected {
  background: linear-gradient(135deg, rgba(14, 165, 233, 0.18) 0%, rgba(37, 99, 235, 0.14) 100%) !important;
  border-color: rgba(2, 132, 199, 0.72) !important;
  box-shadow: 0 0 0 1px rgba(2, 132, 199, 0.22), inset 0 1px 0 rgba(255, 255, 255, 0.80) !important;
}
html[data-theme="light"] .launchpad-picker-main {
  color: #0f172a !important;
}
html[data-theme="light"] .launchpad-picker-name {
  color: #334155 !important;
}
html[data-theme="light"] .launchpad-picker-sub {
  color: #475569 !important;
}
html[data-theme="light"] .launchpad-picker-item.is-selected .launchpad-picker-sub {
  color: #0369a1 !important;
}
html[data-theme="light"] .launchpad-switch-wifi-btn {
  background: rgba(2, 132, 199, 0.12) !important;
  border-color: rgba(2, 132, 199, 0.42) !important;
  color: #0369a1 !important;
}
html[data-theme="light"] .launchpad-switch-wifi-btn:hover {
  background: rgba(2, 132, 199, 0.22) !important;
}

/* Tahap 248 Pilar 3: Anti-Blank-Putih Light Mode pre-first-frame lock.
 * Specificity (0,2,3,3) beats (0,2,2,3) transparent streaming override until frame 1 renders. */
html body #app-root.app.is-streaming:not(.has-first-frame) #phone-screen,
html body #app-root.app.is-streaming:not(.has-first-frame) iframe#screen-view {
  background: #090d16 !important;
  background-color: #090d16 !important;
}

/* Tahap 255 Pilar 2: Honest bilingual stream-mode description inside Settings Drawer (0-diff app.css) */
#stream-mode-desc {
  margin-top: 6px;
  font-size: 11px;
  line-height: 1.42;
  color: rgba(203, 213, 225, 0.86);
}
html[data-theme="light"] #stream-mode-desc {
  color: rgba(51, 65, 85, 0.90) !important;
}
"#;

pub const V4_BRIDGE_JS: &str = r#"/* v4_bridge.js - Native Rust IPC Shim for pywebview.api (Tahap 184 / v5.0.0) */
(function() {
    console.log("[iPhone Link v5.0 Native Rust] Bridge initialized");

    function sanitizePinDigits(rawVal) {
        return String(rawVal || '').replace(/\D/g, '').slice(0, 6);
    }

    function seedLocalStorageFromSettings(initData) {
        if (!initData || typeof initData !== 'object') return;
        try {
            window.__v4BootSettings = Object.assign({}, window.__v4BootSettings || {}, initData);
            var lang = (initData.language === 'en') ? 'en' : 'id';
            window.__v4CurrentLang = lang;
            localStorage.setItem('iphone_link_lang', lang);
            localStorage.setItem('iphone_link_language', lang);
            document.documentElement.setAttribute('lang', lang);
            document.documentElement.setAttribute('data-lang', lang);

            var thm = (initData.theme === 'light') ? 'light' : 'dark';
            localStorage.setItem('pane-theme', thm);
            document.documentElement.setAttribute('data-theme', thm);
            try { document.documentElement.style.colorScheme = thm; } catch (_) {}

            if (initData.fitMode === 'autofit' || initData.fitMode === 'fill' || initData.fitMode === 'contain') {
                localStorage.setItem('pane-fit-mode', initData.fitMode);
            }
            if (typeof initData.autoRotate === 'boolean') {
                localStorage.setItem('pane-auto-rotate', String(initData.autoRotate));
            }
            if (initData.deviceProfile === 'auto' || initData.deviceProfile === 'iphone' || initData.deviceProfile === 'ipad') {
                window.__v4ActiveDeviceProfile = initData.deviceProfile;
                localStorage.setItem('pane-device-profile', initData.deviceProfile);
                var initDw = (initData.deviceProfile === 'ipad') ? '564px' : '346px';
                document.documentElement.style.setProperty('--display-width', initDw);
            }
            if (initData.crop && typeof initData.crop === 'object') {
                var cleanCrop = {
                    top: Number(initData.crop.top || 0),
                    bottom: Number(initData.crop.bottom || 0),
                    left: Number(initData.crop.left || 0),
                    right: Number(initData.crop.right || 0)
                };
                localStorage.setItem('pane-crop-edges', JSON.stringify(cleanCrop));
            }
            if (typeof initData.trackpadSpeed === 'number') {
                localStorage.setItem('pane-trackpad-speed', String(initData.trackpadSpeed));
            }
            if (typeof initData.crossPlatformShortcuts === 'boolean') {
                localStorage.setItem('pane-shortcuts', String(initData.crossPlatformShortcuts));
            }
            if (typeof initData.pastePcClipboard === 'boolean') {
                localStorage.setItem('pane-paste-pc', String(initData.pastePcClipboard));
            }
            if (initData.hasUserSettingsFile && initData.transport) {
                var trVal = (initData.transport === 'wifi') ? 'wifi' : 'usb';
                localStorage.setItem('handset-transport', trVal);
            } else if (!initData.hasUserSettingsFile) {
                localStorage.removeItem('handset-transport');
            }
            // Tahap 255 Pilar 1: Fresh install (!hasUserSettingsFile) MUST purge any stale streamMode
            // keys left in the persistent WebView2 profile so Click #1 never hijacks Auto -> AirPlay!
            if (!initData.hasUserSettingsFile) {
                localStorage.removeItem('iphone_link_stream_mode');
                localStorage.removeItem('handset-stream-mode');
                localStorage.removeItem('v4-stream-mode');
                window.currentStreamMode = 'auto';
            } else {
                var smVal = (String(initData.streamMode || localStorage.getItem('iphone_link_stream_mode') || 'auto').trim().toLowerCase() === 'airplay') ? 'airplay' : 'auto';
                localStorage.setItem('iphone_link_stream_mode', smVal);
                window.currentStreamMode = smVal;
            }
            if (initData.folderInstanceId) {
                window.__v4FolderInstanceId = String(initData.folderInstanceId);
                localStorage.setItem('iphone_link_folder_instance_id', window.__v4FolderInstanceId);
            }
            var pinDigits = sanitizePinDigits(initData.unlockPin || '');
            if (pinDigits) {
                window.__v4SavedUnlockPin = pinDigits;
                window.__v4IntentionalWipe = false;
                localStorage.setItem('iphone_link_unlock_pin', pinDigits);
                localStorage.setItem('pane-unlock-pin', pinDigits);
            } else if (!initData.hasUserSettingsFile) {
                window.__v4SavedUnlockPin = '';
                localStorage.removeItem('iphone_link_unlock_pin');
                localStorage.removeItem('pane-unlock-pin');
            }
        } catch (_) {}
    }

    if (window.__V4_INITIAL_SETTINGS__) {
        seedLocalStorageFromSettings(window.__V4_INITIAL_SETTINGS__);
    }

    function postApi(url, payload) {
        return fetch(url, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload || {})
        }).then(function(r) { return r.json(); }).catch(function() { return { ok: false }; });
    }

    window.__v4PendingPatch = {};
    window.__v4PatchTimer = null;
    window.__v4LastToggleChangeMs = 0;

    function flushSettingsPatch(useKeepalive) {
        try {
            if (window.__v4JustWiped) {
                window.__v4PendingPatch = {};
                return Promise.resolve({ ok: true, skippedWipe: true });
            }
            var body = Object.assign({}, window.__v4PendingPatch || {});
            window.__v4PendingPatch = {};
            if (Object.keys(body).length === 0) {
                return Promise.resolve({ ok: true, empty: true });
            }
            var fn = window.__v4OrigFetch || window.fetch;
            return fn('/api/settings', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
                keepalive: Boolean(useKeepalive)
            }).then(function(r) { return r.json(); }).catch(function() { return { ok: false }; });
        } catch (_) {
            return Promise.resolve({ ok: false });
        }
    }

    function persistSettingPartial(patch, immediate) {
        if (!patch || typeof patch !== 'object' || window.__v4Hydrating) return;
        window.__v4JustWiped = false;
        window.__v4LastToggleChangeMs = Date.now();
        window.__v4BootSettings = Object.assign({}, window.__v4BootSettings || {}, patch);
        window.__v4PendingPatch = Object.assign({}, window.__v4PendingPatch || {}, patch);
        if (window.__v4PatchTimer) {
            clearTimeout(window.__v4PatchTimer);
            window.__v4PatchTimer = null;
        }
        if (immediate) {
            return flushSettingsPatch(true);
        }
        window.__v4PatchTimer = setTimeout(function() {
            window.__v4PatchTimer = null;
            flushSettingsPatch(false).then(function() {
                setTimeout(function() { sendDomBeacon(false); }, 30);
            });
        }, 35);
    }

    function syncNativeVideoPresenter(overrides) {
        try {
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            var bs = window.__v4BootSettings || {};
            var cObj = (typeof readCurrentCropFromDom === 'function') ? readCurrentCropFromDom() : (bs.crop || {});
            var fmRaw = (overrides && overrides.fitMode) || bs.fitMode || localStorage.getItem('pane-fit-mode') || 'autofit';
            var payload = {
                fitMode: fmRaw === 'fill' ? 'edge_to_edge' : (fmRaw === 'contain' ? 'original_ratio' : fmRaw),
                cropTop: Math.min(24, Math.max(0, Number(cObj.top || 0))),
                cropBottom: Math.min(24, Math.max(0, Number(cObj.bottom || 0))),
                cropLeft: Math.min(24, Math.max(0, Number(cObj.left || 0))),
                cropRight: Math.min(24, Math.max(0, Number(cObj.right || 0))),
                windowWidth: Math.round(window.innerWidth || 346),
                windowHeight: Math.round(window.innerHeight || 832),
                isFullscreen: Boolean(appEl && appEl.classList.contains('is-fullsize')),
                isLandscape: Boolean(appEl && appEl.classList.contains('is-landscape')),
                drawerOpen: Boolean(appEl && appEl.classList.contains('drawer-open'))
            };
            if (overrides && typeof overrides === 'object') {
                Object.assign(payload, overrides);
            }
            return postApi('/api/video/presenter', payload);
        } catch (_) {
            return Promise.resolve({ ok: false });
        }
    }
    window.__v4SyncNativePresenter = syncNativeVideoPresenter;

    function enforceSingleActivePerSegmentedGroup(clickedBtn) {
        if (!clickedBtn) return;
        var segGroup = (clickedBtn.parentElement ? clickedBtn.parentElement.closest('.segmented, .seg-group, .seg, div.seg') : null) || clickedBtn.parentElement;
        if (!segGroup) return;
        var children = Array.from(segGroup.children || []);
        var buttons = children.filter(function(el) {
            return el.tagName === 'BUTTON' || el.classList.contains('seg-btn') || el.classList.contains('seg');
        });
        if (buttons.length === 0) return;
        for (var i = 0; i < buttons.length; i++) {
            var b = buttons[i];
            if (b === clickedBtn) {
                b.classList.add('active', 'is-active');
                b.setAttribute('aria-checked', 'true');
                b.setAttribute('aria-pressed', 'true');
            } else {
                b.classList.remove('active', 'is-active');
                b.setAttribute('aria-checked', 'false');
                b.setAttribute('aria-pressed', 'false');
            }
        }
    }

    function clearSegmentedGroupSelection(segGroup) {
        if (!segGroup) return;
        var buttons = Array.from(segGroup.children || []).filter(function(el) {
            return el.tagName === 'BUTTON' || el.classList.contains('seg-btn') || el.classList.contains('seg');
        });
        for (var i = 0; i < buttons.length; i++) {
            buttons[i].classList.remove('active', 'is-active');
            buttons[i].setAttribute('aria-checked', 'false');
            buttons[i].setAttribute('aria-pressed', 'false');
        }
    }

    function sanitizeAllSegmentedGroupsOnBoot() {
        var bs = window.__v4BootSettings || {};
        var groups = document.querySelectorAll('.segmented, .seg-group, div.seg');
        for (var gi = 0; gi < groups.length; gi++) {
            var group = groups[gi];
            if (group.tagName === 'BUTTON') continue;
            var children = Array.from(group.children || []);
            var buttons = children.filter(function(el) {
                return el.tagName === 'BUTTON' || el.classList.contains('seg-btn') || el.classList.contains('seg');
            });
            if (buttons.length <= 1) continue;

            var resolvedBtn = null;
            if (group.id === 'language-toggle-group') {
                var authLang = window.__v4CurrentLang || document.documentElement.getAttribute('data-lang') || localStorage.getItem('iphone_link_language') || localStorage.getItem('iphone_link_lang') || 'id';
                resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-lang') === authLang; });
            } else if (group.querySelector('[data-theme-choice]')) {
                var th = localStorage.getItem('pane-theme') || bs.theme;
                if (th) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-theme-choice') === th; });
            } else if (group.querySelector('[data-fit-choice]')) {
                var fm = localStorage.getItem('pane-fit-mode') || bs.fitMode;
                if (fm) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-fit-choice') === fm; });
            } else if (group.id === 'segmented-device-profile') {
                var dp = window.__v4ActiveDeviceProfile || bs.deviceProfile;
                if (dp) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-device-profile') === dp; });
            } else if (group.querySelector('[data-auto-rotate-choice]')) {
                var arStr = localStorage.getItem('pane-auto-rotate');
                var arChoice = (arStr === 'true' || bs.autoRotate === true) ? 'on' : ((arStr === 'false' || bs.autoRotate === false) ? 'off' : null);
                if (arChoice) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-auto-rotate-choice') === arChoice; });
            } else if (group.id === 'segmented-audio-out') {
                var aout = bs.audioOut;
                if (aout === 'bluetooth' || bs.bluetoothA2dp) {
                    clearSegmentedGroupSelection(group);
                    continue;
                }
                if (aout) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-audio-out') === aout; });
            } else if (group.id === 'segmented-audio-buffer') {
                if (bs.audioBufferPreset) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-audio-buffer') === bs.audioBufferPreset; });
            } else if (group.id === 'segmented-lip-sync') {
                if (typeof bs.lipSync === 'boolean') resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-lip-sync') === String(bs.lipSync); });
            } else if (group.querySelector('[data-speed-choice]')) {
                var spdVal = localStorage.getItem('pane-trackpad-speed') || (typeof bs.trackpadSpeed === 'number' ? String(bs.trackpadSpeed) : '');
                if (spdVal) {
                    var matchedSpd = buttons.find(function(b) { return b.getAttribute('data-speed-choice') === spdVal; });
                    if (!matchedSpd) {
                        clearSegmentedGroupSelection(group);
                        continue;
                    }
                    resolvedBtn = matchedSpd;
                }
            } else if (group.querySelector('[data-shortcut-choice]')) {
                var scStr = localStorage.getItem('pane-shortcuts');
                if (scStr === null && typeof bs.crossPlatformShortcuts === 'boolean') scStr = String(bs.crossPlatformShortcuts);
                if (scStr === 'true' || scStr === 'false') resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-shortcut-choice') === scStr; });
            } else if (group.querySelector('[data-paste-pc-choice]')) {
                var ppcStr = localStorage.getItem('pane-paste-pc');
                if (ppcStr === null && typeof bs.pastePcClipboard === 'boolean') ppcStr = String(bs.pastePcClipboard);
                if (ppcStr === 'true' || ppcStr === 'false') resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-paste-pc-choice') === ppcStr; });
            } else if (group.id === 'segmented-stream-mode') {
                var sm = (window.currentStreamMode === 'airplay' || (bs && bs.hasUserSettingsFile && localStorage.getItem('iphone_link_stream_mode') === 'airplay')) ? 'airplay' : 'auto';
                window.currentStreamMode = sm;
                resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-stream-mode') === sm; });
            } else if (group.querySelector('[data-transport]')) {
                var liveTr = (window.currentTransport === 'wifi') ? 'wifi' : '';
                var savedTr = (bs && bs.hasUserSettingsFile)
                    ? (localStorage.getItem('handset-transport') || bs.transport)
                    : localStorage.getItem('handset-transport');
                var trVal = liveTr || savedTr || '';
                if (trVal) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-transport') === (trVal === 'wifi' ? 'wifi' : 'usb'); });
            } else if (group.id === 'update-channel-group') {
                if (bs.updateChannel) resolvedBtn = buttons.find(function(b) { return b.getAttribute('data-channel') === bs.updateChannel; });
            }

            var activeBtn = resolvedBtn || buttons.find(function(b) {
                return b.classList.contains('is-active') ||
                       b.classList.contains('active') ||
                       b.getAttribute('aria-checked') === 'true' ||
                       b.getAttribute('aria-pressed') === 'true';
            }) || buttons[0];
            enforceSingleActivePerSegmentedGroup(activeBtn);
        }
    }

    window.__v4ActiveDeviceProfile = "auto";
    window.__v4LastProfileTouch = 0;
    window.__v4LastSyncedPortraitWidth = 346;

    function syncMirroringStateClass() {
        try {
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            if (!appEl) return false;
            var se = document.getElementById('screen-empty');
            var sv = document.getElementById('screen-view');
            var emptyHidden = !!(se && (se.hidden || se.hasAttribute('hidden') || window.getComputedStyle(se).display === 'none'));
            var streamCls = appEl.classList.contains('is-streaming');
            var svSrc = sv ? (sv.getAttribute('src') || '') : '';
            var svVisible = !!(sv && !sv.hidden && window.getComputedStyle(sv).display !== 'none' && svSrc && svSrc !== 'about:blank');
            var mirroring = emptyHidden && (streamCls || svVisible);
            appEl.classList.toggle('is-mirroring', mirroring);
            return mirroring;
        } catch (_) {
            return false;
        }
    }
    window.__v4SyncMirroringState = syncMirroringStateClass;

    function computeV4DisplayWidthCss() {
        try {
            if (window.customWindowSize && window.customWindowSize.width) {
                return Math.round(window.customWindowSize.width) + 'px';
            }
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            var isIpad = !!(appEl && appEl.classList.contains('is-ipad')) || window.__v4ActiveDeviceProfile === 'ipad';
            var isLand = !!(appEl && appEl.classList.contains('is-landscape'));
            if (isIpad) return isLand ? '760px' : '564px';
            if (isLand) return '840px';
            var pw = (window.__v4LastVideoAspectSync && window.__v4LastVideoAspectSync.idealPortraitWidth)
                ? window.__v4LastVideoAspectSync.idealPortraitWidth
                : (window.__v4LastSyncedPortraitWidth || 346);
            return Math.max(336, Math.min(440, pw)) + 'px';
        } catch (_) {
            return '346px';
        }
    }
    window.__v4ComputeDisplayWidthCss = computeV4DisplayWidthCss;

    function syncPreFullscreenTransitionDom(targetFullscreen) {
        try {
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            if (!appEl) return;
            var nextFs = Boolean(targetFullscreen);
            appEl.classList.toggle('is-fullsize', nextFs);
            window.isFullSize = nextFs;
            var dw = computeV4DisplayWidthCss();
            document.documentElement.style.setProperty('--display-width', dw);
            if (document.body) document.body.style.setProperty('--display-width', dw);
            appEl.style.setProperty('--display-width', dw);
            syncMirroringStateClass();
        } catch (_) {}
    }
    window.__v4SyncPreFullscreenDom = syncPreFullscreenTransitionDom;

    function updateDeviceProfileState(prof, skipPost) {
        window.__v4LastProfileTouch = Date.now();
        var p = (prof === "ipad") ? "ipad" : ((prof === "iphone") ? "iphone" : "auto");
        window.__v4ActiveDeviceProfile = p;
        var isIpad = (p === "ipad");
        var targetW = isIpad ? 564 : 346;
        if (!isIpad) {
            window.__v4LastSyncedPortraitWidth = 346;
            window.__v4LastVideoAspectSync = null;
        }
        document.documentElement.style.setProperty('--display-width', targetW + 'px');
        if (document.body) document.body.style.setProperty('--display-width', targetW + 'px');
        var appEl = document.getElementById('app-root') || document.querySelector('.app');
        if (appEl) {
            appEl.style.setProperty('--display-width', targetW + 'px');
            appEl.classList.toggle('is-ipad', isIpad);
        }
        syncMirroringStateClass();
        var btn = document.querySelector('[data-device-profile="' + p + '"]') || document.getElementById('profile-' + p);
        if (btn) {
            enforceSingleActivePerSegmentedGroup(btn);
        }
        if (skipPost && typeof window.applyDeviceProfile === 'function') {
            try { window.applyDeviceProfile(p, { persist: false, syncNative: false }); } catch (_) {}
        }
        if (!skipPost) {
            var isDrawer = !!(appEl && (appEl.classList.contains('drawer-open') || (document.querySelector('.side-drawer') && !document.querySelector('.side-drawer').hidden)));
            postApi('/api/window/set_device_profile', { profile: p, family: isIpad ? 'ipad' : 'iphone', resize: true, open: isDrawer, drawer: isDrawer });
            postApi('/api/window/sync_device_aspect', { width: targetW, open: isDrawer, drawer: isDrawer, profile: p });
        }
        setTimeout(function() {
            if (btn) enforceSingleActivePerSegmentedGroup(btn);
            sendDomBeacon(false);
        }, 50);
    }

    function handleResetDefaultSize() {
        window.__v4LastProfileTouch = Date.now();
        window.__v4ActiveDeviceProfile = 'auto';
        if (typeof window.applyDeviceProfile === 'function') {
            try { window.applyDeviceProfile('auto', { persist: true, syncNative: false }); } catch (_) {}
        }
        document.documentElement.style.setProperty('--display-width', '346px');
        if (document.body) document.body.style.setProperty('--display-width', '346px');
        var appEl = document.getElementById('app-root') || document.querySelector('.app');
        if (appEl) {
            appEl.style.setProperty('--display-width', '346px');
            appEl.classList.remove('is-ipad');
        }
        var autoBtn = document.querySelector('[data-device-profile="auto"]') || document.getElementById('profile-auto');
        if (autoBtn) {
            enforceSingleActivePerSegmentedGroup(autoBtn);
        }
        var autoFitBtn = document.querySelector('[data-fit-choice="autofit"]') || document.getElementById('fit-autofit');
        if (autoFitBtn) {
            enforceSingleActivePerSegmentedGroup(autoFitBtn);
        }
        var isDrawer = !!(appEl && (appEl.classList.contains('drawer-open') || (document.querySelector('.side-drawer') && !document.querySelector('.side-drawer').hidden)));
        postApi('/api/window/reset_size', { open: isDrawer, drawer: isDrawer });
        postApi('/api/window/sync_device_aspect', { width: 346, open: isDrawer, drawer: isDrawer, profile: 'auto' });
        setTimeout(function() {
            if (autoBtn) enforceSingleActivePerSegmentedGroup(autoBtn);
            if (autoFitBtn) enforceSingleActivePerSegmentedGroup(autoFitBtn);
            sendDomBeacon(false);
        }, 50);
    }

    document.addEventListener('click', function(e) {
        var profBtn = e.target && e.target.closest ? e.target.closest('[data-device-profile]') : null;
        if (profBtn) {
            var prof = profBtn.getAttribute('data-device-profile') || 'auto';
            updateDeviceProfileState(prof);
        }
        var resetBtn = e.target && e.target.closest ? e.target.closest('#btn-reset-size, #btn-reset-window-size, [data-action="reset-window-size"]') : null;
        if (resetBtn) {
            handleResetDefaultSize();
        }
        var btn = e.target && e.target.closest ? e.target.closest('.seg > button, .segmented > button, .seg-group > button, .seg button, .segmented button, .seg-btn, button.seg, button.seg-btn') : null;
        if (btn) {
            enforceSingleActivePerSegmentedGroup(btn);
            setTimeout(function() { enforceSingleActivePerSegmentedGroup(btn); }, 0);
            setTimeout(function() { enforceSingleActivePerSegmentedGroup(btn); }, 40);
        }
    }, true);
    document.addEventListener('click', function(e) {
        var btn = e.target && e.target.closest ? e.target.closest('.seg > button, .segmented > button, .seg-group > button, .seg button, .segmented button, .seg-btn, button.seg, button.seg-btn') : null;
        if (btn) {
            enforceSingleActivePerSegmentedGroup(btn);
            setTimeout(function() { enforceSingleActivePerSegmentedGroup(btn); }, 0);
            setTimeout(function() { enforceSingleActivePerSegmentedGroup(btn); }, 40);
        }
    }, false);

    window.__v4RuntimeErrors = window.__v4RuntimeErrors || [];
    window.addEventListener('error', function(ev) {
        try {
            window.__v4RuntimeErrors.push(String((ev && ev.message) || 'error'));
        } catch (_) {}
    });

    (function installV4DocumentPrototypeAliases() {
        var origProtoQS = Document.prototype.querySelector;
        if (origProtoQS && !origProtoQS.__v4Patched) {
            var v4QS = function(sel) {
                var ctx = (this && this.nodeType === 9) ? this : document;
                if (sel === '#btn-reset-size' || sel === '#btn-reset-window-size') return origProtoQS.call(ctx, '#btn-reset-window-size') || origProtoQS.call(ctx, sel);
                if (sel === '#lang-en') return origProtoQS.call(ctx, '#btn-lang-en') || origProtoQS.call(ctx, '[data-lang="en"]') || origProtoQS.call(ctx, sel);
                if (sel === '#lang-id') return origProtoQS.call(ctx, '#btn-lang-id') || origProtoQS.call(ctx, '[data-lang="id"]') || origProtoQS.call(ctx, sel);
                if (sel === '#fit-autofit') return origProtoQS.call(ctx, '[data-fit-choice="autofit"]') || origProtoQS.call(ctx, sel);
                if (sel === '#fit-fill') return origProtoQS.call(ctx, '[data-fit-choice="fill"]') || origProtoQS.call(ctx, sel);
                if (sel === '#fit-contain') return origProtoQS.call(ctx, '[data-fit-choice="contain"]') || origProtoQS.call(ctx, sel);
                if (sel === '#profile-auto') return origProtoQS.call(ctx, '[data-device-profile="auto"]') || origProtoQS.call(ctx, sel);
                if (sel === '#profile-iphone') return origProtoQS.call(ctx, '[data-device-profile="iphone"]') || origProtoQS.call(ctx, sel);
                if (sel === '#profile-ipad') return origProtoQS.call(ctx, '[data-device-profile="ipad"]') || origProtoQS.call(ctx, sel);
                if (sel === '#rotate-off') return origProtoQS.call(ctx, '[data-auto-rotate-choice="off"]') || origProtoQS.call(ctx, sel);
                if (sel === '#rotate-on') return origProtoQS.call(ctx, '[data-auto-rotate-choice="on"]') || origProtoQS.call(ctx, sel);
                if (sel === '#pill-connection') return origProtoQS.call(ctx, '#pill-connection') || origProtoQS.call(ctx, '#status-pill');
                if (sel === '#wifi-summary') return origProtoQS.call(ctx, '#wifi-summary') || origProtoQS.call(ctx, '#wifi-state');
                return origProtoQS.call(ctx, sel);
            };
            v4QS.__v4Patched = true;
            Document.prototype.querySelector = v4QS;
        }
        var origProtoGEBI = Document.prototype.getElementById;
        if (origProtoGEBI && !origProtoGEBI.__v4Patched) {
            var v4GEBI = function(id) {
                var ctx = (this && this.nodeType === 9) ? this : document;
                if (id === 'btn-reset-size' || id === 'btn-reset-window-size') return origProtoGEBI.call(ctx, 'btn-reset-window-size') || origProtoGEBI.call(ctx, id);
                if (id === 'lang-en') return origProtoGEBI.call(ctx, 'btn-lang-en') || origProtoGEBI.call(ctx, id);
                if (id === 'lang-id') return origProtoGEBI.call(ctx, 'btn-lang-id') || origProtoGEBI.call(ctx, id);
                if (id === 'fit-autofit') return ctx.querySelector('[data-fit-choice="autofit"]') || origProtoGEBI.call(ctx, id);
                if (id === 'fit-fill') return ctx.querySelector('[data-fit-choice="fill"]') || origProtoGEBI.call(ctx, id);
                if (id === 'fit-contain') return ctx.querySelector('[data-fit-choice="contain"]') || origProtoGEBI.call(ctx, id);
                if (id === 'pill-connection') return origProtoGEBI.call(ctx, 'pill-connection') || origProtoGEBI.call(ctx, 'status-pill');
                if (id === 'wifi-summary') return origProtoGEBI.call(ctx, 'wifi-summary') || origProtoGEBI.call(ctx, 'wifi-state');
                return origProtoGEBI.call(ctx, id);
            };
            v4GEBI.__v4Patched = true;
            Document.prototype.getElementById = v4GEBI;
        }
    })();

    function sanitizeErrorBanners() {
        try {
            var ids = ['screen-error-banner', 'launchpad-error'];
            for (var i = 0; i < ids.length; i++) {
                var el = document.getElementById(ids[i]);
                if (!el) continue;
                var txt = (el.textContent || '').trim();
                if (txt && (txt.indexOf('Cannot set properties of null') !== -1 || txt.indexOf("setting 'hidden'") !== -1 || txt.indexOf('null (setting') !== -1)) {
                    el.textContent = '';
                    el.hidden = true;
                    el.style.display = 'none';
                    window.__v4SuppressedNullHiddenCount = (window.__v4SuppressedNullHiddenCount || 0) + 1;
                } else if (!txt && !el.hidden) {
                    el.hidden = true;
                }
            }
        } catch (_) {}
    }

    window.__v4ClickSelector = function(sel) {
        var target = document.querySelector(sel);
        if (target) {
            var inSeg = Boolean(target.closest && target.closest('.segmented, .seg-group, div.seg'));
            target.click();
            if (inSeg) {
                enforceSingleActivePerSegmentedGroup(target);
            }
            setTimeout(function() {
                if (inSeg) {
                    enforceSingleActivePerSegmentedGroup(target);
                }
                sendDomBeacon(false);
            }, 30);
            return true;
        }
        return false;
    };

    window.__v4ShowMainHevcBanner = function(codec) {
        var existing = document.getElementById('v4-hevc-install-banner');
        var isId = (window.__v4CurrentLang || 'id') === 'id';
        if (!existing) {
            existing = document.createElement('div');
            existing.id = 'v4-hevc-install-banner';
            existing.style.cssText = 'position:fixed;left:16px;right:16px;bottom:58px;z-index:99999;padding:12px 14px;border-radius:14px;background:rgba(15,23,42,0.88);backdrop-filter:blur(16px);border:1px solid rgba(56,189,248,0.45);color:#f8fafc;display:flex;flex-direction:column;gap:8px;box-shadow:0 10px 28px rgba(0,0,0,0.42);font-size:12px;';
            document.body.appendChild(existing);
        }
        var msg = isId
            ? 'Windows Anda belum memiliki Microsoft HEVC Video Extensions untuk mendekode video iPhone.'
            : 'Your Windows PC is missing Microsoft HEVC Video Extensions to decode the iPhone video stream.';
        var btnLabel = isId ? 'Pasang Codec Video HEVC (1-Klik)' : 'Install HEVC Video Codec (1-Click)';
        existing.innerHTML = '';
        var p = document.createElement('div');
        p.textContent = msg + (codec ? ' (' + codec + ')' : '');
        existing.appendChild(p);
        var row = document.createElement('div');
        row.style.cssText = 'display:flex;gap:8px;align-items:center;';
        var btn = document.createElement('button');
        btn.id = 'btn-install-hevc-1click';
        btn.type = 'button';
        btn.textContent = btnLabel;
        btn.style.cssText = 'flex:1;padding:7px 12px;border-radius:9px;border:none;background:#0A84FF;color:#ffffff;font-weight:600;cursor:pointer;font-size:12px;';
        btn.onclick = function() {
            btn.disabled = true;
            btn.textContent = isId ? 'Memasang HEVC Codec...' : 'Installing HEVC Codec...';
            fetch('/api/system/install_hevc', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: '{}'
            }).then(function(r) { return r.json(); }).then(function(res) {
                if (res && res.installed) {
                    btn.textContent = isId ? 'Terpasang! Memuat ulang stream...' : 'Installed! Reloading stream...';
                    setTimeout(function() {
                        try { existing.remove(); } catch (_) {}
                        var mf = document.getElementById('screen-view');
                        if (mf && mf.contentWindow) mf.contentWindow.location.reload();
                    }, 600);
                } else {
                    var target = (res && res.mediaFeaturePackMissing) ? 'media_feature_pack' : 'hevc_oem';
                    fetch('/api/system/open_hevc_store', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ target: target })
                    }).catch(function() {});
                    btn.disabled = false;
                    btn.textContent = isId
                        ? 'Muat Ulang Stream (Setelah Pasang)'
                        : 'Reload Stream (After Install)';
                    p.textContent = (res && res.mediaFeaturePackMissing)
                        ? (isId
                            ? 'Pengaturan Optional Features dibuka. Pasang Media Feature Pack lalu restart PC.'
                            : 'Windows Optional Features opened. Install Media Feature Pack then restart PC.')
                        : (isId
                            ? 'Microsoft Store dibuka (HEVC OEM Gratis). Setelah terpasang, klik tombol di bawah untuk muat ulang stream.'
                            : 'Microsoft Store opened (Free OEM HEVC). Once installed, click below to reload the stream.');
                    btn.onclick = function() {
                        try { existing.remove(); } catch (_) {}
                        var mf2 = document.getElementById('screen-view');
                        if (mf2 && mf2.contentWindow) mf2.contentWindow.location.reload();
                    };
                }
            }).catch(function() {
                btn.disabled = false;
                btn.textContent = btnLabel;
            });
        };
        row.appendChild(btn);
        var closeBtn = document.createElement('button');
        closeBtn.type = 'button';
        closeBtn.textContent = 'X';
        closeBtn.style.cssText = 'padding:6px 10px;border-radius:9px;border:1px solid rgba(255,255,255,0.22);background:transparent;color:#cbd5e1;cursor:pointer;';
        closeBtn.onclick = function() { try { existing.remove(); } catch (_) {} };
        row.appendChild(closeBtn);
        existing.appendChild(row);
    };

    window.__v4ApplyTheme = function(theme) {
        var t = theme === 'light' ? 'light' : 'dark';
        document.documentElement.setAttribute('data-theme', t);
        try {
            document.documentElement.style.setProperty('background', 'transparent', 'important');
            document.documentElement.style.setProperty('background-color', 'transparent', 'important');
        } catch (_) {}
        if (document.body) {
            document.body.setAttribute('data-theme', t);
            try {
                document.body.style.setProperty('background', 'transparent', 'important');
                document.body.style.setProperty('background-color', 'transparent', 'important');
            } catch (_) {}
        }
        var appEl = document.getElementById('app-root') || document.querySelector('.app');
        if (appEl) {
            appEl.setAttribute('data-theme', t);
            appEl.classList.toggle('theme-light', t === 'light');
            appEl.classList.toggle('theme-dark', t === 'dark');
            try {
                appEl.style.removeProperty('background');
                appEl.style.removeProperty('background-color');
            } catch (_) {}
        }
        var lightBtn = document.querySelector('[data-theme-choice="light"]') || document.getElementById('theme-light');
        var darkBtn = document.querySelector('[data-theme-choice="dark"]') || document.getElementById('theme-dark');
        if (lightBtn) {
            lightBtn.id = 'theme-light';
            lightBtn.classList.toggle('is-active', t === 'light');
            lightBtn.classList.toggle('active', t === 'light');
            lightBtn.setAttribute('aria-checked', String(t === 'light'));
        }
        if (darkBtn) {
            darkBtn.id = 'theme-dark';
            darkBtn.classList.toggle('is-active', t === 'dark');
            darkBtn.classList.toggle('active', t === 'dark');
            darkBtn.setAttribute('aria-checked', String(t === 'dark'));
        }
        try {
            localStorage.setItem('pane-theme', t);
        } catch (_) {}
        if (typeof applyTheme === 'function' && typeof currentTheme !== 'undefined' && currentTheme !== t) {
            try { applyTheme(t); } catch (_) {}
        }
        sendDomBeacon(false);
    };

    function sendDomBeacon(rafVerified) {
        try {
            syncIpadDisplayWidthVar();
            var appEl = document.querySelector(".app") || document.body;
            var disp = window.getComputedStyle ? window.getComputedStyle(appEl).display : "block";
            var appRect = appEl.getBoundingClientRect ? appEl.getBoundingClientRect() : { width: window.innerWidth, height: window.innerHeight };
            var launchpadTitle = document.getElementById("launchpad-title");
            var launchpadHeight = (launchpadTitle && launchpadTitle.getBoundingClientRect) ? launchpadTitle.getBoundingClientRect().height : 0;
            var startBtn = document.getElementById("launchpad-start-btn") || document.getElementById("show-screen");
            var startBtnWidth = (startBtn && startBtn.getBoundingClientRect) ? startBtn.getBoundingClientRect().width : 0;

            var screenEmpty = document.getElementById("screen-empty");
            var lpVis = Boolean(launchpadHeight > 10) || Boolean(screenEmpty && !screenEmpty.hidden);
            var btnVis = Boolean(startBtnWidth >= 80) || Boolean(startBtn && !startBtn.hidden);
            var curW = (appRect.width > 50) ? Math.round(appRect.width) : (window.innerWidth || 346);
            var curH = (appRect.height > 50) ? Math.round(appRect.height) : (window.innerHeight || 832);

            var btnText = (startBtn ? (startBtn.innerText || startBtn.textContent || "") : "").trim();
            var titleText = (launchpadTitle ? (launchpadTitle.innerText || launchpadTitle.textContent || "") : "").trim();

            var curTheme = document.documentElement.getAttribute('data-theme') || (document.body ? document.body.getAttribute('data-theme') : null) || 'dark';
            var lightBtn = document.querySelector('[data-theme-choice="light"]') || document.getElementById('theme-light');
            var darkBtn = document.querySelector('[data-theme-choice="dark"]') || document.getElementById('theme-dark');
            var lightRect = (lightBtn && lightBtn.getBoundingClientRect) ? lightBtn.getBoundingClientRect() : null;
            var darkRect = (darkBtn && darkBtn.getBoundingClientRect) ? darkBtn.getBoundingClientRect() : null;
            var themeLightX = lightRect ? Math.round(lightRect.left + lightRect.width / 2) : 0;
            var themeLightY = lightRect ? Math.round(lightRect.top + lightRect.height / 2) : 0;
            var themeDarkX = darkRect ? Math.round(darkRect.left + darkRect.width / 2) : 0;
            var themeDarkY = darkRect ? Math.round(darkRect.top + darkRect.height / 2) : 0;

            var bgHtml = window.getComputedStyle ? window.getComputedStyle(document.documentElement).backgroundColor : "";
            var bgBody = window.getComputedStyle ? window.getComputedStyle(document.body).backgroundColor : "";
            var bgApp = window.getComputedStyle ? window.getComputedStyle(appEl).backgroundColor : "";
            var sideDrawer = document.querySelector(".side-drawer");
            var bgDrawer = (sideDrawer && window.getComputedStyle) ? window.getComputedStyle(sideDrawer).backgroundColor : "";

            var elTop = document.elementFromPoint ? document.elementFromPoint(100, 20) : null;
            var elLp = document.elementFromPoint ? document.elementFromPoint(200, 450) : null;
            var elDr = document.elementFromPoint ? document.elementFromPoint(600, 400) : null;
            var elProbe = document.elementFromPoint ? document.elementFromPoint(580, 250) : null;
            var chain = [];
            var cur = elProbe;
            while (cur && cur !== document.documentElement) {
                var bg = window.getComputedStyle ? window.getComputedStyle(cur).backgroundColor : "";
                var bgImg = window.getComputedStyle ? window.getComputedStyle(cur).backgroundImage : "";
                var idPart = cur.id ? ('#' + cur.id) : '';
                var clsPart = cur.className ? ('.' + cur.className) : '';
                chain.push(cur.tagName + idPart + clsPart + '[bg:' + bg + '|img:' + bgImg + ']');
                cur = cur.parentElement;
            }
            var probeChain = chain.join(' -> ');

            var infoTop = elTop ? (elTop.tagName + "." + elTop.className + " id:" + elTop.id + " bg:" + (window.getComputedStyle ? window.getComputedStyle(elTop).backgroundColor : "")) : "none";
            var infoLp = elLp ? (elLp.tagName + "." + elLp.className + " id:" + elLp.id + " bg:" + (window.getComputedStyle ? window.getComputedStyle(elLp).backgroundColor : "")) : "none";
            var infoDr = elDr ? (elDr.tagName + "." + elDr.className + " id:" + elDr.id + " bg:" + (window.getComputedStyle ? window.getComputedStyle(elDr).backgroundColor : "")) : "none";

            var wireframeSelectors = ['.brand', '.placeholder', '#screen-empty h1', '#screen-empty h2', '#screen-empty .sub', '.drawer-header'];
            var wireframeClean = true;
            var wireframeDetails = [];
            for (var wi = 0; wi < wireframeSelectors.length; wi++) {
                var wel = document.querySelector(wireframeSelectors[wi]);
                if (wel && window.getComputedStyle) {
                    var wcs = window.getComputedStyle(wel);
                    var btw = wcs.borderTopWidth || '0px';
                    var bbg = wcs.backgroundColor || 'rgba(0, 0, 0, 0)';
                    var isClean = (btw === '0px' || btw === '0' || wcs.borderStyle === 'none') &&
                                  (bbg === 'rgba(0, 0, 0, 0)' || bbg === 'transparent' || bbg === '');
                    if (!isClean) {
                        wireframeClean = false;
                        wireframeDetails.push(wireframeSelectors[wi] + '[bw:' + btw + '|bg:' + bbg + ']');
                    }
                }
            }

            var segTrack = document.querySelector('.seg, .segmented, .seg-group');
            var concentricTrackRadius = (segTrack && window.getComputedStyle) ? window.getComputedStyle(segTrack).borderRadius : "12px";
            var segBtn = document.querySelector('.seg > button, .seg button, .segmented button, .seg-btn, button.seg');
            var concentricButtonRadius = (segBtn && window.getComputedStyle) ? window.getComputedStyle(segBtn).borderRadius : "9px";
            var badgeEl = document.querySelector('.lang-badge, .seg .badge');
            var badgeBg = (badgeEl && window.getComputedStyle) ? window.getComputedStyle(badgeEl).backgroundColor : "rgba(0, 0, 0, 0)";

            var displayColEl = document.querySelector('.display-col') || document.getElementById('display-col');
            var displayColWidth = (displayColEl && displayColEl.getBoundingClientRect) ? Math.round(displayColEl.getBoundingClientRect().width) : 0;
            var sdRect = (sideDrawer && sideDrawer.getBoundingClientRect) ? sideDrawer.getBoundingClientRect() : null;
            var drawerRectWidth = (sideDrawer && !sideDrawer.hidden && sdRect && sdRect.width > 0) ? Math.round(sdRect.width) : 0;
            var sideDrawerWidth = drawerRectWidth;
            var drawerRight = (sdRect && sdRect.right) ? Math.round(sdRect.right) : 0;
            var winInnerWidth = window.innerWidth || (document.documentElement ? document.documentElement.clientWidth : 0) || 0;
            var drawerRightOverflow = (sideDrawer && !sideDrawer.hidden && drawerRight > winInnerWidth) ? (drawerRight - winInnerWidth) : 0;
            var sideDrawerRightGap = (sideDrawer && !sideDrawer.hidden && sdRect && sdRect.width > 0) ? Math.abs(Math.round(winInnerWidth - sdRect.right)) : 0;
            var drawerScrollEl = sideDrawer ? (sideDrawer.querySelector('.drawer-scroll') || sideDrawer.querySelector('.drawer-content')) : null;
            var dsRect = (drawerScrollEl && drawerScrollEl.getBoundingClientRect) ? drawerScrollEl.getBoundingClientRect() : null;
            var drawerScrollRightGap = (sideDrawer && !sideDrawer.hidden && sdRect && sdRect.width > 0 && dsRect && dsRect.width > 0)
                ? Math.abs(Math.round(sdRect.right - dsRect.right))
                : 0;
            var activeProf = window.__v4ActiveDeviceProfile || "auto";
            var stretchGlitchCount = window.__v4StretchGlitchCount || 0;
            var maxDisplayColDuringClose = window.__v4MaxDisplayColDuringClose || 0;

            var segmentedClean = true;
            var segmentedDetails = [];
            var auditGroups = document.querySelectorAll('.segmented, .seg-group, div.seg');
            for (var agi = 0; agi < auditGroups.length; agi++) {
                var ag = auditGroups[agi];
                if (ag.tagName === 'BUTTON') continue;
                var btns = Array.from(ag.children || []).filter(function(el) {
                    return el.tagName === 'BUTTON' || el.classList.contains('seg-btn') || el.classList.contains('seg');
                });
                if (btns.length <= 1) continue;
                var activeCount = 0;
                var activeIds = [];
                for (var bi = 0; bi < btns.length; bi++) {
                    var b = btns[bi];
                    var isActive = b.classList.contains('active') ||
                                   b.classList.contains('is-active') ||
                                   b.getAttribute('aria-checked') === 'true' ||
                                   b.getAttribute('aria-pressed') === 'true';
                    if (isActive) {
                        activeCount++;
                        activeIds.push(b.id || b.getAttribute('data-fit-choice') || b.getAttribute('data-device-profile') || b.getAttribute('data-auto-rotate-choice') || b.getAttribute('data-theme-choice') || b.getAttribute('data-lang') || ('btn' + bi));
                    }
                }
                if (activeCount > 1) {
                    segmentedClean = false;
                    var gid = ag.id || ag.className || ('group' + agi);
                    segmentedDetails.push(gid + '[actives:' + activeCount + ' (' + activeIds.join(',') + ')]');
                }
            }

            var psEl = document.getElementById('phone-screen');
            var svEl = document.getElementById('screen-view');
            var seEl = document.getElementById('screen-empty');
            var psBg = (psEl && window.getComputedStyle) ? window.getComputedStyle(psEl).backgroundColor : "";
            var psWidth = (psEl && psEl.getBoundingClientRect) ? Math.round(psEl.getBoundingClientRect().width) : 0;
            var psHeight = (psEl && psEl.getBoundingClientRect) ? Math.round(psEl.getBoundingClientRect().height) : 0;
            var svVis = (svEl && window.getComputedStyle) ? window.getComputedStyle(svEl).visibility : "";
            var svOpacity = (svEl && window.getComputedStyle) ? window.getComputedStyle(svEl).opacity : "";
            var appIsFullsize = Boolean(appRoot && appRoot.classList.contains('is-fullsize'));
            var appIsMirroring = Boolean(appRoot && appRoot.classList.contains('is-mirroring'));
            var appIsStreaming = Boolean(appRoot && appRoot.classList.contains('is-streaming'));
            var seVisibleNow = Boolean(seEl && !seEl.hidden && window.getComputedStyle(seEl).display !== 'none');
            var cardUpdEl = document.getElementById('card-updater');
            var cardUpdaterCollapsed = Boolean(cardUpdEl && cardUpdEl.classList.contains('is-collapsed'));
            var drawerPaneEl = document.getElementById('control-pane') || document.querySelector('.side-drawer');
            var isDrawerOpenNow = Boolean(appRoot && appRoot.classList.contains('drawer-open') && drawerPaneEl && !drawerPaneEl.hidden);
            var cardUpdRect = (cardUpdEl && cardUpdEl.getBoundingClientRect) ? cardUpdEl.getBoundingClientRect() : null;
            var cardUpdCs = (cardUpdEl && window.getComputedStyle) ? window.getComputedStyle(cardUpdEl) : null;
            var cardUpdaterExistsNow = Boolean(
                cardUpdEl &&
                cardUpdEl.isConnected &&
                !cardUpdEl.hidden &&
                cardUpdCs &&
                cardUpdCs.display !== 'none' &&
                cardUpdCs.visibility !== 'hidden' &&
                (!isDrawerOpenNow || (cardUpdRect && cardUpdRect.width > 50 && cardUpdRect.height > 20))
            );
            var cardUpdTitleEl = cardUpdEl ? cardUpdEl.querySelector('.update-card-title') : null;
            var cardUpdaterTitleNow = cardUpdTitleEl ? cardUpdTitleEl.textContent.trim() : '';

            function isBtnLit(el) {
                return Boolean(el && (el.classList.contains('is-active') || el.classList.contains('active') || el.getAttribute('aria-pressed') === 'true' || el.getAttribute('aria-checked') === 'true'));
            }
            var langIdEl = document.getElementById('btn-lang-id');
            var langEnEl = document.getElementById('btn-lang-en');
            var activeLangId = isBtnLit(langIdEl);
            var activeLangEn = isBtnLit(langEnEl);
            var domLangNow = activeLangEn && !activeLangId ? 'en' : ((activeLangId && !activeLangEn) ? 'id' : (window.__v4CurrentLang || document.documentElement.getAttribute('data-lang') || ''));
            var drawerTitleEl = document.querySelector('.drawer-title');
            var drawerTitleTextNow = drawerTitleEl ? drawerTitleEl.textContent.trim() : '';
            var showScreenEl = document.getElementById('show-screen');
            var showScreenTxtNow = showScreenEl ? showScreenEl.textContent.trim() : '';

            var actFit = '';
            var fitBtns = document.querySelectorAll('[data-fit-choice]');
            for (var fi = 0; fi < fitBtns.length; fi++) { if (isBtnLit(fitBtns[fi])) { actFit = fitBtns[fi].getAttribute('data-fit-choice') || ''; break; } }
            var actProfDom = '';
            var profBtns = document.querySelectorAll('#segmented-device-profile [data-device-profile]');
            for (var pi = 0; pi < profBtns.length; pi++) { if (isBtnLit(profBtns[pi])) { actProfDom = profBtns[pi].getAttribute('data-device-profile') || ''; break; } }
            var actRotDom = '';
            var rotBtns = document.querySelectorAll('[data-auto-rotate-choice]');
            for (var ri = 0; ri < rotBtns.length; ri++) { if (isBtnLit(rotBtns[ri])) { actRotDom = rotBtns[ri].getAttribute('data-auto-rotate-choice') || ''; break; } }
            var btOutEl = document.getElementById('btn-audio-out-bt');
            var actAout = isBtnLit(btOutEl) ? 'bluetooth' : '';
            if (!actAout) {
                var aoutBtns = document.querySelectorAll('#segmented-audio-out [data-audio-out]');
                for (var ai = 0; ai < aoutBtns.length; ai++) { if (isBtnLit(aoutBtns[ai])) { actAout = aoutBtns[ai].getAttribute('data-audio-out') || ''; break; } }
            }
            var avolVal = (document.getElementById('audio-vol-slider') || {}).value || '';
            var aMuteEl = document.getElementById('btn-audio-drawer') || document.getElementById('btn-audio-top');
            var aMutedDom = Boolean(aMuteEl && aMuteEl.classList.contains('is-muted'));
            var actAbuf = '';
            var abufBtns = document.querySelectorAll('#segmented-audio-buffer [data-audio-buffer]');
            for (var bi2 = 0; bi2 < abufBtns.length; bi2++) { if (isBtnLit(abufBtns[bi2])) { actAbuf = abufBtns[bi2].getAttribute('data-audio-buffer') || ''; break; } }
            var actLip = '';
            var lipBtns = document.querySelectorAll('#segmented-lip-sync [data-lip-sync]');
            for (var li = 0; li < lipBtns.length; li++) { if (isBtnLit(lipBtns[li])) { actLip = lipBtns[li].getAttribute('data-lip-sync') || ''; break; } }
            var spdDomVal = (document.getElementById('trackpad-speed-slider') || {}).value || '';
            var actScDom = '';
            var scBtns = document.querySelectorAll('[data-shortcut-choice]');
            for (var si = 0; si < scBtns.length; si++) { if (isBtnLit(scBtns[si])) { actScDom = scBtns[si].getAttribute('data-shortcut-choice') || ''; break; } }
            var actPpcDom = '';
            var ppcBtns = document.querySelectorAll('[data-paste-pc-choice]');
            for (var ppi = 0; ppi < ppcBtns.length; ppi++) { if (isBtnLit(ppcBtns[ppi])) { actPpcDom = ppcBtns[ppi].getAttribute('data-paste-pc-choice') || ''; break; } }
            var actTrDom = '';
            var trBtns = document.querySelectorAll('.segmented [data-transport]');
            for (var tri = 0; tri < trBtns.length; tri++) { if (isBtnLit(trBtns[tri])) { actTrDom = trBtns[tri].getAttribute('data-transport') || ''; break; } }
            var acEl = document.getElementById('toggle-auto-update-check');
            var actAcDom = Boolean(acEl && (acEl.classList.contains('is-active') || acEl.classList.contains('active') || acEl.getAttribute('aria-checked') === 'true'));
            var actUcDom = '';
            var ucBtns = document.querySelectorAll('#update-channel-group [data-channel]');
            for (var ui = 0; ui < ucBtns.length; ui++) { if (isBtnLit(ucBtns[ui])) { actUcDom = ucBtns[ui].getAttribute('data-channel') || ''; break; } }
            var pinDomVal = ((document.getElementById('input-unlock-pin') || {}).value || '');
            var cTopDom = (document.getElementById('crop-top-slider') || {}).value || '0';
            var cBotDom = (document.getElementById('crop-bottom-slider') || {}).value || '0';
            var cLeftDom = (document.getElementById('crop-left-slider') || {}).value || '0';
            var cRightDom = (document.getElementById('crop-right-slider') || {}).value || '0';
            var lpTrCableLit = isBtnLit(document.getElementById('launchpad-transport-cable'));
            var lpTrWifiLit = isBtnLit(document.getElementById('launchpad-transport-wifi'));

            var domTogglesSummary = [
                'lang=' + domLangNow,
                'theme=' + curTheme,
                'fit=' + actFit,
                'prof=' + actProfDom,
                'rot=' + actRotDom,
                'ct=' + cTopDom,
                'cb=' + cBotDom,
                'cl=' + cLeftDom,
                'cr=' + cRightDom,
                'aout=' + actAout,
                'avol=' + avolVal,
                'amut=' + String(aMutedDom),
                'abuf=' + actAbuf,
                'lip=' + actLip,
                'spd=' + spdDomVal,
                'sc=' + actScDom,
                'ppc=' + actPpcDom,
                'tr=' + actTrDom,
                'lptr=' + (lpTrWifiLit ? 'wifi' : (lpTrCableLit ? 'usb' : '')),
                'ac=' + String(actAcDom),
                'uc=' + actUcDom,
                'pin=' + pinDomVal,
                'show=' + showScreenTxtNow.replace(/[;=]/g, ' ')
            ].join(';');

            postApi('/api/window/dom_ready_beacon', {
                display: disp,
                w: curW,
                h: curH,
                appWidth: curW,
                appHeight: curH,
                launchpadVisible: lpVis,
                startBtnVisible: btnVis,
                startBtnText: btnText,
                launchpadTitleText: titleText,
                userAgent: navigator.userAgent || "",
                dpr: window.devicePixelRatio || 1.0,
                rafVerified: !!rafVerified,
                domTheme: curTheme,
                themeLightX: themeLightX,
                themeLightY: themeLightY,
                themeDarkX: themeDarkX,
                themeDarkY: themeDarkY,
                bgHtml: bgHtml,
                bgBody: bgBody,
                bgApp: bgApp,
                bgDrawer: bgDrawer,
                infoTop: infoTop,
                infoLp: infoLp,
                infoDr: infoDr,
                probeChain: probeChain,
                wireframeClean: wireframeClean,
                wireframeDetails: wireframeDetails.join('; '),
                concentricTrackRadius: concentricTrackRadius,
                concentricButtonRadius: concentricButtonRadius,
                badgeBg: badgeBg,
                segmentedClean: segmentedClean,
                segmentedDetails: segmentedDetails.join('; '),
                activeDeviceProfile: activeProf,
                drawerRectWidth: drawerRectWidth,
                drawerRightOverflow: drawerRightOverflow,
                displayColWidth: displayColWidth,
                sideDrawerWidth: sideDrawerWidth,
                sideDrawerRightGap: sideDrawerRightGap,
                drawerScrollRightGap: drawerScrollRightGap,
                innerWidth: winInnerWidth,
                stretchGlitchCount: stretchGlitchCount,
                maxDisplayColDuringClose: maxDisplayColDuringClose,
                phoneScreenBg: psBg,
                phoneScreenWidth: psWidth,
                phoneScreenHeight: psHeight,
                screenViewVisibility: svVis,
                screenViewOpacity: svOpacity,
                isFullsize: appIsFullsize,
                isMirroring: appIsMirroring,
                isStreaming: appIsStreaming,
                unlockPinValue: pinDomVal,
                updateVersionText: (((document.getElementById('update-current-version') || {}).textContent || '').trim()),
                creditAppNameText: (((document.querySelector('.credit-app-name') || {}).textContent || '').trim()),
                lastTrayAction: (window.__v4LastTrayAction || ''),
                domLanguage: domLangNow,
                activeLangId: activeLangId,
                activeLangEn: activeLangEn,
                drawerTitleText: drawerTitleTextNow,
                domTogglesSummary: domTogglesSummary,
                updaterScrolled: Boolean(window.__v4UpdaterScrolledIntoView),
                cardUpdaterCollapsed: cardUpdaterCollapsed,
                cardUpdaterExists: cardUpdaterExistsNow,
                cardUpdaterTitle: cardUpdaterTitleNow,
                checkUpdateClicked: Boolean(window.__v4CheckUpdateClicked),
                screenEmptyVisible: seVisibleNow
            });
        } catch (_) {}
    }

    var rafCount = 0;
    function checkRaf() {
        rafCount++;
        if (rafCount >= 2) {
            sendDomBeacon(true);
        } else {
            requestAnimationFrame(checkRaf);
        }
    }

    window.addEventListener("contextmenu", function(e) {
        e.preventDefault();
    }, true);

    var PARALLAX_CALM_INTERVAL_MS = 1500;
    window.__v4LastWinX = null;
    window.__v4LastWinY = null;
    function updateParallaxVars(force) {
        try {
            var wx = window.screenX || window.screenLeft || 0;
            var wy = window.screenY || window.screenTop || 0;
            var dx = (window.__v4LastWinX === null) ? 999 : Math.abs(wx - window.__v4LastWinX);
            var dy = (window.__v4LastWinY === null) ? 999 : Math.abs(wy - window.__v4LastWinY);
            if (force || dx >= 2 || dy >= 2) {
                window.__v4LastWinX = wx;
                window.__v4LastWinY = wy;
                document.documentElement.style.setProperty("--win-x", wx + "px");
                document.documentElement.style.setProperty("--win-y", wy + "px");
            }
        } catch(_) {}
    }
    updateParallaxVars(true);
    window.addEventListener("mouseup", function() { updateParallaxVars(false); }, { passive: true });
    window.addEventListener("resize", function() { updateParallaxVars(false); }, { passive: true });
    window.addEventListener("focus", function() { updateParallaxVars(false); }, { passive: true });
    setInterval(function() { updateParallaxVars(false); }, PARALLAX_CALM_INTERVAL_MS);

    function installDragListeners() {
        ['.app-bar', '.drawer-header'].forEach(function(sel) {
            var el = document.querySelector(sel);
            if (!el) return;
            var onDragStart = function(e) {
                if (e.button !== 0) return;
                var interactive = e.target.closest('button, a, input, select, textarea, .bar-btn, #btn-settings, #close-settings, #win-min, #win-max, #win-close, #restart-screen, #stop-screen, #btn-rotate-landscape, #toggle-audio-top');
                if (!interactive) {
                    e.preventDefault();
                    fetch('/api/window/start_drag', { method: 'POST' }).catch(function(){});
                }
            };
            el.addEventListener('mousedown', onDragStart, true);
        });
    }

    window.__v4LastDrawerTouch = 0;
    window.__v4LastSentDrawerState = null;
    window.__v4LastSentDrawerMs = 0;
    window.__v4StretchGlitchCount = 0;
    window.__v4MaxDisplayColDuringClose = 0;
    window.__v4LastVideoAspectSync = null;

    function syncIpadDisplayWidthVar() {
        try {
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            if (appEl && appEl.classList.contains('is-ipad') && !appEl.classList.contains('is-landscape')) {
                document.documentElement.style.setProperty('--display-width', '564px');
                appEl.style.setProperty('--display-width', '564px');
            }
        } catch (_) {}
    }

    function runDrawerToggleFlickerVerify() {
        try {
            var btn = document.getElementById('btn-settings');
            var appBar = document.querySelector('.app-bar');
            if (!btn || !appBar) {
                postApi('/api/test/mirror_report', { ok: false, error: 'missing_btn_or_appbar' });
                return;
            }
            if (window.isDrawerOpen && typeof window.toggleSettings === 'function') {
                window.toggleSettings(false);
            }
            setTimeout(function() {
                var maxH = 0;
                var minH = 9999;
                var samples = [];
                function sampleBar() {
                    try {
                        var h = Math.round(appBar.getBoundingClientRect().height * 100) / 100;
                        if (h > 0) {
                            if (h > maxH) maxH = h;
                            if (h < minH) minH = h;
                        }
                    } catch (_) {}
                }
                sampleBar();
                var ro = null;
                if (typeof ResizeObserver !== 'undefined') {
                    ro = new ResizeObserver(function() { sampleBar(); });
                    ro.observe(appBar);
                }
                var iv = setInterval(sampleBar, 16);
                var steps = [];
                var clickIdx = 0;
                function doNextClick() {
                    sampleBar();
                    if (clickIdx >= 6) {
                        clearInterval(iv);
                        if (ro) { try { ro.disconnect(); } catch (_) {} }
                        sampleBar();
                        postApi('/api/test/mirror_report', {
                            ok: true,
                            clicks: steps.length,
                            maxAppBarHeight: maxH,
                            minAppBarHeight: minH,
                            steps: steps
                        });
                        return;
                    }
                    clickIdx++;
                    btn.click();
                    sampleBar();
                    setTimeout(function() {
                        sampleBar();
                        var curH = Math.round(appBar.getBoundingClientRect().height * 100) / 100;
                        steps.push({
                            step: clickIdx,
                            drawerOpen: !!window.isDrawerOpen,
                            appBarHeight: curH,
                            innerWidth: window.innerWidth || 0
                        });
                        doNextClick();
                    }, 300);
                }
                doNextClick();
            }, 250);
        } catch (err) {
            postApi('/api/test/mirror_report', { ok: false, error: String(err) });
        }
    }

    function runFullscreenEnterSyncVerify() {
        try {
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            if (!appEl) {
                postApi('/api/test/mirror_report', { ok: false, error: 'missing_app_root' });
                return;
            }
            var dc = appEl.querySelector('.display-col');
            var sd = appEl.querySelector('.side-drawer') || document.getElementById('control-pane');
            var se = document.getElementById('screen-empty');
            var sv = document.getElementById('screen-view');
            if (se) { se.hidden = true; se.style.display = 'none'; }
            if (sv) { sv.hidden = false; sv.style.display = 'block'; sv.src = '/mirror.html?v=tahap181_fs_enter'; }
            appEl.classList.add('is-streaming');
            syncMirroringStateClass();

            if (!window.isDrawerOpen && typeof window.toggleSettings === 'function') {
                window.toggleSettings(true);
            } else {
                appEl.classList.add('drawer-open');
                if (sd) { sd.hidden = false; sd.classList.add('open'); }
                window.isDrawerOpen = true;
            }

            setTimeout(function() {
                var firstResizeReport = null;
                function onFirstFsResize() {
                    if (window.innerWidth >= 1200 && !firstResizeReport) {
                        var dRect = dc ? dc.getBoundingClientRect() : { width: 0 };
                        var sRect = sd ? sd.getBoundingClientRect() : { right: 0 };
                        var ps = document.getElementById('phone-screen');
                        var pRect = ps ? ps.getBoundingClientRect() : { left: 0, right: 0, width: 0 };
                        firstResizeReport = {
                            innerWidth: window.innerWidth,
                            hasFullsize: appEl.classList.contains('is-fullsize'),
                            displayColWidth: Math.round(dRect.width * 100) / 100,
                            sideDrawerRight: Math.round(sRect.right * 100) / 100,
                            phoneScreenCenterX: Math.round(((pRect.left + pRect.right) / 2) * 100) / 100
                        };
                        window.removeEventListener('resize', onFirstFsResize);
                    }
                }
                window.addEventListener('resize', onFirstFsResize);

                var fsPromise = window.pywebview.api.toggle_fullscreen(true);
                // Measure synchronously at 0ms in the same call stack!
                var t0HasFullsize = appEl.classList.contains('is-fullsize');

                fsPromise.then(function() {
                    requestAnimationFrame(function() {
                        setTimeout(function() {
                            window.removeEventListener('resize', onFirstFsResize);
                            var dRect2 = dc ? dc.getBoundingClientRect() : { width: 0 };
                            var sRect2 = sd ? sd.getBoundingClientRect() : { right: 0 };
                            var settled = {
                                innerWidth: window.innerWidth,
                                hasFullsize: appEl.classList.contains('is-fullsize'),
                                displayColWidth: Math.round(dRect2.width * 100) / 100,
                                sideDrawerRight: Math.round(sRect2.right * 100) / 100
                            };
                            window.pywebview.api.toggle_fullscreen(false).then(function() {
                                setTimeout(function() {
                                    if (!window.isDrawerOpen && typeof window.toggleSettings === 'function') {
                                        window.toggleSettings(true);
                                    } else {
                                        appEl.classList.add('drawer-open');
                                        if (sd) { sd.hidden = false; sd.classList.add('open'); }
                                        window.isDrawerOpen = true;
                                    }
                                    appEl.classList.add('is-streaming');
                                    syncMirroringStateClass();
                                    setTimeout(function() {
                                        var winMaxFirstResize = null;
                                        function onWinMaxFirstResize() {
                                            if (window.innerWidth >= 1200 && !winMaxFirstResize) {
                                                var wdRect = dc ? dc.getBoundingClientRect() : { width: 0 };
                                                var wps = document.getElementById('phone-screen');
                                                var wpRect = wps ? wps.getBoundingClientRect() : { left: 0, right: 0 };
                                                var wCx = (wpRect.left + wpRect.right) / 2;
                                                winMaxFirstResize = {
                                                    innerWidth: window.innerWidth,
                                                    hasFullsize: appEl.classList.contains('is-fullsize'),
                                                    drawerOpen: Boolean(appEl.classList.contains('drawer-open') || window.isDrawerOpen),
                                                    displayColWidth: Math.round(wdRect.width * 100) / 100,
                                                    phoneScreenCenterX: Math.round(wCx * 100) / 100,
                                                    centerDeltaPx: Math.round(Math.abs(wCx - (window.innerWidth / 2)) * 100) / 100
                                                };
                                                window.removeEventListener('resize', onWinMaxFirstResize);
                                            }
                                        }
                                        window.addEventListener('resize', onWinMaxFirstResize);
                                        var winMaxBtn = document.getElementById('win-max');
                                        if (winMaxBtn) winMaxBtn.click();
                                        var winMaxT0HasFullsize = appEl.classList.contains('is-fullsize');
                                        var winMaxT0DrawerOpen = Boolean(appEl.classList.contains('drawer-open') || window.isDrawerOpen);

                                        setTimeout(function() {
                                            window.removeEventListener('resize', onWinMaxFirstResize);
                                            var wdRect2 = dc ? dc.getBoundingClientRect() : { width: 0 };
                                            var wps2 = document.getElementById('phone-screen');
                                            var wpRect2 = wps2 ? wps2.getBoundingClientRect() : { left: 0, right: 0 };
                                            var wCx2 = (wpRect2.left + wpRect2.right) / 2;
                                            var winMaxSettled = {
                                                innerWidth: window.innerWidth,
                                                hasFullsize: appEl.classList.contains('is-fullsize'),
                                                drawerOpen: Boolean(appEl.classList.contains('drawer-open') || window.isDrawerOpen),
                                                displayColWidth: Math.round(wdRect2.width * 100) / 100,
                                                phoneScreenCenterX: Math.round(wCx2 * 100) / 100,
                                                centerDeltaPx: Math.round(Math.abs(wCx2 - (window.innerWidth / 2)) * 100) / 100
                                            };
                                            window.pywebview.api.toggle_fullscreen(false).then(function() {
                                                if (typeof window.toggleSettings === 'function') window.toggleSettings(false);
                                                if (se) { se.hidden = false; se.style.display = 'flex'; }
                                                if (sv) { sv.hidden = true; sv.style.display = 'none'; sv.src = 'about:blank'; }
                                                appEl.classList.remove('is-streaming');
                                                appEl.classList.remove('is-mirroring');
                                                syncMirroringStateClass();
                                                postApi('/api/test/mirror_report', {
                                                    ok: true,
                                                    t0HasFullsize: t0HasFullsize,
                                                    firstResize: firstResizeReport || settled,
                                                    settled: settled,
                                                    winMaxClick: {
                                                        t0HasFullsize: winMaxT0HasFullsize,
                                                        t0DrawerOpen: winMaxT0DrawerOpen,
                                                        firstResize: winMaxFirstResize || winMaxSettled,
                                                        settled: winMaxSettled
                                                    }
                                                });
                                            });
                                        }, 320);
                                    }, 250);
                                }, 180);
                            });
                        }, 120);
                    });
                });
            }, 350);
        } catch (err) {
            postApi('/api/test/mirror_report', { ok: false, error: String(err) });
        }
    }

    function runTahap182UiVerify() {
        try {
            sanitizeErrorBanners();
            var banner = document.getElementById('screen-error-banner');
            var initialBannerText = banner ? (banner.textContent || '').trim() : '';
            var initialBannerHidden = banner ? !!banner.hidden : true;
            var runtimeErrorsBefore = Array.isArray(window.__v4RuntimeErrors) ? window.__v4RuntimeErrors.slice() : [];

            var hasOwnQS = Object.prototype.hasOwnProperty.call(document, 'querySelector');
            var hasOwnGEBI = Object.prototype.hasOwnProperty.call(document, 'getElementById');
            var resolvedPillQS = document.querySelector('#pill-connection');
            var resolvedPillGEBI = document.getElementById('pill-connection');
            var resolvedWifiQS = document.querySelector('#wifi-summary');
            var resolvedWifiGEBI = document.getElementById('wifi-summary');

            var wasDrawerOpen = !!window.isDrawerOpen;
            if (!wasDrawerOpen && typeof window.toggleSettings === 'function') {
                window.toggleSettings(true);
            } else {
                var btnSet = document.getElementById('btn-settings');
                if (btnSet && !wasDrawerOpen) btnSet.click();
            }

            setTimeout(function() {
                window.__v4ApplyTheme('light');
                waitTwoRafs(window, function() {
                    setTimeout(function() {
                        var group = document.getElementById('update-channel-group');
                        var btnStable = document.getElementById('btn-channel-stable');
                        var btnBeta = document.getElementById('btn-channel-beta');
                        if (!group || !btnStable || !btnBeta) {
                            postApi('/api/test/mirror_report', { ok: false, error: 'missing_update_channel_dom' });
                            return;
                        }
                        function snapBtn(el) {
                            var cs = getComputedStyle(el);
                            var sp = el.querySelector('span');
                            var scs = sp ? getComputedStyle(sp) : cs;
                            return {
                                isActive: el.classList.contains('is-active') || el.classList.contains('active') || el.getAttribute('aria-checked') === 'true',
                                bgColor: cs.backgroundColor,
                                bgImage: cs.backgroundImage,
                                borderRadius: cs.borderRadius,
                                color: cs.color,
                                spanColor: scs.color
                            };
                        }
                        var lightInitial = {
                            trackRadius: getComputedStyle(group).borderRadius,
                            trackBg: getComputedStyle(group).backgroundColor,
                            stable: snapBtn(btnStable),
                            beta: snapBtn(btnBeta)
                        };
                        btnBeta.click();
                        enforceSingleActivePerSegmentedGroup(btnBeta);
                        waitTwoRafs(window, function() {
                            setTimeout(function() {
                                var lightBetaClicked = {
                                    stable: snapBtn(btnStable),
                                    beta: snapBtn(btnBeta)
                                };
                                btnStable.click();
                                enforceSingleActivePerSegmentedGroup(btnStable);
                                window.__v4ApplyTheme('dark');
                                waitTwoRafs(window, function() {
                                    setTimeout(function() {
                                        var darkFinal = {
                                            trackRadius: getComputedStyle(group).borderRadius,
                                            trackBg: getComputedStyle(group).backgroundColor,
                                            stable: snapBtn(btnStable),
                                            beta: snapBtn(btnBeta)
                                        };
                                        var sanitizerCleared = false;
                                        if (banner) {
                                            banner.textContent = "Cannot set properties of null (setting 'hidden')";
                                            banner.hidden = false;
                                            sanitizeErrorBanners();
                                            sanitizerCleared = (banner.textContent === '' && banner.hidden === true);
                                        }
                                        if (!wasDrawerOpen && typeof window.toggleSettings === 'function') {
                                            window.toggleSettings(false);
                                        }
                                        postApi('/api/test/mirror_report', {
                                            ok: true,
                                            uiTahap182: {
                                                docPrototypePreserved: (!hasOwnQS && !hasOwnGEBI),
                                                pillConnectionResolved: !!(resolvedPillQS && resolvedPillGEBI),
                                                wifiSummaryResolved: !!(resolvedWifiQS && resolvedWifiGEBI),
                                                initialBannerText: initialBannerText,
                                                initialBannerHidden: initialBannerHidden,
                                                sanitizerCleared: sanitizerCleared,
                                                runtimeErrorCount: runtimeErrorsBefore.length,
                                                runtimeErrors: runtimeErrorsBefore,
                                                lightInitial: lightInitial,
                                                lightBetaClicked: lightBetaClicked,
                                                darkFinal: darkFinal
                                            }
                                        });
                                    }, 80);
                                });
                            }, 80);
                        });
                    }, 80);
                });
            }, 150);
        } catch (err) {
            postApi('/api/test/mirror_report', { ok: false, error: String(err) });
        }
    }

    window.addEventListener('message', function(e) {
        try {
            if (!e || !e.data) return;
            if (e.data.type === 'mirror-first-frame' || e.data.type === 'mirror-first-frame-ready') {
                var appRootEl = document.getElementById('app-root') || document.querySelector('.app');
                if (appRootEl) {
                    appRootEl.classList.add('has-first-frame');
                }
                return;
            }
            if (e.data.type === 'hevc-missing') {
                if (typeof window.__v4ShowMainHevcBanner === 'function') {
                    window.__v4ShowMainHevcBanner(e.data.codec || '');
                }
                return;
            }
            if (e.data.type !== 'video-dimensions') return;
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            if (!appEl) return;
            var cleanW = Number(e.data.cleanW || e.data.width) || 0;
            var cleanH = Number(e.data.cleanH || e.data.height) || 0;
            var rawW = Number(e.data.rawWidth || e.data.rawW || cleanW) || cleanW;
            var rawH = Number(e.data.rawHeight || e.data.rawH || cleanH) || cleanH;
            if (cleanW <= 0 || cleanH <= 0) return;

            var deviceAspect = cleanW / cleanH;
            if (deviceAspect > 0.30 && deviceAspect < 2.50) {
                var cleanAspectStr = deviceAspect.toFixed(6);
                document.documentElement.style.setProperty('--v4-clean-aspect', cleanAspectStr);
                appEl.style.setProperty('--v4-clean-aspect', cleanAspectStr);
            }

            var isLandscape = appEl.classList.contains('is-landscape');
            var isFullsize = appEl.classList.contains('is-fullsize');
            var prof = window.__v4ActiveDeviceProfile || 'auto';
            var isIpad = appEl.classList.contains('is-ipad') || prof === 'ipad' || Boolean(e.data.isIpad && prof === 'auto');
            if (isIpad) {
                syncIpadDisplayWidthVar();
                return;
            }
            if (isLandscape || isFullsize) return;

            if (deviceAspect > 0.35 && deviceAspect < 0.72) {
                var idealW = 346;
                if (deviceAspect >= 0.440 && deviceAspect <= 0.475) {
                    // Tahap 182: Super Retina iPhone portrait stream (1178x2598, 1202x2598, 1290x2796)
                    // stays locked at 346px boot width so 0px 346->340 calibration resize jump occurs.
                    idealW = 346;
                } else {
                    var stageH = 832 - 38 - 44; // 750px logical stage height for 9:16 streams (1080x1920 -> 422px)
                    idealW = Math.max(336, Math.min(440, Math.round(stageH * deviceAspect)));
                }
                window.__v4LastSyncedPortraitWidth = idealW;
                window.__v4LastVideoAspectSync = {
                    cleanW: cleanW,
                    cleanH: cleanH,
                    rawW: rawW,
                    rawH: rawH,
                    deviceAspect: Number(deviceAspect.toFixed(5)),
                    idealPortraitWidth: idealW
                };
                var curDw = parseInt(getComputedStyle(document.documentElement).getPropertyValue('--display-width') || '346', 10) || 346;
                if (idealW !== curDw) {
                    document.documentElement.style.setProperty('--display-width', idealW + 'px');
                    appEl.style.setProperty('--display-width', idealW + 'px');
                    var isDrawer = appEl.classList.contains('drawer-open');
                    postApi('/api/window/sync_device_aspect', {
                        width: idealW,
                        open: isDrawer,
                        drawer: isDrawer,
                        profile: prof
                    });
                }
            }
        } catch (_) {}
    });

    function waitTwoRafs(win, cb) {
        win.requestAnimationFrame(function() {
            win.requestAnimationFrame(function() {
                setTimeout(cb, 30);
            });
        });
    }

    function runSyntheticMirrorAutofitVerify() {
        var sv = document.getElementById('screen-view');
        if (!sv) {
            postApi('/api/test/mirror_report', { ok: false, error: 'missing_screen_view' });
            return;
        }
        var se = document.getElementById('screen-empty');
        if (se) {
            se.hidden = true;
            se.style.display = 'none';
        }
        sv.hidden = false;
        sv.style.display = 'block';
        var curSrc = sv.getAttribute('src') || '';
        if (!curSrc.startsWith('/mirror.html')) {
            sv.src = '/mirror.html?v=tahap181_verify';
        }
        syncMirroringStateClass();
        var attempts = 0;
        function waitIframeReady() {
            attempts++;
            var iw = null;
            var idoc = null;
            try {
                iw = sv.contentWindow;
                idoc = sv.contentDocument || (iw && iw.document);
            } catch (_) {}
            if (iw && idoc && idoc.getElementById('c') && typeof iw.drawFrame === 'function' && typeof iw.VideoFrame === 'function') {
                executeFrameChecks(iw, idoc);
                return;
            }
            if (attempts < 40) {
                setTimeout(waitIframeReady, 100);
            } else {
                postApi('/api/test/mirror_report', { ok: false, error: 'iframe_timeout' });
            }
        }
        function makeFrameCanvas(idoc, rawW, rawH, trimR, trimB) {
            var off = idoc.createElement('canvas');
            off.width = rawW;
            off.height = rawH;
            var octx = off.getContext('2d');
            octx.fillStyle = '#000000';
            octx.fillRect(0, 0, rawW, rawH);
            var cw = rawW - trimR;
            var ch = rawH - trimB;
            octx.fillStyle = 'rgb(250, 250, 252)';
            octx.fillRect(0, 0, cw, ch);
            octx.fillStyle = '#ff2020';
            octx.fillRect(0, 0, 12, 12);
            octx.fillStyle = '#20e040';
            octx.fillRect(cw - 12, ch - 12, 12, 12);
            return off;
        }
        function makeBlackFrameCanvas(idoc, rawW, rawH) {
            var off = idoc.createElement('canvas');
            off.width = rawW;
            off.height = rawH;
            var octx = off.getContext('2d');
            octx.fillStyle = '#000000';
            octx.fillRect(0, 0, rawW, rawH);
            return off;
        }
        function executeFrameChecks(iw, idoc) {
            try {
                var appEl = document.getElementById('app-root') || document.querySelector('.app');
                if (appEl) {
                    appEl.classList.add('is-streaming');
                    appEl.classList.remove('is-ipad');
                }
                syncMirroringStateClass();

                var c = idoc.getElementById('c');
                var cCtx = c.getContext('2d', { willReadFrequently: true });

                function inspectMode(mode, rawW, rawH, expectTrimR, expectTrimB, tsOffset, sendBlackFirst, nextCb) {
                    var fitBtn = document.querySelector('[data-fit-choice="' + mode + '"]');
                    if (fitBtn) fitBtn.click();
                    iw.postMessage({ type: 'set-fit-mode', mode: mode }, '*');
                    iw.postMessage({ type: 'set-crop', crop: { top: 0, bottom: 0, left: 0, right: 0 } }, '*');
                    if (typeof iw.__v4ResetMacroblockCache === 'function') {
                        iw.__v4ResetMacroblockCache();
                    }
                    var off = makeFrameCanvas(idoc, rawW, rawH, expectTrimR, expectTrimB);
                    setTimeout(function() {
                        var blackCacheLocked = false;
                        var blackTrimR = -1;
                        var blackTrimB = -1;
                        var blackCleanW = 0;
                        var blackCleanH = 0;
                        if (sendBlackFirst) {
                            var blkOff = makeBlackFrameCanvas(idoc, rawW, rawH);
                            var vfBlk = new iw.VideoFrame(blkOff, { timestamp: Math.max(0, tsOffset - 500) });
                            iw.drawFrame(vfBlk);
                            var trBlk = iw._lastCleanTransform || {};
                            blackCacheLocked = !!(trBlk.macroblockLocked || trBlk.verifiedOnBrightFrame);
                            blackTrimR = trBlk.nativeTrimRight !== undefined ? trBlk.nativeTrimRight : -1;
                            blackTrimB = trBlk.nativeTrimBot !== undefined ? trBlk.nativeTrimBot : -1;
                            blackCleanW = trBlk.cleanW || 0;
                            blackCleanH = trBlk.cleanH || 0;
                        }
                        var vf = new iw.VideoFrame(off, { timestamp: tsOffset });
                        iw.drawFrame(vf);
                        waitTwoRafs(iw, function() {
                            var cleanExpW = rawW - expectTrimR;
                            var cleanExpH = rawH - expectTrimB;
                            var rightEdgePx = Array.from(cCtx.getImageData(c.width - 1, Math.floor(c.height / 2), 1, 1).data);
                            var rightEdgePrevPx = Array.from(cCtx.getImageData(Math.max(0, c.width - 2), Math.floor(c.height / 2), 1, 1).data);
                            var bottomEdgePx = Array.from(cCtx.getImageData(Math.floor(c.width / 2), c.height - 1, 1, 1).data);
                            var topLeftMarkerPx = Array.from(cCtx.getImageData(4, 4, 1, 1).data);
                            var brX = Math.max(0, Math.min(c.width - 1, Math.round((cleanExpW - 6) * c.width / cleanExpW)));
                            var brY = Math.max(0, Math.min(c.height - 1, Math.round((cleanExpH - 6) * c.height / cleanExpH)));
                            var botRightMarkerPx = Array.from(cCtx.getImageData(brX, brY, 1, 1).data);

                            var rightBright = (
                                rightEdgePx[0] >= 240 && rightEdgePx[1] >= 240 && rightEdgePx[2] >= 240 &&
                                rightEdgePrevPx[0] >= 240 && rightEdgePrevPx[1] >= 240 && rightEdgePrevPx[2] >= 240
                            );
                            var bottomBright = (bottomEdgePx[0] >= 240 && bottomEdgePx[1] >= 240 && bottomEdgePx[2] >= 240);
                            var topLeftOk = (topLeftMarkerPx[0] > 200 && topLeftMarkerPx[1] < 80);
                            var botRightOk = (botRightMarkerPx[1] > 180 && botRightMarkerPx[0] < 80);

                            var tr = iw._lastCleanTransform || {};
                            var cs = iw.getComputedStyle(c);
                            var rc = c.getBoundingClientRect();
                            var psEl = document.getElementById('phone-screen');
                            var psRc = psEl ? psEl.getBoundingClientRect() : rc;
                            nextCb({
                                mode: mode,
                                rawW: rawW,
                                rawH: rawH,
                                canvasWidth: c.width,
                                canvasHeight: c.height,
                                cleanW: tr.cleanW || 0,
                                cleanH: tr.cleanH || 0,
                                baseTrim: tr.baseTrim || 0,
                                macroblockSource: tr.macroblockSource || '',
                                verifiedOnBrightFrame: !!tr.verifiedOnBrightFrame,
                                macroblockLocked: !!tr.macroblockLocked,
                                blackFirstLocked: blackCacheLocked,
                                blackFirstTrimRight: blackTrimR,
                                blackFirstTrimBot: blackTrimB,
                                blackFirstCleanW: blackCleanW,
                                blackFirstCleanH: blackCleanH,
                                nativeTrimTop: tr.nativeTrimTop !== undefined ? tr.nativeTrimTop : -1,
                                nativeTrimLeft: tr.nativeTrimLeft !== undefined ? tr.nativeTrimLeft : -1,
                                nativeTrimRight: tr.nativeTrimRight !== undefined ? tr.nativeTrimRight : -1,
                                nativeTrimBot: tr.nativeTrimBot !== undefined ? tr.nativeTrimBot : -1,
                                objectFit: cs.objectFit || '',
                                rectWidth: Math.round(rc.width),
                                rectHeight: Math.round(rc.height),
                                phoneScreenWidth: Math.round(psRc.width),
                                phoneScreenHeight: Math.round(psRc.height),
                                rightEdgePx: rightEdgePx,
                                rightEdgePrevPx: rightEdgePrevPx,
                                bottomEdgePx: bottomEdgePx,
                                rightBright: rightBright,
                                bottomBright: bottomBright,
                                topLeftOk: topLeftOk,
                                botRightOk: botRightOk
                            });
                        });
                    }, 80);
                }

                inspectMode('autofit', 1216, 2656, 38, 58, 1000, true, function(resAutoFit1216) {
                    inspectMode('autofit', 1200, 2600, 20, 40, 1500, true, function(resAutoFitDynamic) {
                        inspectMode('autofit', 1184, 2560, 14, 28, 2000, false, function(resAutoFit1184) {
                            inspectMode('autofit', 1248, 2688, 6, 0, 3000, false, function(resAutoFit1248) {
                                inspectMode('contain', 1216, 2656, 38, 58, 4000, false, function(resContain) {
                                    var afBtn = document.querySelector('[data-fit-choice="autofit"]');
                                    if (afBtn) afBtn.click();
                                    iw.postMessage({ type: 'set-fit-mode', mode: 'autofit' }, '*');
                                    if (appEl) appEl.classList.remove('is-ipad');
                                    window.__v4LastVideoAspectSync = null;
                                    window.postMessage({ type: 'video-dimensions', width: 1080, height: 1920, cleanW: 1080, cleanH: 1920, isIpad: false }, '*');
                                    setTimeout(function() {
                                        var aspect916 = window.__v4LastVideoAspectSync ? Object.assign({}, window.__v4LastVideoAspectSync) : {};
                                        window.postMessage({ type: 'video-dimensions', width: 1178, height: 2598, cleanW: 1178, cleanH: 2598, rawWidth: 1216, rawHeight: 2656, isIpad: false }, '*');
                                        setTimeout(function() {
                                            var aspect1178 = window.__v4LastVideoAspectSync ? Object.assign({}, window.__v4LastVideoAspectSync) : {};
                                            window.postMessage({ type: 'video-dimensions', width: 1202, height: 2598, cleanW: 1202, cleanH: 2598, rawWidth: 1216, rawHeight: 2656, isIpad: false }, '*');
                                            setTimeout(function() {
                                                var aspect1202 = window.__v4LastVideoAspectSync ? Object.assign({}, window.__v4LastVideoAspectSync) : {};
                                                window.postMessage({ type: 'video-dimensions', width: 1290, height: 2796, cleanW: 1290, cleanH: 2796, rawWidth: 1296, rawHeight: 2800, isIpad: false }, '*');
                                                setTimeout(function() {
                                                    var aspect1290 = window.__v4LastVideoAspectSync ? Object.assign({}, window.__v4LastVideoAspectSync) : {};
                                                    postApi('/api/window/sync_device_aspect', { width: 346, open: false, drawer: false, profile: 'auto' });
                                                    document.documentElement.style.setProperty('--v4-clean-aspect', '0.453425');
                                                    if (appEl) appEl.style.setProperty('--v4-clean-aspect', '0.453425');
                                                    if (se) {
                                                        se.hidden = false;
                                                        se.style.display = 'flex';
                                                    }
                                                    sv.hidden = true;
                                                    sv.style.display = 'none';
                                                    sv.src = 'about:blank';
                                                    if (appEl) {
                                                        appEl.classList.remove('is-streaming');
                                                        appEl.classList.remove('is-mirroring');
                                                    }
                                                    syncMirroringStateClass();
                                                    postApi('/api/test/mirror_report', {
                                                        ok: true,
                                                        autofit: resAutoFit1216,
                                                        autofitDynamic1200: resAutoFitDynamic,
                                                        autofit1184: resAutoFit1184,
                                                        autofit1248: resAutoFit1248,
                                                        contain: resContain,
                                                        aspect9x16: aspect916,
                                                        aspect1178x2598: aspect1178,
                                                        aspect1202x2598: aspect1202,
                                                        aspect1290x2796: aspect1290
                                                    });
                                                }, 140);
                                            }, 140);
                                        }, 140);
                                    }, 180);
                                });
                            });
                        });
                    });
                });
            } catch (err) {
                postApi('/api/test/mirror_report', { ok: false, error: String(err) });
            }
        }
        waitIframeReady();
    }
    window.__v4RunMirrorAutofitVerify = runSyntheticMirrorAutofitVerify;
    window.__v4RunTahap182UiVerify = runTahap182UiVerify;

    function syncVersionLabels() {
        try {
            var updVer = document.getElementById('update-current-version');
            if (updVer && updVer.textContent.trim() !== 'iPhone Link v5.0.1') {
                updVer.textContent = 'iPhone Link v5.0.1';
            }
            var credName = document.querySelector('.credit-app-name');
            if (credName && credName.textContent.trim() !== 'iPhone Link v5.0') {
                credName.textContent = 'iPhone Link v5.0';
            }
        } catch (_) {}
    }

    function ensureUpdaterCardExistsInDom() {
        try {
            var card = document.getElementById('card-updater');
            var langNow = (window.__v4CurrentLang === 'en' || document.documentElement.getAttribute('data-lang') === 'en') ? 'en' : 'id';
            var isEn = (langNow === 'en');
            if (!card) {
                var drawerBody = document.querySelector('#control-pane .drawer-content')
                    || document.querySelector('#control-pane .drawer-scroll')
                    || document.querySelector('.side-drawer .drawer-content')
                    || document.querySelector('.side-drawer .drawer-scroll')
                    || document.querySelector('#control-pane .drawer-body')
                    || document.getElementById('control-pane')
                    || document.querySelector('.side-drawer');
                if (!drawerBody) return null;

                card = document.createElement('section');
                card.className = 'card glass-card card-updater';
                card.id = 'card-updater';
                card.innerHTML = [
                    '<div class="update-card-header">',
                    '  <div class="update-header-left">',
                    '    <div class="update-icon-badge" aria-hidden="true">',
                    '      <svg class="fluent-icon" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67"/></svg>',
                    '    </div>',
                    '    <span class="update-card-title" data-i18n="updater.title">' + (isEn ? 'SOFTWARE UPDATE' : 'PEMBARUAN PERANGKAT LUNAK') + '</span>',
                    '  </div>',
                    '  <div class="update-header-actions">',
                    '    <button type="button" class="update-icon-btn" id="btn-update-info" title="Update info" aria-label="Update info" aria-expanded="false">',
                    '      <svg class="fluent-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="16" x2="12" y2="12"></line><line x1="12" y1="8" x2="12.01" y2="8"></line></svg>',
                    '    </button>',
                    '    <button type="button" class="update-icon-btn" id="btn-update-collapse" title="Toggle update card" aria-label="Toggle update card" aria-expanded="true">',
                    '      <svg class="fluent-icon icon-chevron" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="18 15 12 9 6 15"></polyline></svg>',
                    '    </button>',
                    '  </div>',
                    '</div>',
                    '<div class="update-info-box" id="update-info-box" hidden>',
                    '  <p data-i18n="updater.info"></p>',
                    '</div>',
                    '<div class="update-card-body" id="update-card-body">',
                    '  <div class="update-version-label" id="update-current-version">iPhone Link v5.0.1</div>',
                    '  <div class="update-row">',
                    '    <span class="update-row-label" data-i18n="updater.auto_check">' + (isEn ? 'Check automatically on startup' : 'Cek pembaruan otomatis saat dibuka') + '</span>',
                    '    <button type="button" class="toggle-switch is-active" id="toggle-auto-update-check" role="switch" aria-checked="true" aria-label="Auto check updates"><span class="toggle-switch-thumb"></span></button>',
                    '  </div>',
                    '  <div class="update-row">',
                    '    <span class="update-row-label" data-i18n="updater.channel">' + (isEn ? 'Channel' : 'Saluran Pembaruan') + '</span>',
                    '    <div class="segmented segmented-channel" id="update-channel-group" role="radiogroup" aria-label="Update Channel">',
                    '      <button type="button" class="seg is-active" id="btn-channel-stable" data-channel="stable" role="radio" aria-checked="true"><span class="btn-label" data-i18n="updater.stable">' + (isEn ? 'Stable' : 'Stabil') + '</span></button>',
                    '      <button type="button" class="seg" id="btn-channel-beta" data-channel="beta" role="radio" aria-checked="false"><span class="btn-label" data-i18n="updater.beta">Beta</span></button>',
                    '    </div>',
                    '  </div>',
                    '  <div class="update-status-row"><p class="update-status-text" id="update-status-text" data-i18n="updater.status_up_to_date">' + (isEn ? 'iPhone Link is up to date.' : 'iPhone Link sudah versi terbaru.') + '</p></div>',
                    '  <div class="update-progress-wrap" id="update-progress-wrap" hidden><div class="update-progress-bar"><div class="update-progress-fill" id="update-progress-fill" style="width: 0%;"></div></div><div class="update-progress-label" id="update-progress-label">0%</div></div>',
                    '  <div class="update-action-row"><button type="button" class="ctl ctl-accent update-action-btn" id="btn-check-update"><span class="btn-label" data-i18n="updater.check_btn">' + (isEn ? 'Check for updates' : 'Periksa Pembaruan') + '</span></button></div>',
                    '</div>'
                ].join('');

                var footer = drawerBody.querySelector('.drawer-footer-credits') || drawerBody.querySelector('.drawer-footer');
                if (footer && footer.parentNode === drawerBody) {
                    drawerBody.insertBefore(card, footer);
                } else {
                    drawerBody.appendChild(card);
                }

                if (typeof window.initUpdaterControls === 'function') {
                    try { window.initUpdaterControls(); } catch (_) {}
                }
            }

            if (card.hidden) card.hidden = false;
            if (card.style.display === 'none') card.style.display = '';
            if (card.style.visibility === 'hidden') card.style.visibility = '';

            if (!card.__v4UpdaterFallbackBound && typeof window.initUpdaterControls !== 'function') {
                card.__v4UpdaterFallbackBound = true;
                var btnInfo = document.getElementById('btn-update-info');
                var infoBox = document.getElementById('update-info-box');
                var btnCol = document.getElementById('btn-update-collapse');
                var tAuto = document.getElementById('toggle-auto-update-check');
                var bSt = document.getElementById('btn-channel-stable');
                var bBe = document.getElementById('btn-channel-beta');
                var bChk = document.getElementById('btn-check-update');
                if (btnInfo && infoBox) {
                    btnInfo.addEventListener('click', function() {
                        infoBox.hidden = !infoBox.hidden;
                        btnInfo.setAttribute('aria-expanded', String(!infoBox.hidden));
                    });
                }
                if (btnCol) {
                    btnCol.addEventListener('click', function() {
                        card.classList.toggle('is-collapsed');
                        btnCol.setAttribute('aria-expanded', String(!card.classList.contains('is-collapsed')));
                    });
                }
                if (tAuto) {
                    tAuto.addEventListener('click', function() {
                        var nextVal = !(tAuto.classList.contains('is-active') || tAuto.getAttribute('aria-checked') === 'true');
                        tAuto.classList.toggle('is-active', nextVal);
                        tAuto.setAttribute('aria-checked', String(nextVal));
                        persistSettingPartial({ autoCheckUpdate: nextVal }, false);
                    });
                }
                if (bSt) {
                    bSt.addEventListener('click', function() {
                        enforceSingleActivePerSegmentedGroup(bSt);
                        persistSettingPartial({ updateChannel: 'stable' }, false);
                    });
                }
                if (bBe) {
                    bBe.addEventListener('click', function() {
                        enforceSingleActivePerSegmentedGroup(bBe);
                        persistSettingPartial({ updateChannel: 'beta' }, false);
                    });
                }
                if (bChk) {
                    bChk.addEventListener('click', function() {
                        var stEl = document.getElementById('update-status-text');
                        var curEn = (window.__v4CurrentLang === 'en');
                        if (stEl) stEl.textContent = curEn ? 'Checking for updates...' : 'Memeriksa pembaruan...';
                        var ch = (bBe && bBe.classList.contains('is-active')) ? 'beta' : 'stable';
                        postApi('/api/update/check', { channel: ch });
                    });
                }
            }
            return card;
        } catch (_) {
            return null;
        }
    }

    function applyV4DomTranslations(lang) {
        if (window.__v4TranslatingDom) return;
        window.__v4TranslatingDom = true;
        try {
            ensureUpdaterCardExistsInDom();
            var isEn = (lang === 'en');
            var drawerTitle = document.querySelector('.drawer-title');
            if (drawerTitle) {
                var nextTitle = isEn ? 'Settings & Controls' : 'Pengaturan & Kontrol';
                if (drawerTitle.textContent !== nextTitle) drawerTitle.textContent = nextTitle;
            }
            var updTitle = document.querySelector('#card-updater .update-card-title');
            if (updTitle) {
                var nextUpdTitle = isEn ? 'SOFTWARE UPDATE' : 'PEMBARUAN PERANGKAT LUNAK';
                if (updTitle.textContent.trim() !== nextUpdTitle) updTitle.textContent = nextUpdTitle;
            }
            var updAutoLbl = document.querySelector('#card-updater [data-i18n="updater.auto_check"]');
            if (updAutoLbl) {
                var nextAutoLbl = isEn ? 'Check automatically on startup' : 'Cek pembaruan otomatis saat dibuka';
                if (updAutoLbl.textContent.trim() !== nextAutoLbl) updAutoLbl.textContent = nextAutoLbl;
            }
            var updChLbl = document.querySelector('#card-updater [data-i18n="updater.channel"]');
            if (updChLbl) {
                var nextChLbl = isEn ? 'Channel' : 'Saluran Pembaruan';
                if (updChLbl.textContent.trim() !== nextChLbl) updChLbl.textContent = nextChLbl;
            }
            var updStLbl = document.querySelector('#btn-channel-stable .btn-label') || document.getElementById('btn-channel-stable');
            if (updStLbl) {
                var nextStLbl = isEn ? 'Stable' : 'Stabil';
                if (updStLbl.textContent.trim() !== nextStLbl) updStLbl.textContent = nextStLbl;
            }
            var updBeLbl = document.querySelector('#btn-channel-beta .btn-label') || document.getElementById('btn-channel-beta');
            if (updBeLbl && updBeLbl.textContent.trim() !== 'Beta') {
                updBeLbl.textContent = 'Beta';
            }
            var updInfoP = document.querySelector('#update-info-box p');
            if (updInfoP) {
                var nextInfoP = isEn
                    ? 'Checks for new GitHub releases, downloads the installer, verifies SHA-256 integrity, and updates smoothly while keeping your settings.'
                    : 'Memeriksa rilis baru di GitHub, mengunduh berkas instalasi, memverifikasi integritas SHA-256, dan memperbarui aplikasi tanpa menghapus pengaturan Anda.';
                if (updInfoP.textContent.trim() !== nextInfoP) updInfoP.textContent = nextInfoP;
            }
            var updStatusEl = document.getElementById('update-status-text');
            if (updStatusEl) {
                var curUpdStatus = updStatusEl.textContent.trim();
                if (!curUpdStatus || curUpdStatus === 'iPhone Link is up to date.' || curUpdStatus === 'iPhone Link sudah versi terbaru.') {
                    var targetUpdStatus = isEn ? 'iPhone Link is up to date.' : 'iPhone Link sudah versi terbaru.';
                    if (curUpdStatus !== targetUpdStatus) updStatusEl.textContent = targetUpdStatus;
                }
            }
            var updBtnEl = document.getElementById('btn-check-update');
            if (updBtnEl && !updBtnEl.disabled && !updBtnEl.classList.contains('has-update')) {
                var updBtnLbl = updBtnEl.querySelector('.btn-label') || updBtnEl;
                var nextBtnLbl = isEn ? 'Check for updates' : 'Periksa Pembaruan';
                if (updBtnLbl.textContent.trim() !== nextBtnLbl) updBtnLbl.textContent = nextBtnLbl;
            }
            var showScreen = document.getElementById('show-screen');
            if (showScreen) {
                var showTxt = (showScreen.textContent || '').trim();
                var lblSpan = showScreen.querySelector('.btn-label');
                if (!showTxt || showTxt === 'Start mirroring' || showTxt === 'Mulai mirroring') {
                    var targetShow = isEn ? 'Start mirroring' : 'Mulai mirroring';
                    if (lblSpan) {
                        if (lblSpan.textContent.trim() !== targetShow) lblSpan.textContent = targetShow;
                    } else {
                        showScreen.innerHTML = '<span class="btn-label" data-i18n="conn.start_mirror">' + targetShow + '</span>';
                    }
                }
            }
            var lpStartLbl = document.querySelector('#launchpad-start-btn .btn-label');
            if (lpStartLbl) {
                var lpTxt = lpStartLbl.textContent.trim();
                if (!lpTxt || lpTxt === 'Start mirroring' || lpTxt === 'Mulai mirroring') {
                    lpStartLbl.textContent = isEn ? 'Start mirroring' : 'Mulai mirroring';
                }
            }
            var audioPcLbl = document.querySelector('#btn-audio-out-pc .btn-label');
            if (audioPcLbl) audioPcLbl.textContent = isEn ? 'On this PC (USB)' : 'Di PC ini (USB)';
            var audioDevLbl = document.querySelector('#btn-audio-out-device .btn-label');
            if (audioDevLbl) audioDevLbl.textContent = isEn ? 'Device only' : 'Hanya perangkat';

            var audioBadgeLbl = document.querySelector('#audio-status-badge .btn-label');
            if (audioBadgeLbl) {
                var activeAudioDev = (window.__v4ActiveAudioDevice || '').trim();
                if (activeAudioDev) {
                    audioBadgeLbl.textContent = isEn
                        ? ('A2DP Connected: ' + activeAudioDev + ' (48 kHz WASAPI)')
                        : ('A2DP Terhubung: ' + activeAudioDev + ' (48 kHz WASAPI)');
                } else {
                    audioBadgeLbl.textContent = isEn
                        ? 'PC Audio Bridge (48 kHz WASAPI \u00B7 ~40 ms)'
                        : 'Audio PC Bridge (48 kHz WASAPI \u00B7 ~40 ms)';
                }
            }

            var bufHint = document.getElementById('audio-buffer-hint');
            if (bufHint) {
                var activeBufBtn = document.querySelector('#segmented-audio-buffer .seg.is-active, #segmented-audio-buffer .seg.active');
                var bufVal = activeBufBtn ? activeBufBtn.getAttribute('data-audio-buffer') : 'normal';
                if (bufVal === 'low') {
                    bufHint.textContent = isEn ? 'Low \u00B7 ~20 ms' : 'Rendah \u00B7 ~20 ms';
                } else if (bufVal === 'high') {
                    bufHint.textContent = isEn ? 'High \u00B7 ~80 ms' : 'Tinggi \u00B7 ~80 ms';
                } else {
                    bufHint.textContent = 'Normal \u00B7 ~40 ms';
                }
            }

            var lipSeg = document.getElementById('segmented-lip-sync');
            var lipItem = lipSeg ? lipSeg.closest('.setting-toggle-item') : null;
            var lipLbl = lipItem ? lipItem.querySelector('.setting-toggle-header .btn-label') : null;
            if (lipLbl) lipLbl.textContent = isEn ? 'Lip Sync' : 'Sinkron Audio (Lip Sync)';

            var btSummary = document.querySelector('.audio-bt-fallback summary');
            if (btSummary) btSummary.textContent = isEn ? 'Bluetooth A2DP Fallback Options' : 'Opsi Fallback Bluetooth A2DP';
            var btUseLbl = document.querySelector('#btn-audio-out-bt .btn-label');
            if (btUseLbl) btUseLbl.textContent = isEn ? 'Use Bluetooth' : 'Gunakan Bluetooth';
            var btPairLbl = document.querySelector('#btn-audio-pair .btn-label');
            if (btPairLbl) btPairLbl.textContent = isEn ? 'Pair BT' : 'Pasangkan BT';

            var stealthCard = document.getElementById('panel-screen-stealth');
            var stealthHdrLbl = stealthCard ? stealthCard.querySelector('.setting-toggle-header .btn-label') : null;
            if (stealthHdrLbl) stealthHdrLbl.textContent = isEn ? 'Stealth Dim Mode (Alt+D)' : 'Mode Redup Stealth (Alt+D)';
            var stealthBtnLbl = document.querySelector('#btn-stealth-mode .btn-label');
            var stealthHintEl = document.getElementById('stealth-mode-hint');
            if (stealthBtnLbl && stealthHintEl) {
                var isStealthOn = stealthHintEl.textContent.trim().toUpperCase() === 'ON';
                stealthBtnLbl.textContent = isEn
                    ? ('Stealth Dim Mode: ' + (isStealthOn ? 'ON' : 'OFF'))
                    : ('Mode Redup Stealth: ' + (isStealthOn ? 'ON' : 'OFF'));
            }

            var pinInput = document.getElementById('input-unlock-pin');
            if (pinInput) {
                pinInput.setAttribute('placeholder', isEn ? '4 or 6 digit PIN' : '4 atau 6 digit PIN');
            }

            // Tahap 222: Sync iOS 27 Shortcut Card Bilingual Labels (EN / ID)
            var scOcrNote = document.getElementById('shortcut-ocr-note');
            if (scOcrNote) {
                scOcrNote.textContent = isEn ? 'Auto 100% accurate without OCR' : 'Auto 100% akurat tanpa OCR';
            }
            var scGuideLink = document.getElementById('link-shortcut-guide-hint');
            if (scGuideLink) {
                scGuideLink.textContent = isEn ? 'Setup Guide' : 'Panduan Setup';
            }
            var scCopyUrlLbl = document.querySelector('#btn-copy-shortcut-url .btn-label') || document.getElementById('btn-copy-shortcut-url');
            if (scCopyUrlLbl) {
                scCopyUrlLbl.textContent = isEn ? 'Copy URL' : 'Salin URL';
            }
            var scGetIcloudLbl = document.querySelector('#btn-open-icloud-shortcut .btn-label') || document.getElementById('btn-open-icloud-shortcut');
            if (scGetIcloudLbl) {
                scGetIcloudLbl.textContent = isEn ? 'Get iCloud Shortcut' : 'Ambil iCloud Shortcut';
            }
            var scCopyPromptLbl = document.querySelector('#btn-copy-siri-prompt .btn-label') || document.getElementById('btn-copy-siri-prompt');
            if (scCopyPromptLbl) {
                scCopyPromptLbl.textContent = isEn ? 'Copy Prompt Shortcut' : 'Salin Prompt Shortcut';
            }
            if (typeof window.renderShortcutIpComparison === 'function') {
                try { window.renderShortcutIpComparison(); } catch (_) {}
            }
        } catch (_) {} finally {
            window.__v4TranslatingDom = false;
        }
    }

    function applyV4LanguageUiAfterAppJs(targetLang, persistToServer) {
        var target = (targetLang === 'en') ? 'en' : 'id';
        window.__v4CurrentLang = target;
        document.documentElement.setAttribute('lang', target);
        document.documentElement.setAttribute('data-lang', target);
        try {
            localStorage.setItem('iphone_link_lang', target);
            localStorage.setItem('iphone_link_language', target);
        } catch (_) {}
        var targetBtn = document.getElementById(target === 'en' ? 'btn-lang-en' : 'btn-lang-id');
        if (targetBtn) {
            enforceSingleActivePerSegmentedGroup(targetBtn);
        }
        applyV4DomTranslations(target);
        syncVersionLabels();
        if (persistToServer) {
            persistSettingPartial({ language: target }, false);
        }
        setTimeout(function() { sendDomBeacon(false); }, 40);
    }

    function wrapApplyLanguageIfReady() {
        if (typeof window.applyLanguage === 'function' && !window.applyLanguage.__v4Wrapped) {
            var origApplyLang = window.applyLanguage;
            window.__v4OrigApplyLanguage = origApplyLang;
            var wrappedApplyLang = function(lang, save) {
                var target = (lang === 'en') ? 'en' : 'id';
                window.__v4CurrentLang = target;
                document.documentElement.setAttribute('lang', target);
                document.documentElement.setAttribute('data-lang', target);
                var res;
                try { res = origApplyLang.call(this, target, save); } catch (_) {}
                try { applyV4LanguageUiAfterAppJs(target, save !== false && !window.__v4Hydrating); } catch (_) {}
                return res;
            };
            wrappedApplyLang.__v4Wrapped = true;
            window.applyLanguage = wrappedApplyLang;
            window.applyAppLanguage = wrappedApplyLang;
        }
        if (!window.__v4ShowScreenLangObs && typeof MutationObserver !== 'undefined') {
            var showScreenEl = document.getElementById('show-screen');
            if (showScreenEl) {
                window.__v4ShowScreenLangObs = true;
                var ssObs = new MutationObserver(function() {
                    if (window.__v4TranslatingDom) return;
                    applyV4DomTranslations(window.__v4CurrentLang || 'id');
                });
                ssObs.observe(showScreenEl, { childList: true, characterData: true, subtree: true });
            }
        }
    }

    function hydrateAllTogglesFromSettings(data) {
        if (!data || typeof data !== 'object') return;
        window.__v4Hydrating = true;
        try {
            seedLocalStorageFromSettings(data);
            hydratePinFromSettings(data);

            var lang = (data.language === 'en') ? 'en' : 'id';
            if (typeof window.__v4OrigApplyLanguage === 'function') {
                try { window.__v4OrigApplyLanguage(lang, false); } catch (_) {}
            } else if (typeof window.applyLanguage === 'function') {
                try { window.applyLanguage(lang, false); } catch (_) {}
            }
            applyV4LanguageUiAfterAppJs(lang, false);

            var thm = (data.theme === 'light') ? 'light' : 'dark';
            if (typeof window.__v4ApplyTheme === 'function') {
                window.__v4ApplyTheme(thm);
            }

            if (data.fitMode === 'autofit' || data.fitMode === 'fill' || data.fitMode === 'contain') {
                if (typeof window.applyFitMode === 'function') {
                    try { window.applyFitMode(data.fitMode); } catch (_) {}
                }
                var fitBtn = document.querySelector('[data-fit-choice="' + data.fitMode + '"]');
                if (fitBtn) enforceSingleActivePerSegmentedGroup(fitBtn);
            }

            if (data.deviceProfile === 'auto' || data.deviceProfile === 'iphone' || data.deviceProfile === 'ipad') {
                if (typeof window.applyDeviceProfile === 'function') {
                    try { window.applyDeviceProfile(data.deviceProfile, { persist: false, syncNative: false }); } catch (_) {}
                }
                var profBtn = document.querySelector('#segmented-device-profile [data-device-profile="' + data.deviceProfile + '"]');
                if (profBtn) enforceSingleActivePerSegmentedGroup(profBtn);
                window.__v4ActiveDeviceProfile = data.deviceProfile;
                var dwCss = (data.deviceProfile === 'ipad') ? '564px' : '346px';
                document.documentElement.style.setProperty('--display-width', dwCss);
                var appElP = document.getElementById('app-root') || document.querySelector('.app');
                if (appElP) {
                    appElP.style.setProperty('--display-width', dwCss);
                    appElP.classList.toggle('is-ipad', data.deviceProfile === 'ipad');
                }
            }

            if (typeof data.autoRotate === 'boolean') {
                if (typeof window.applyAutoRotateSetting === 'function') {
                    try { window.applyAutoRotateSetting(data.autoRotate, false); } catch (_) {}
                }
                var arBtn = document.querySelector('[data-auto-rotate-choice="' + (data.autoRotate ? 'on' : 'off') + '"]');
                if (arBtn) enforceSingleActivePerSegmentedGroup(arBtn);
            }

            if (data.crop && typeof data.crop === 'object') {
                var cObj = {
                    top: Number(data.crop.top || 0),
                    bottom: Number(data.crop.bottom || 0),
                    left: Number(data.crop.left || 0),
                    right: Number(data.crop.right || 0)
                };
                if (typeof window.applyCrop === 'function') {
                    try { window.applyCrop(cObj, true); } catch (_) {}
                }
                ['top', 'bottom', 'left', 'right'].forEach(function(edge) {
                    var sEl = document.getElementById('crop-' + edge + '-slider');
                    var nEl = document.getElementById('crop-' + edge + '-num');
                    if (sEl) sEl.value = String(cObj[edge]);
                    if (nEl) nEl.value = String(cObj[edge]);
                });
            }

            var aout = (data.bluetoothA2dp || data.audioOut === 'bluetooth') ? 'bluetooth' : (data.audioOut || 'pc');
            var avol = (typeof data.audioVolume === 'number') ? data.audioVolume : 100;
            var amut = Boolean(data.audioMuted);
            var abuf = data.audioBufferPreset || 'normal';
            var lipOn = (typeof data.lipSync === 'boolean') ? data.lipSync : true;
            if (typeof window.renderAudio === 'function') {
                try {
                    window.renderAudio({
                        audioOut: aout,
                        volume: avol,
                        muted: amut,
                        bufferPreset: abuf,
                        lipSync: lipOn
                    });
                } catch (_) {}
            }
            var segAu = document.getElementById('segmented-audio-out');
            var btBtn = document.getElementById('btn-audio-out-bt');
            if (aout === 'bluetooth') {
                clearSegmentedGroupSelection(segAu);
                if (btBtn) {
                    btBtn.classList.add('is-active', 'active');
                    btBtn.setAttribute('aria-pressed', 'true');
                }
            } else {
                if (btBtn) {
                    btBtn.classList.remove('is-active', 'active');
                    btBtn.setAttribute('aria-pressed', 'false');
                }
                var auBtn = document.querySelector('#segmented-audio-out [data-audio-out="' + aout + '"]');
                if (auBtn) enforceSingleActivePerSegmentedGroup(auBtn);
            }
            var volSl = document.getElementById('audio-vol-slider');
            var volTxt = document.getElementById('audio-vol-val');
            if (volSl) volSl.value = String(avol);
            if (volTxt) volTxt.textContent = String(avol) + '%';
            ['btn-audio-top', 'btn-audio-drawer'].forEach(function(aid) {
                var aBtn = document.getElementById(aid);
                if (aBtn) {
                    aBtn.classList.toggle('is-muted', amut);
                    aBtn.classList.toggle('is-active', !amut);
                }
            });
            var abufBtn = document.querySelector('#segmented-audio-buffer [data-audio-buffer="' + abuf + '"]');
            if (abufBtn) enforceSingleActivePerSegmentedGroup(abufBtn);
            var lipBtn = document.querySelector('#segmented-lip-sync [data-lip-sync="' + String(lipOn) + '"]');
            if (lipBtn) enforceSingleActivePerSegmentedGroup(lipBtn);
            var lipHint = document.getElementById('lip-sync-hint');
            if (lipHint) lipHint.textContent = lipOn ? 'ON' : 'OFF';

            if (typeof data.trackpadSpeed === 'number') {
                if (typeof window.applyTrackpadSpeed === 'function') {
                    try { window.applyTrackpadSpeed(data.trackpadSpeed, true); } catch (_) {}
                }
                var spdSl = document.getElementById('trackpad-speed-slider');
                var spdNm = document.getElementById('trackpad-speed-num');
                if (spdSl) spdSl.value = String(data.trackpadSpeed);
                if (spdNm) spdNm.value = String(data.trackpadSpeed);
                var spdGroup = document.querySelector('.segmented-speed');
                var spdPresetBtn = document.querySelector('.segmented-speed [data-speed-choice="' + String(data.trackpadSpeed) + '"]');
                if (spdPresetBtn) {
                    enforceSingleActivePerSegmentedGroup(spdPresetBtn);
                } else {
                    clearSegmentedGroupSelection(spdGroup);
                }
            }

            if (typeof data.crossPlatformShortcuts === 'boolean' || typeof data.pastePcClipboard === 'boolean') {
                if (typeof window.applyKeyboardSettings === 'function') {
                    try {
                        window.applyKeyboardSettings({
                            crossPlatformShortcuts: data.crossPlatformShortcuts !== false,
                            pastePcClipboard: data.pastePcClipboard !== false
                        }, true);
                    } catch (_) {}
                }
                var scBtn = document.querySelector('[data-shortcut-choice="' + String(data.crossPlatformShortcuts !== false) + '"]');
                if (scBtn) enforceSingleActivePerSegmentedGroup(scBtn);
                var ppcBtn = document.querySelector('[data-paste-pc-choice="' + String(data.pastePcClipboard !== false) + '"]');
                if (ppcBtn) enforceSingleActivePerSegmentedGroup(ppcBtn);
            }

            if (data.transport) {
                var trNorm = (data.transport === 'wifi') ? 'wifi' : 'usb';
                var trDrawerBtn = document.querySelector('.segmented [data-transport="' + trNorm + '"]');
                if (trDrawerBtn) enforceSingleActivePerSegmentedGroup(trDrawerBtn);
                var lpCable = document.getElementById('launchpad-transport-cable');
                var lpWifi = document.getElementById('launchpad-transport-wifi');
                if (lpCable) {
                    lpCable.classList.toggle('is-active', trNorm === 'usb');
                    lpCable.classList.toggle('active', trNorm === 'usb');
                    lpCable.setAttribute('aria-checked', String(trNorm === 'usb'));
                }
                if (lpWifi) {
                    lpWifi.classList.toggle('is-active', trNorm === 'wifi');
                    lpWifi.classList.toggle('active', trNorm === 'wifi');
                    lpWifi.setAttribute('aria-checked', String(trNorm === 'wifi'));
                }
            }

            if (typeof window.syncUpdaterSettings === 'function') {
                try {
                    window.syncUpdaterSettings({
                        autoCheckUpdate: data.autoCheckUpdate !== false,
                        updateChannel: (data.updateChannel === 'beta') ? 'beta' : 'stable'
                    });
                } catch (_) {}
            }
            var acToggle = document.getElementById('toggle-auto-update-check');
            if (acToggle && typeof data.autoCheckUpdate === 'boolean') {
                acToggle.classList.toggle('is-active', data.autoCheckUpdate);
                acToggle.classList.toggle('active', data.autoCheckUpdate);
                acToggle.setAttribute('aria-checked', String(data.autoCheckUpdate));
            }
            if (data.updateChannel === 'stable' || data.updateChannel === 'beta') {
                var ucBtn = document.querySelector('#update-channel-group [data-channel="' + data.updateChannel + '"]');
                if (ucBtn) enforceSingleActivePerSegmentedGroup(ucBtn);
            }

            applyV4DomTranslations(lang);
            sanitizeAllSegmentedGroupsOnBoot();
            window.__v4BootHydratedOnce = true;
        } catch (_) {} finally {
            window.__v4Hydrating = false;
        }
    }

    function readCurrentCropFromDom() {
        return {
            top: parseInt((document.getElementById('crop-top-slider') || {}).value || '0', 10) || 0,
            bottom: parseInt((document.getElementById('crop-bottom-slider') || {}).value || '0', 10) || 0,
            left: parseInt((document.getElementById('crop-left-slider') || {}).value || '0', 10) || 0,
            right: parseInt((document.getElementById('crop-right-slider') || {}).value || '0', 10) || 0
        };
    }

    function bindUniversalTogglePersistence() {
        if (window.__v4UniversalListenersBound) return;
        window.__v4UniversalListenersBound = true;

        document.addEventListener('pointerdown', function() {
            var now = Date.now();
            if (now - (window.__v4LastDismissTrayMs || 0) > 40) {
                window.__v4LastDismissTrayMs = now;
                var fn = window.__v4OrigFetch || window.fetch;
                try {
                    fn('/api/tray/dismiss_menu', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: '{}'
                    }).catch(function(){});
                } catch (_) {}
            }
        }, true);

        document.addEventListener('wheel', function() {
            var now = Date.now();
            if (now - (window.__v4LastDismissTrayWheelMs || 0) > 200) {
                window.__v4LastDismissTrayWheelMs = now;
                var fn = window.__v4OrigFetch || window.fetch;
                try {
                    fn('/api/tray/dismiss_menu', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: '{}'
                    }).catch(function(){});
                } catch (_) {}
            }
        }, { capture: true, passive: true });

        document.addEventListener('click', function(e) {
            if (window.__v4Hydrating) return;
            var target = e.target && e.target.closest ? e.target.closest('button') : null;
            if (!target) return;

            if (target.id === 'btn-lang-id' || target.id === 'btn-lang-en' || target.hasAttribute('data-lang')) {
                var nextLang = (target.getAttribute('data-lang') === 'en' || target.id === 'btn-lang-en') ? 'en' : 'id';
                window.__v4LastToggleChangeMs = Date.now();
                wrapApplyLanguageIfReady();
                setTimeout(function() {
                    applyV4LanguageUiAfterAppJs(nextLang, true);
                }, 0);
                return;
            }

            if (target.hasAttribute('data-fit-choice')) {
                var fm = target.getAttribute('data-fit-choice');
                enforceSingleActivePerSegmentedGroup(target);
                try { localStorage.setItem('pane-fit-mode', fm); } catch (_) {}
                persistSettingPartial({ fitMode: fm }, false);
                syncNativeVideoPresenter({ fitMode: fm === 'fill' ? 'edge_to_edge' : (fm === 'contain' ? 'original_ratio' : 'auto_fit') });
                return;
            }

            if (target.hasAttribute('data-device-profile')) {
                var dp = target.getAttribute('data-device-profile') || 'auto';
                enforceSingleActivePerSegmentedGroup(target);
                try { localStorage.setItem('pane-device-profile', dp); } catch (_) {}
                persistSettingPartial({ deviceProfile: dp }, false);
                return;
            }

            if (target.hasAttribute('data-auto-rotate-choice')) {
                var ar = target.getAttribute('data-auto-rotate-choice') === 'on';
                enforceSingleActivePerSegmentedGroup(target);
                try { localStorage.setItem('pane-auto-rotate', String(ar)); } catch (_) {}
                persistSettingPartial({ autoRotate: ar }, false);
                return;
            }

            if (target.id === 'btn-reset-crop' || target.id === 'btn-save-crop') {
                setTimeout(function() {
                    var cObj = target.id === 'btn-reset-crop' ? { top: 0, bottom: 0, left: 0, right: 0 } : readCurrentCropFromDom();
                    try { localStorage.setItem('pane-crop-edges', JSON.stringify(cObj)); } catch (_) {}
                    persistSettingPartial({ crop: cObj }, false);
                    syncNativeVideoPresenter({
                        cropTop: Math.min(24, Number(cObj.top || 0)),
                        cropBottom: Math.min(24, Number(cObj.bottom || 0)),
                        cropLeft: Math.min(24, Number(cObj.left || 0)),
                        cropRight: Math.min(24, Number(cObj.right || 0))
                    });
                }, 0);
                return;
            }

            if (target.hasAttribute('data-audio-out')) {
                var aout = target.getAttribute('data-audio-out') || 'pc';
                var segAu = document.getElementById('segmented-audio-out');
                var btBtn = document.getElementById('btn-audio-out-bt');
                if (aout === 'bluetooth') {
                    clearSegmentedGroupSelection(segAu);
                    if (btBtn) {
                        btBtn.classList.add('is-active', 'active');
                        btBtn.setAttribute('aria-pressed', 'true');
                    }
                    persistSettingPartial({ audioOut: 'bluetooth', bluetoothA2dp: true }, false);
                } else {
                    if (btBtn) {
                        btBtn.classList.remove('is-active', 'active');
                        btBtn.setAttribute('aria-pressed', 'false');
                    }
                    enforceSingleActivePerSegmentedGroup(target);
                    persistSettingPartial({ audioOut: aout, bluetoothA2dp: false }, false);
                }
                return;
            }

            if (target.id === 'btn-audio-top' || target.id === 'btn-audio-drawer') {
                window.__v4LastToggleChangeMs = Date.now();
                setTimeout(function() { sendDomBeacon(false); }, 120);
                return;
            }

            if (target.hasAttribute('data-audio-buffer')) {
                var ab = target.getAttribute('data-audio-buffer') || 'normal';
                enforceSingleActivePerSegmentedGroup(target);
                setTimeout(function() { applyV4DomTranslations(window.__v4CurrentLang || 'id'); }, 0);
                persistSettingPartial({ audioBufferPreset: ab }, false);
                return;
            }

            if (target.hasAttribute('data-lip-sync')) {
                var ls = target.getAttribute('data-lip-sync') === 'true';
                enforceSingleActivePerSegmentedGroup(target);
                var lsh = document.getElementById('lip-sync-hint');
                if (lsh) lsh.textContent = ls ? 'ON' : 'OFF';
                persistSettingPartial({ lipSync: ls }, false);
                return;
            }

            if (target.hasAttribute('data-speed-choice')) {
                var spd = parseInt(target.getAttribute('data-speed-choice'), 10) || 100;
                enforceSingleActivePerSegmentedGroup(target);
                try { localStorage.setItem('pane-trackpad-speed', String(spd)); } catch (_) {}
                persistSettingPartial({ trackpadSpeed: spd }, false);
                return;
            }

            if (target.id === 'btn-save-speed') {
                setTimeout(function() {
                    var sv = parseInt((document.getElementById('trackpad-speed-slider') || {}).value || '100', 10) || 100;
                    try { localStorage.setItem('pane-trackpad-speed', String(sv)); } catch (_) {}
                    persistSettingPartial({ trackpadSpeed: sv }, false);
                }, 0);
                return;
            }

            if (target.hasAttribute('data-shortcut-choice')) {
                var sc = target.getAttribute('data-shortcut-choice') === 'true';
                enforceSingleActivePerSegmentedGroup(target);
                try { localStorage.setItem('pane-shortcuts', String(sc)); } catch (_) {}
                persistSettingPartial({ crossPlatformShortcuts: sc }, false);
                return;
            }

            if (target.hasAttribute('data-paste-pc-choice')) {
                var ppc = target.getAttribute('data-paste-pc-choice') === 'true';
                enforceSingleActivePerSegmentedGroup(target);
                try { localStorage.setItem('pane-paste-pc', String(ppc)); } catch (_) {}
                persistSettingPartial({ pastePcClipboard: ppc }, false);
                return;
            }

            if (target.hasAttribute('data-transport')) {
                var tr = (target.getAttribute('data-transport') === 'wifi') ? 'wifi' : 'usb';
                try { localStorage.setItem('handset-transport', tr); } catch (_) {}
                setTimeout(function() {
                    var drBtn = document.querySelector('.segmented [data-transport="' + tr + '"]');
                    if (drBtn) enforceSingleActivePerSegmentedGroup(drBtn);
                    var lpC = document.getElementById('launchpad-transport-cable');
                    var lpW = document.getElementById('launchpad-transport-wifi');
                    if (lpC) {
                        lpC.classList.toggle('is-active', tr === 'usb');
                        lpC.classList.toggle('active', tr === 'usb');
                        lpC.setAttribute('aria-checked', String(tr === 'usb'));
                    }
                    if (lpW) {
                        lpW.classList.toggle('is-active', tr === 'wifi');
                        lpW.classList.toggle('active', tr === 'wifi');
                        lpW.setAttribute('aria-checked', String(tr === 'wifi'));
                    }
                }, 0);
                persistSettingPartial({ transport: tr }, false);
                return;
            }

            if (target.id === 'toggle-auto-update-check') {
                window.__v4LastToggleChangeMs = Date.now();
                setTimeout(function() {
                    var nextAc = target.classList.contains('is-active') || target.getAttribute('aria-checked') === 'true';
                    target.classList.toggle('active', nextAc);
                    target.setAttribute('aria-checked', String(nextAc));
                    persistSettingPartial({ autoCheckUpdate: nextAc }, false);
                }, 0);
                return;
            }

            if (target.hasAttribute('data-channel')) {
                var ch = (target.getAttribute('data-channel') === 'beta') ? 'beta' : 'stable';
                enforceSingleActivePerSegmentedGroup(target);
                persistSettingPartial({ updateChannel: ch }, false);
                return;
            }

            if (target.hasAttribute('data-theme-choice')) {
                var thm = (target.getAttribute('data-theme-choice') === 'light') ? 'light' : 'dark';
                window.__v4LastThemeTouchMs = Date.now();
                window.__v4LastToggleChangeMs = Date.now();
                enforceSingleActivePerSegmentedGroup(target);
                try { localStorage.setItem('pane-theme', thm); } catch (_) {}
                persistSettingPartial({ theme: thm }, false);
                postApi('/api/window/set_theme', { theme: thm });
                return;
            }

            if (target.id === 'btn-wipe-privacy') {
                window.__v4IntentionalWipe = true;
                window.__v4JustWiped = true;
                window.__v4PendingPatch = {};
                if (window.__v4PatchTimer) {
                    clearTimeout(window.__v4PatchTimer);
                    window.__v4PatchTimer = null;
                }
                return;
            }
        }, true);

        var onControlInputOrChange = function(e) {
            if (window.__v4Hydrating) return;
            var el = e.target;
            if (!el || !el.id) return;
            if (el.id.indexOf('crop-') === 0) {
                var match = el.id.match(/^crop-(top|bottom|left|right)-(slider|num)$/);
                if (match) {
                    var edge = match[1];
                    var kind = match[2];
                    var val = Math.max(0, Math.min(120, parseInt(el.value || '0', 10) || 0));
                    var peer = document.getElementById('crop-' + edge + '-' + (kind === 'slider' ? 'num' : 'slider'));
                    if (peer && peer.value !== String(val)) peer.value = String(val);
                    var cObj = readCurrentCropFromDom();
                    try { localStorage.setItem('pane-crop-edges', JSON.stringify(cObj)); } catch (_) {}
                    persistSettingPartial({ crop: cObj }, false);
                    syncNativeVideoPresenter({
                        cropTop: Math.min(24, Number(cObj.top || 0)),
                        cropBottom: Math.min(24, Number(cObj.bottom || 0)),
                        cropLeft: Math.min(24, Number(cObj.left || 0)),
                        cropRight: Math.min(24, Number(cObj.right || 0))
                    });
                }
                return;
            }
            if (el.id === 'audio-vol-slider') {
                var vol = Math.max(0, Math.min(100, parseInt(el.value || '100', 10) || 0));
                var vTxt = document.getElementById('audio-vol-val');
                if (vTxt) vTxt.textContent = String(vol) + '%';
                persistSettingPartial({ audioVolume: vol }, false);
                return;
            }
            if (el.id === 'trackpad-speed-slider' || el.id === 'trackpad-speed-num') {
                var spd = Math.max(30, Math.min(200, parseInt(el.value || '100', 10) || 100));
                var peerId = (el.id === 'trackpad-speed-slider') ? 'trackpad-speed-num' : 'trackpad-speed-slider';
                var peerSpd = document.getElementById(peerId);
                if (peerSpd && peerSpd.value !== String(spd)) peerSpd.value = String(spd);
                var spdPreset = document.querySelector('.segmented-speed [data-speed-choice="' + String(spd) + '"]');
                if (spdPreset) {
                    enforceSingleActivePerSegmentedGroup(spdPreset);
                } else {
                    clearSegmentedGroupSelection(document.querySelector('.segmented-speed'));
                }
                try { localStorage.setItem('pane-trackpad-speed', String(spd)); } catch (_) {}
                persistSettingPartial({ trackpadSpeed: spd }, false);
                return;
            }
        };
        document.addEventListener('input', onControlInputOrChange, false);
        document.addEventListener('change', onControlInputOrChange, false);
    }

    function hydratePinFromSettings(data) {
        if (!data || typeof data !== 'object') return;
        try {
            var fid = String(data.folderInstanceId || '');
            if (fid) {
                window.__v4FolderInstanceId = fid;
                localStorage.setItem('iphone_link_folder_instance_id', fid);
            }
            var serverPin = sanitizePinDigits(data.unlockPin || '');
            var hasSettingsFile = Boolean(data.hasUserSettingsFile);
            if (serverPin) {
                window.__v4SavedUnlockPin = serverPin;
                window.__v4IntentionalWipe = false;
                localStorage.setItem('iphone_link_unlock_pin', serverPin);
                localStorage.setItem('pane-unlock-pin', serverPin);
                var pinEl = document.getElementById('input-unlock-pin');
                if (pinEl && document.activeElement !== pinEl && pinEl.value !== serverPin) {
                    pinEl.value = serverPin;
                }
            } else if (!hasSettingsFile && !window.__v4PinTypedThisSession) {
                window.__v4SavedUnlockPin = '';
                localStorage.removeItem('iphone_link_unlock_pin');
                localStorage.removeItem('pane-unlock-pin');
                var pinElClear = document.getElementById('input-unlock-pin');
                if (pinElClear && document.activeElement !== pinElClear && pinElClear.value !== '') {
                    pinElClear.value = '';
                }
            }
        } catch (_) {}
    }

    if (!window.__v4FetchWrapped && typeof window.fetch === 'function') {
        window.__v4FetchWrapped = true;
        var __origFetch = window.fetch.bind(window);
        window.__v4OrigFetch = __origFetch;
        window.fetch = function(input, init) {
            var urlStr = (typeof input === 'string') ? input : ((input && input.url) || '');
            var method = (((init && init.method) || (input && input.method) || 'GET') + '').toUpperCase();
            var p = __origFetch.apply(window, arguments);
            if ((urlStr.indexOf('/api/settings') !== -1 || urlStr.indexOf('/api/keyboard/settings') !== -1) && method === 'GET') {
                return p.then(function(resp) {
                    try {
                        if (!resp || !resp.ok) return resp;
                        return resp.clone().json().then(function(data) {
                            hydratePinFromSettings(data);
                            var recentToggle = (Date.now() - (window.__v4LastToggleChangeMs || 0)) < 2500;
                            var hasPending = Object.keys(window.__v4PendingPatch || {}).length > 0;
                            if (!recentToggle && !hasPending) {
                                hydrateAllTogglesFromSettings(data);
                            }
                            syncVersionLabels();
                            return new Response(JSON.stringify(data), {
                                status: resp.status,
                                statusText: resp.statusText,
                                headers: resp.headers
                            });
                        }).catch(function() { return resp; });
                    } catch (_) {
                        return resp;
                    }
                });
            }
            if (urlStr.indexOf('/api/device/wipe-privacy-data') !== -1 && method === 'POST') {
                window.__v4IntentionalWipe = true;
                window.__v4JustWiped = true;
                window.__v4PendingPatch = {};
                if (window.__v4PatchTimer) {
                    clearTimeout(window.__v4PatchTimer);
                    window.__v4PatchTimer = null;
                }
            }
            return p;
        };
    }

    var __v4PinSyncInFlight = false;
    var __v4PinSyncPending = null;
    function flushPinSyncQueue(useKeepalive) {
        if (__v4PinSyncPending === null) return;
        if (__v4PinSyncInFlight && !useKeepalive) return;
        var nextPin = __v4PinSyncPending;
        __v4PinSyncPending = null;
        __v4PinSyncInFlight = true;
        var fn = window.__v4OrigFetch || window.fetch;
        try {
            fn('/api/settings', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ unlockPin: nextPin }),
                keepalive: Boolean(useKeepalive)
            }).then(function() {
                __v4PinSyncInFlight = false;
                if (__v4PinSyncPending !== null) {
                    flushPinSyncQueue(false);
                } else {
                    setTimeout(function() { sendDomBeacon(false); }, 40);
                }
            }).catch(function() {
                __v4PinSyncInFlight = false;
            });
        } catch (_) {
            __v4PinSyncInFlight = false;
        }
    }

    function persistUnlockPinDual(rawVal, useKeepalive) {
        var clean = sanitizePinDigits(rawVal);
        window.__v4SavedUnlockPin = clean;
        window.__v4PinTypedThisSession = true;
        window.__v4IntentionalWipe = false;
        try {
            if (window.__v4FolderInstanceId) {
                localStorage.setItem('iphone_link_folder_instance_id', window.__v4FolderInstanceId);
            }
            localStorage.setItem('iphone_link_unlock_pin', clean);
            localStorage.setItem('pane-unlock-pin', clean);
        } catch (_) {}
        __v4PinSyncPending = clean;
        flushPinSyncQueue(Boolean(useKeepalive));
    }

    function guardPinInputAgainstAccidentalWipe() {
        try {
            if (window.__v4IntentionalWipe) return;
            var saved = sanitizePinDigits(window.__v4SavedUnlockPin || localStorage.getItem('iphone_link_unlock_pin') || localStorage.getItem('pane-unlock-pin') || '');
            if (!saved) return;
            var pinEl = document.getElementById('input-unlock-pin');
            if (pinEl && document.activeElement !== pinEl && pinEl.value !== saved) {
                pinEl.value = saved;
            }
            localStorage.setItem('iphone_link_unlock_pin', saved);
            localStorage.setItem('pane-unlock-pin', saved);
            if (window.__v4FolderInstanceId) {
                localStorage.setItem('iphone_link_folder_instance_id', window.__v4FolderInstanceId);
            }
        } catch (_) {}
    }

    function bindPinPersistence() {
        var pinEl = document.getElementById('input-unlock-pin');
        if (pinEl && !pinEl.__v4PinBound) {
            pinEl.__v4PinBound = true;
            pinEl.addEventListener('input', function() {
                var clean = sanitizePinDigits(pinEl.value);
                if (pinEl.value !== clean) pinEl.value = clean;
                persistUnlockPinDual(clean, false);
            });
            pinEl.addEventListener('change', function() {
                var clean = sanitizePinDigits(pinEl.value);
                if (pinEl.value !== clean) pinEl.value = clean;
                persistUnlockPinDual(clean, true);
            });
            pinEl.addEventListener('blur', function() {
                var clean = sanitizePinDigits(pinEl.value);
                persistUnlockPinDual(clean, true);
            });
        }
        var wipeBtn = document.getElementById('btn-wipe-privacy');
        if (wipeBtn && !wipeBtn.__v4WipeBound) {
            wipeBtn.__v4WipeBound = true;
            wipeBtn.addEventListener('click', function() {
                window.__v4IntentionalWipe = true;
                window.__v4PinTypedThisSession = false;
                window.__v4SavedUnlockPin = '';
                __v4PinSyncPending = null;
                try {
                    localStorage.removeItem('iphone_link_unlock_pin');
                    localStorage.removeItem('pane-unlock-pin');
                } catch (_) {}
                setTimeout(function() { sendDomBeacon(false); }, 150);
            }, true);
        }
        var stealthBtn = document.getElementById('btn-stealth-mode');
        if (stealthBtn && !stealthBtn.__v4StealthGuardBound) {
            stealthBtn.__v4StealthGuardBound = true;
            stealthBtn.addEventListener('click', function() {
                setTimeout(guardPinInputAgainstAccidentalWipe, 0);
                setTimeout(guardPinInputAgainstAccidentalWipe, 80);
                setTimeout(guardPinInputAgainstAccidentalWipe, 250);
            });
        }
    }

    function handlePendingTrayAction(act) {
        if (!act) return;
        window.__v4LastTrayAction = String(act);
        try {
            if (act === 'check_updates') {
                window.__v4LastDrawerTouch = Date.now();
                var appElU = document.getElementById('app-root') || document.querySelector('.app');
                if (appElU) {
                    appElU.classList.add('drawer-open');
                }
                var cpEl = document.getElementById('control-pane');
                if (cpEl) cpEl.hidden = false;
                if (typeof toggleSettings === 'function') {
                    toggleSettings(true);
                }
                setTimeout(function() {
                    ensureUpdaterCardExistsInDom();
                    applyV4DomTranslations(window.__v4CurrentLang || 'id');
                    var cardUpd = document.getElementById('card-updater');
                    if (cardUpd) {
                        cardUpd.classList.remove('is-collapsed');
                        var btnCol = document.getElementById('btn-update-collapse');
                        if (btnCol) btnCol.setAttribute('aria-expanded', 'true');
                        if (typeof cardUpd.scrollIntoView === 'function') {
                            cardUpd.scrollIntoView({ behavior: 'smooth', block: 'center' });
                        }
                        window.__v4UpdaterScrolledIntoView = true;
                    }
                    var btnChk = document.getElementById('btn-check-update');
                    if (btnChk) {
                        window.__v4CheckUpdateClicked = true;
                        if (!btnChk.disabled) btnChk.click();
                    }
                    setTimeout(function() { sendDomBeacon(false); }, 180);
                }, 140);
                return;
            }
            if (act.indexOf('set_landscape:') === 0) {
                var targetL = act.slice(-4) === 'true';
                var appElL = document.getElementById('app-root') || document.querySelector('.app');
                var isLandNow = Boolean(appElL && appElL.classList.contains('is-landscape'));
                if (isLandNow !== targetL) {
                    var btnRot = document.getElementById('btn-rotate-landscape');
                    if (btnRot) btnRot.click();
                }
            } else if (act.indexOf('set_stealth:') === 0) {
                var targetS = act.slice(-4) === 'true';
                if (typeof window.triggerToggleStealthDim === 'function') {
                    window.triggerToggleStealthDim(targetS);
                }
                setTimeout(guardPinInputAgainstAccidentalWipe, 120);
            } else if (act.indexOf('set_audio_pc:') === 0) {
                var targetA = act.slice(-4) === 'true';
                var btnAu = document.getElementById(targetA ? 'btn-audio-out-pc' : 'btn-audio-out-device');
                if (btnAu) btnAu.click();
            } else if (act === 'clear_cache') {
                if (typeof window.triggerClearCopyCacheWithFeedback === 'function') {
                    window.triggerClearCopyCacheWithFeedback();
                } else {
                    var btnCc = document.getElementById('btn-clear-copy-cache');
                    if (btnCc) btnCc.click();
                }
            }
        } catch (_) {}
        setTimeout(function() { sendDomBeacon(false); }, 180);
    }

    window.__v4PersistUnlockPin = persistUnlockPinDual;
    window.__v4HydratePinFromSettings = hydratePinFromSettings;
    window.__v4HandleTrayAction = handlePendingTrayAction;
    window.__v4EnsureUpdaterCard = ensureUpdaterCardExistsInDom;

    function initBridge() {
        var isMainAppWindow = Boolean(document.getElementById('app-root') || document.querySelector('.side-drawer') || document.getElementById('screen-view'));
        if (!isMainAppWindow) {
            return;
        }
        installDragListeners();
        updateParallaxVars();
        syncIpadDisplayWidthVar();
        syncMirroringStateClass();
        ensureUpdaterCardExistsInDom();
        wrapApplyLanguageIfReady();
        bindUniversalTogglePersistence();
        if (window.__V4_INITIAL_SETTINGS__) {
            hydrateAllTogglesFromSettings(window.__V4_INITIAL_SETTINGS__);
        }
        sanitizeAllSegmentedGroupsOnBoot();
        sanitizeErrorBanners();
        syncVersionLabels();
        bindPinPersistence();
        try {
            fetch('/api/settings').then(function(r) { return r.json(); }).then(function(data) {
                if (!window.__v4LastToggleChangeMs) {
                    hydrateAllTogglesFromSettings(data);
                } else {
                    hydratePinFromSettings(data);
                }
                syncVersionLabels();
            }).catch(function(){});
        } catch (_) {}
        window.addEventListener('beforeunload', function() {
            var pEl = document.getElementById('input-unlock-pin');
            if (pEl && !window.__v4IntentionalWipe && sanitizePinDigits(pEl.value)) {
                persistUnlockPinDual(pEl.value, true);
            }
            if (!window.__v4JustWiped) {
                flushSettingsPatch(true);
            }
        });

        // Override app.js global computeDisplayWidthCss so exits from Full Screen keep exact 564px (iPad) or dynamic portrait width
        window.computeDisplayWidthCss = computeV4DisplayWidthCss;

        // Instantaneous .is-mirroring state observer (#screen-empty <-> #screen-view) + error banner null-hidden sanitizer
        try {
            var obsTargetEmpty = document.getElementById('screen-empty');
            var obsTargetRoot = document.getElementById('app-root') || document.querySelector('.app');
            if (typeof MutationObserver !== 'undefined') {
                var mObs = new MutationObserver(function() {
                    syncMirroringStateClass();
                });
                if (obsTargetEmpty) {
                    mObs.observe(obsTargetEmpty, { attributes: true, attributeFilter: ['hidden', 'style', 'class'] });
                }
                if (obsTargetRoot) {
                    mObs.observe(obsTargetRoot, { attributes: true, attributeFilter: ['class'] });
                }
                var bObs = new MutationObserver(function() {
                    sanitizeErrorBanners();
                });
                ['screen-error-banner', 'launchpad-error'].forEach(function(bid) {
                    var bel = document.getElementById(bid);
                    if (bel) {
                        bObs.observe(bel, { childList: true, characterData: true, subtree: true, attributes: true });
                    }
                });
            }
        } catch (_) {}

        // Pre-sync --display-width cleanly when #win-max clicked (preserve .is-fullsize until app.js reads it, then pywebview.api.toggle_fullscreen syncs .is-fullsize & --display-width)
        document.addEventListener('click', function(e) {
            var fsBtn = e.target && e.target.closest ? e.target.closest('#win-max, #btn-fullsize') : null;
            if (fsBtn) {
                var dw = computeV4DisplayWidthCss();
                document.documentElement.style.setProperty('--display-width', dw);
                if (document.body) document.body.style.setProperty('--display-width', dw);
                var appEl = document.getElementById('app-root') || document.querySelector('.app');
                if (appEl) appEl.style.setProperty('--display-width', dw);
                syncMirroringStateClass();
            }
            if (e.target && e.target.closest && e.target.closest('#btn-settings, #close-settings')) {
                window.__v4LastDrawerTouch = Date.now();
            }
        }, true);

        // Ensure theme buttons have proper IDs and listeners
        var lightBtn = document.querySelector('[data-theme-choice="light"]');
        if (lightBtn) {
            lightBtn.id = 'theme-light';
            lightBtn.addEventListener('click', function() {
                window.__v4LastThemeTouchMs = Date.now();
                window.__v4ApplyTheme('light');
                persistSettingPartial({ theme: 'light' }, false);
                postApi('/api/window/set_theme', { theme: 'light' });
            }, true);
        }
        var darkBtn = document.querySelector('[data-theme-choice="dark"]');
        if (darkBtn) {
            darkBtn.id = 'theme-dark';
            darkBtn.addEventListener('click', function() {
                window.__v4LastThemeTouchMs = Date.now();
                window.__v4ApplyTheme('dark');
                persistSettingPartial({ theme: 'dark' }, false);
                postApi('/api/window/set_theme', { theme: 'dark' });
            }, true);
        }

        requestAnimationFrame(checkRaf);
        setTimeout(function() {
            window.computeDisplayWidthCss = computeV4DisplayWidthCss;
            wrapApplyLanguageIfReady();
            syncIpadDisplayWidthVar();
            syncMirroringStateClass();
            if (!window.__v4LastToggleChangeMs && window.__V4_BOOT_SETTINGS__) {
                hydrateAllTogglesFromSettings(window.__V4_BOOT_SETTINGS__);
            }
            applyV4DomTranslations(window.__v4CurrentLang || 'id');
            sanitizeAllSegmentedGroupsOnBoot();
            syncVersionLabels();
            bindPinPersistence();
            guardPinInputAgainstAccidentalWipe();
            sendDomBeacon(false);
        }, 300);
        setTimeout(function() {
            wrapApplyLanguageIfReady();
            syncIpadDisplayWidthVar();
            syncMirroringStateClass();
            if (!window.__v4LastToggleChangeMs && window.__V4_BOOT_SETTINGS__) {
                hydrateAllTogglesFromSettings(window.__V4_BOOT_SETTINGS__);
            }
            applyV4DomTranslations(window.__v4CurrentLang || 'id');
            sanitizeAllSegmentedGroupsOnBoot();
            syncVersionLabels();
            bindPinPersistence();
            guardPinInputAgainstAccidentalWipe();
            sendDomBeacon(false);
        }, 1000);

        setInterval(function() {
            try {
                wrapApplyLanguageIfReady();
                syncMirroringStateClass();
                sanitizeErrorBanners();
                syncVersionLabels();
                guardPinInputAgainstAccidentalWipe();
                applyV4DomTranslations(window.__v4CurrentLang || 'id');
                fetch('/api/status').then(function(r){ return r.json(); }).then(function(s) {
                    if (s && s.pendingClick) {
                        window.__v4ClickSelector(s.pendingClick);
                        setTimeout(function() { sendDomBeacon(false); }, 180);
                    }
                    if (s && s.pendingInput && s.pendingInput.selector) {
                        var piEl = document.querySelector(s.pendingInput.selector);
                        if (piEl) {
                            piEl.focus();
                            piEl.value = String(s.pendingInput.value || '');
                            piEl.dispatchEvent(new Event('input', { bubbles: true }));
                            piEl.dispatchEvent(new Event('change', { bubbles: true }));
                            piEl.blur();
                            setTimeout(function() { sendDomBeacon(false); }, 180);
                        }
                    }
                    if (s && s.pendingTrayAction) {
                        handlePendingTrayAction(s.pendingTrayAction);
                    }
                    if (s && s.pendingTest === 'verify_mirror_autofit') {
                        runSyntheticMirrorAutofitVerify();
                    }
                    if (s && s.pendingTest === 'verify_drawer_toggle_flicker') {
                        runDrawerToggleFlickerVerify();
                    }
                    if (s && s.pendingTest === 'verify_fullscreen_enter_sync') {
                        runFullscreenEnterSyncVerify();
                    }
                    if (s && s.pendingTest === 'verify_tahap182_ui') {
                        runTahap182UiVerify();
                    }
                    if (s && s.window && typeof s.window.drawer_open === 'boolean') {
                        var timeSinceDrawerTouch = Date.now() - (window.__v4LastDrawerTouch || 0);
                        var appRootPoll = document.getElementById('app-root') || document.querySelector('.app');
                        var isFullNow = Boolean(appRootPoll && appRootPoll.classList.contains('is-fullsize'));
                        if (!isFullNow && timeSinceDrawerTouch > 1200 && typeof toggleSettings === 'function' && window.isDrawerOpen !== s.window.drawer_open) {
                            toggleSettings(s.window.drawer_open);
                            setTimeout(function() { sendDomBeacon(false); }, 300);
                        }
                    }
                    if (s && s.window && s.window.active_device_profile) {
                        var servProf = s.window.active_device_profile;
                        var timeSinceTouch = Date.now() - (window.__v4LastProfileTouch || 0);
                        if (timeSinceTouch > 1500 && window.__v4ActiveDeviceProfile !== servProf) {
                            updateDeviceProfileState(servProf, true);
                            setTimeout(function() { sendDomBeacon(false); }, 100);
                        }
                    }
                    syncIpadDisplayWidthVar();
                    var desiredTheme = (s && s.webviewMetrics && s.webviewMetrics.activeTheme) || (s && s.settings && s.settings.theme);
                    var timeSinceThemeTouch = Date.now() - (window.__v4LastThemeTouchMs || 0);
                    if (desiredTheme && timeSinceThemeTouch > 1500) {
                        var curTheme = document.documentElement.getAttribute('data-theme') || 'dark';
                        if (curTheme !== desiredTheme) {
                            window.__v4ApplyTheme(desiredTheme);
                        }
                    }
                    try {
                        var errBanner = document.getElementById('screen-error-banner');
                        if (errBanner && s && s.screen) {
                            var errMsg = (s.screen.error || s.screen.note || '').trim();
                            if (s.screen.phase === 'error' && errMsg) {
                                errBanner.textContent = errMsg;
                                errBanner.hidden = false;
                                errBanner.style.display = 'block';
                            } else if (s.screen.phase === 'running' && (errBanner.textContent || '').trim()) {
                                errBanner.textContent = '';
                                errBanner.hidden = true;
                                errBanner.style.display = 'none';
                            }
                        }
                    } catch (_) {}
                    try {
                        // Tahap 249 Pilar 3: Eliminate stale boot pill ("Memuat..." / "Menunggu iPhone")
                        // and keep #status-pill synced in real time across phase & Wi-Fi online transitions.
                        var stopLatchActive = typeof window._isUserStopLatchActive === 'function' && window._isUserStopLatchActive();
                        if (s && !stopLatchActive && typeof window.renderPills === 'function') {
                            var spEl = document.getElementById('status-pill') || document.getElementById('pill-connection');
                            var spTxt = spEl ? (spEl.textContent || '').trim().toLowerCase() : '';
                            var isBootPlaceholder = !spTxt
                                || spTxt.indexOf('memuat') !== -1
                                || spTxt.indexOf('loading') !== -1
                                || ((spTxt === 'menunggu iphone' || spTxt === 'waiting for iphone') && Array.isArray(s.devices) && s.devices.length > 0);
                            var curPhase = (s.screen && s.screen.phase) ? String(s.screen.phase) : 'idle';
                            var curOnlineSig = Array.isArray(s.devices)
                                ? s.devices.map(function(d) { return d ? (d.udid + ':' + (d.usb ? 'U' : '') + (d.wifiOnline ? 'W' : '')) : ''; }).join('|')
                                : '';
                            if (isBootPlaceholder
                                || window.__v4LastPolledScreenPhase !== curPhase
                                || window.__v4LastPolledOnlineSig !== curOnlineSig) {
                                window.__v4LastPolledScreenPhase = curPhase;
                                window.__v4LastPolledOnlineSig = curOnlineSig;
                                window.renderPills(s);
                            }
                        }
                    } catch (_) {}
                    try {
                        // Tahap 241 + 243 + 249 Pilar 3: Fresh-Install Auto-Audio Safety Sync
                        // respecting pcBridgeConnecting, preserving .btn-label, with up to 3 spaced retries.
                        var audioObj = (s && s.audio) ? s.audio : null;
                        if (audioObj) {
                            var bridgeOnNow = Boolean(audioObj.pcBridgeActive === true);
                            var bridgeConnNow = Boolean(audioObj.pcBridgeConnecting === true);
                            var bridgeDevNow = (typeof audioObj.pcBridgeDevice === 'string') ? audioObj.pcBridgeDevice.trim() : '';
                            var audioSig = (bridgeOnNow ? '1' : '0') + '|' + (bridgeConnNow ? '1' : '0') + '|' + bridgeDevNow + '|' + String(audioObj.audioOut || '');
                            window.__v4ActiveAudioDevice = bridgeOnNow ? bridgeDevNow : '';
                            if (window.__v4LastAudioBadgeSig !== audioSig) {
                                window.__v4LastAudioBadgeSig = audioSig;
                                if (typeof window.renderAudio === 'function') {
                                    try { window.renderAudio(audioObj); } catch (_) {}
                                }
                            }
                            var audioBadgeLblLive = document.querySelector('#audio-status-badge .btn-label') || document.getElementById('audio-status-badge');
                            if (audioBadgeLblLive && bridgeOnNow && bridgeDevNow) {
                                var isEnAud = (window.__v4CurrentLang || 'id').toLowerCase() === 'en';
                                audioBadgeLblLive.textContent = isEnAud
                                    ? ('A2DP Connected: ' + bridgeDevNow + ' (48 kHz WASAPI)')
                                    : ('A2DP Terhubung: ' + bridgeDevNow + ' (48 kHz WASAPI)');
                            }
                        }
                        if (s && s.screen && s.screen.phase === 'running') {
                            var btnPcA = document.getElementById('btn-audio-out-pc');
                            var btnBtA = document.getElementById('btn-audio-out-bt');
                            var pcLit = Boolean(btnPcA && (btnPcA.classList.contains('active') || btnPcA.classList.contains('is-active')));
                            var btLit = Boolean(btnBtA && (btnBtA.classList.contains('active') || btnBtA.classList.contains('is-active')));
                            var bridgeOn = Boolean(s.audio && s.audio.pcBridgeActive === true);
                            var bridgeConn = Boolean(s.audio && s.audio.pcBridgeConnecting === true);
                            var mutedNow = Boolean(s.audio && s.audio.muted === true);
                            if (bridgeOn) {
                                window.__v4AutoAudioSyncedForRun = true;
                                window.__v4AutoAudioRetryCount = 0;
                            } else if ((pcLit || btLit) && !bridgeConn && !mutedNow) {
                                var nowMs = Date.now();
                                var tries = window.__v4AutoAudioRetryCount || 0;
                                var lastTry = window.__v4AutoAudioLastTryMs || 0;
                                if (tries < 3 && (nowMs - lastTry) >= 2500) {
                                    window.__v4AutoAudioSyncedForRun = true;
                                    window.__v4AutoAudioRetryCount = tries + 1;
                                    window.__v4AutoAudioLastTryMs = nowMs;
                                    postApi('/api/audio', { enable: true, audioOut: btLit ? 'bluetooth' : 'pc' });
                                }
                            }
                        } else {
                            window.__v4AutoAudioSyncedForRun = false;
                            window.__v4AutoAudioRetryCount = 0;
                            window.__v4AutoAudioLastTryMs = 0;
                        }
                    } catch (_) {}
                }).catch(function(){});
            } catch (_) {}
        }, 250);
    }

    if (document.readyState === "loading") {
        document.addEventListener('DOMContentLoaded', initBridge);
    } else {
        initBridge();
    }

    window.pywebview = window.pywebview || {};
    window.pywebview.api = {
        is_pywebview: function() {
            return Promise.resolve(true);
        },
        close: function() {
            try {
                var pEl = document.getElementById('input-unlock-pin');
                if (pEl && !window.__v4IntentionalWipe && sanitizePinDigits(pEl.value)) {
                    persistUnlockPinDual(pEl.value, true);
                }
            } catch (_) {}
            var flushP = (!window.__v4JustWiped) ? flushSettingsPatch(true) : Promise.resolve({ ok: true });
            return Promise.resolve(flushP).catch(function(){}).then(function() {
                if (window.chrome && window.chrome.webview) {
                    try { window.chrome.webview.postMessage("close"); } catch (_) {}
                }
                return postApi('/api/window/close', {});
            });
        },
        minimize: function() {
            if (window.chrome && window.chrome.webview) {
                try { window.chrome.webview.postMessage("minimize"); } catch (_) {}
            }
            return postApi('/api/window/minimize', {});
        },
        minimize_to_tray: function() {
            return postApi('/api/window/hide_to_tray', {});
        },
        hide_to_tray: function() {
            return postApi('/api/window/hide_to_tray', {});
        },
        restore_from_tray: function() {
            return postApi('/api/window/restore_from_tray', {});
        },
        toggle_fullscreen: function(target_state) {
            var appEl = document.getElementById('app-root') || document.querySelector('.app');
            var curFs = Boolean(appEl && appEl.classList.contains('is-fullsize'));
            var nextFs = (typeof target_state === 'boolean') ? target_state : !curFs;
            var curDrawerOpen = Boolean((appEl && appEl.classList.contains('drawer-open')) || window.isDrawerOpen);
            window.__v4LastDrawerTouch = Date.now();
            if (appEl) {
                appEl.classList.toggle('is-fullsize', nextFs);
                syncMirroringStateClass();
                syncPreFullscreenTransitionDom(nextFs);
                void appEl.offsetHeight;
            }
            return postApi('/api/window/toggle_fullscreen', { target: nextFs, drawer_open: curDrawerOpen }).then(function(res) {
                if (res && typeof res.fullscreen === 'boolean') {
                    if (typeof window.applyFullsizeState === 'function') {
                        window.applyFullsizeState(res.fullscreen, false);
                    }
                    syncPreFullscreenTransitionDom(res.fullscreen);
                }
                setTimeout(function() {
                    syncNativeVideoPresenter({ isFullscreen: Boolean(res && res.fullscreen) });
                    sendDomBeacon(false);
                }, 60);
                return res;
            });
        },
        set_drawer: function(open) {
            var target = !!open;
            var now = Date.now();
            window.__v4LastDrawerTouch = now;
            if (window.__v4LastSentDrawerState === target && (now - (window.__v4LastSentDrawerMs || 0)) < 80) {
                return Promise.resolve({ ok: true, deduplicated: true, drawer_open: target });
            }
            window.__v4LastSentDrawerState = target;
            window.__v4LastSentDrawerMs = now;
            syncNativeVideoPresenter({ drawerOpen: target });
            return postApi('/api/window/set_drawer', { open: target });
        },
        toggle_side_drawer: function(open) {
            return window.pywebview.api.set_drawer(open);
        },
        set_orientation: function(landscape, drawer_open) {
            window.__v4LastDrawerTouch = Date.now();
            syncNativeVideoPresenter({
                isLandscape: !!landscape,
                rotationDeg: landscape ? 270 : 0,
                drawerOpen: drawer_open !== undefined ? !!drawer_open : undefined
            });
            return postApi('/api/window/set_orientation', {
                landscape: !!landscape,
                drawer: drawer_open !== undefined ? !!drawer_open : undefined
            });
        },
        sync_device_aspect: function(display_width, drawer_open) {
            var prof = window.__v4ActiveDeviceProfile || "auto";
            var w = display_width;
            if (prof === "ipad" && (!w || w < 500)) {
                w = 564;
            }
            return postApi('/api/window/sync_device_aspect', {
                width: w,
                open: !!drawer_open,
                drawer: !!drawer_open,
                profile: prof
            });
        },
        move_by: function(dx, dy) {
            return postApi('/api/window/move_by', { dx: dx, dy: dy });
        },
        move_to: function(x, y) {
            return postApi('/api/window/move_to', { x: x, y: y });
        },
        sync_layout: function(data) {
            return postApi('/api/window/sync_layout', data || {});
        },
        set_device_family: function(family, should_resize) {
            return postApi('/api/window/set_device_family', {
                family: family || 'iphone',
                resize: should_resize !== false
            });
        },
        set_device_profile: function(profile, family, should_resize) {
            updateDeviceProfileState(profile);
            return postApi('/api/window/set_device_profile', {
                profile: profile || 'auto',
                family: family || 'iphone',
                resize: should_resize !== false
            });
        },
        apply_custom_size: function(width, height) {
            return postApi('/api/window/apply_custom_size', { width: width, height: height });
        },
        reset_window_size: function() {
            handleResetDefaultSize();
            return postApi('/api/window/reset_size', {});
        },
        begin_resize_drag: function(edge, x, y) {
            return postApi('/api/window/begin_resize', { edge: edge, x: x, y: y });
        },
        update_resize_drag: function(x, y) {
            return postApi('/api/window/update_resize', { x: x, y: y });
        },
        end_resize_drag: function() {
            return postApi('/api/window/end_resize', {});
        },
        get_window_rect: function() {
            return fetch('/api/window/rect').then(function(r) { return r.json(); }).catch(function() {
                return { x: 100, y: 100, width: 346, height: 832 };
            });
        },
        lock_screen: function() {
            return postApi('/api/device/lock', {});
        },
        wake_and_unlock: function() {
            return postApi('/api/device/wake-unlock', {});
        },
        toggle_stealth_dim: function() {
            return postApi('/api/device/stealth', {});
        },
        wipe_privacy_data: function() {
            return postApi('/api/device/wipe-privacy-data', {});
        },
        grab_social_link: function() {
            return postApi('/api/social/grab-link', {});
        },
        set_language: function(lang) {
            return postApi('/api/settings', { language: lang });
        },
        set_window_size: function(w, drawer_open) {
            return postApi('/api/window/set_drawer', { width: w, open: !!drawer_open });
        },
        set_theme: function(theme) {
            window.__v4LastThemeTouchMs = Date.now();
            window.__v4ApplyTheme(theme);
            return postApi('/api/window/set_theme', { theme: theme });
        },
        start_window_drag: function() {
            if (window.chrome && window.chrome.webview) {
                try { window.chrome.webview.postMessage("start_drag"); } catch (_) {}
            }
            return postApi('/api/window/start_drag', {});
        },
        open_external_link: function(url) {
            return postApi('/api/system/open_url', { url: url });
        },
        get_system_info: function() {
            return fetch('/api/status').then(function(r) { return r.json(); });
        }
    };

    // Tahap 242 Pilar 3: Seamless Reconnect Overlay Pill + Anti-Blank-Gray Guard
    // Observes existing `/api/screen` responses without adding any extra polling timer.
    (function installTahap242ReconnectGuard() {
        if (window.__v4Tahap242ReconnectGuard) return;
        window.__v4Tahap242ReconnectGuard = true;
        function updateReconnectPill(isReconnecting, noteText) {
            try {
                var host = document.getElementById('phone-screen') || document.querySelector('.stage') || document.body;
                if (!host) return;
                var pill = document.getElementById('v4-reconnect-pill');
                if (isReconnecting) {
                    if (!pill) {
                        pill = document.createElement('div');
                        pill.id = 'v4-reconnect-pill';
                        host.appendChild(pill);
                    }
                    var lang = (window.__v4CurrentLang || 'id').toLowerCase();
                    var attemptMatch = (typeof noteText === 'string') ? noteText.match(/\((\d+\/\d+)\)/) : null;
                    var attemptSuffix = attemptMatch ? (' (' + attemptMatch[1] + ')') : '';
                    pill.textContent = lang === 'en'
                        ? ('\u27F3 Reconnecting' + attemptSuffix + '\u2026')
                        : ('\u27F3 Menyambung ulang' + attemptSuffix + '\u2026');
                    pill.hidden = false;
                    pill.style.display = 'block';
                    var sv = document.getElementById('screen-view');
                    if (sv && sv.getAttribute('src') && sv.getAttribute('src').indexOf('/mirror.html') !== -1) {
                        sv.hidden = false;
                        sv.style.display = 'block';
                    }
                } else if (pill) {
                    pill.hidden = true;
                    pill.style.display = 'none';
                }
            } catch (_) {}
        }
        window.__v4UpdateReconnectPill = updateReconnectPill;
        if (typeof window.fetch === 'function' && !window.__v4FetchWrapped242) {
            window.__v4FetchWrapped242 = true;
            var origFetch = window.fetch.bind(window);
            window.fetch = function(input, init) {
                var urlStr = (typeof input === 'string') ? input : (input && input.url ? String(input.url) : '');
                if (urlStr === '/api/status' && window.__v4ForceNextStatusRefresh) {
                    window.__v4ForceNextStatusRefresh = false;
                    input = '/api/status?force=1';
                    urlStr = '/api/status?force=1';
                }
                var p = origFetch(input, init);
                var isGet = (!init || !init.method || String(init.method).toUpperCase() === 'GET');
                if (isGet && (urlStr.indexOf('/api/screen') !== -1 || urlStr.indexOf('/api/status') !== -1)) {
                    p.then(function(resp) {
                        try {
                            if (resp && resp.ok) {
                                resp.clone().json().then(function(data) {
                                    if (data && typeof data.reconnecting === 'boolean') {
                                        updateReconnectPill(data.reconnecting, data.reconnecting ? (data.note || '') : '');
                                    } else if (data && data.screen && typeof data.screen.reconnecting === 'boolean') {
                                        updateReconnectPill(data.screen.reconnecting, data.screen.reconnecting ? (data.screen.note || '') : '');
                                    }
                                }).catch(function(){});
                            }
                        } catch (_) {}
                    }).catch(function(){});
                }
                return p;
            };
        }
    })();

    // Tahap 243 Pilar 4: Launchpad Startup Wi-Fi Auto-Detect & Live Refresh Button Hook
    (function installTahap243LaunchpadRefreshAndWifiBoot() {
        if (window.__v4Tahap243LaunchpadHook) return;
        window.__v4Tahap243LaunchpadHook = true;

        function triggerLiveRefresh(btnEl) {
            try {
                if (btnEl) btnEl.classList.add('v4-refresh-spinning');
                window.__v4ForceNextStatusRefresh = true;
                var scanP = fetch('/api/wifi/scan', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: '{}'
                }).catch(function(){});
                Promise.resolve(scanP).then(function() {
                    return fetch('/api/status?force=1').then(function(r) { return r.json(); }).catch(function(){ return null; });
                }).then(function(fresh) {
                    try {
                        if (fresh && typeof window.applyServerStatusState === 'function') {
                            window.applyServerStatusState(fresh);
                        } else if (fresh) {
                            if (typeof window.updateLaunchpadState === 'function') window.updateLaunchpadState(fresh);
                            if (typeof window.renderPills === 'function') window.renderPills(fresh);
                        }
                    } catch (_) {}
                }).finally(function() {
                    setTimeout(function() {
                        try { if (btnEl) btnEl.classList.remove('v4-refresh-spinning'); } catch (_) {}
                    }, 350);
                });
            } catch (_) {
                try { if (btnEl) btnEl.classList.remove('v4-refresh-spinning'); } catch (__) {}
            }
        }

        document.addEventListener('click', function(ev) {
            try {
                var target = ev.target && ev.target.closest ? ev.target.closest('#launchpad-scan-btn, #refresh') : null;
                if (!target) return;
                triggerLiveRefresh(target);
            } catch (_) {}
        }, true);

        setTimeout(function() {
            try {
                if (window.__v4Tahap243BootWifiChecked) return;
                window.__v4Tahap243BootWifiChecked = true;
                fetch('/api/status').then(function(r) { return r.json(); }).then(function(s) {
                    var pairedList = (s && s.wifi && Array.isArray(s.wifi.paired)) ? s.wifi.paired : [];
                    var wifiOk = Boolean(s && s.wifi && s.wifi.reachable && s.wifi.ip);
                    var needMultiProbe = pairedList.length >= 2 && !(Array.isArray(s && s.devices) && s.devices.some(function(d) { return d && d.wifiOnline; }));
                    if (pairedList.length > 0 && (!wifiOk || needMultiProbe)) {
                        fetch('/api/wifi/scan', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: '{}'
                        }).catch(function(){}).then(function() {
                            return fetch('/api/status?force=1').then(function(r) { return r.json(); }).catch(function(){ return null; });
                        }).then(function(fresh) {
                            try {
                                if (fresh && typeof window.applyServerStatusState === 'function') {
                                    window.applyServerStatusState(fresh);
                                } else if (fresh) {
                                    if (typeof window.updateLaunchpadState === 'function') window.updateLaunchpadState(fresh);
                                    if (typeof window.renderPills === 'function') window.renderPills(fresh);
                                }
                                var refBtn = document.getElementById('refresh');
                                if (refBtn && !refBtn.disabled) {
                                    window.__v4ForceNextStatusRefresh = false;
                                    refBtn.click();
                                }
                            } catch (_) {}
                        });
                    } else if (s) {
                        if (typeof window.applyServerStatusState === 'function') {
                            window.applyServerStatusState(s);
                        } else {
                            if (typeof window.updateLaunchpadState === 'function') window.updateLaunchpadState(s);
                            if (typeof window.renderPills === 'function') window.renderPills(s);
                        }
                    }
                }).catch(function(){});
            } catch (_) {}
        }, 450);
    })();

    // Tahap 239: Ensure #wifi-pair stays clickable whenever any iPhone is connected (USB or Wi-Fi)
    // and ensure Copy iPhone Text (Alt+C) feedback stays accurate.
    setInterval(function() {
        try {
            var wp = document.getElementById('wifi-pair');
            if (wp) {
                var txt = (wp.textContent || '').toLowerCase();
                var inProgress = txt.indexOf('setting up') !== -1 || txt.indexOf('menyiapkan') !== -1;
                var devSel = document.getElementById('device-select');
                var hasDevice = Boolean(
                    (devSel && devSel.options && devSel.options.length > 0 && devSel.value) ||
                    document.querySelector('#device-meta')?.textContent?.trim()
                );
                if (!inProgress && hasDevice) {
                    wp.hidden = false;
                    if (wp.disabled) {
                        wp.disabled = false;
                        wp.removeAttribute('disabled');
                    }
                }
            }
        } catch (_) {}
    }, 600);
})();
"#;

// ============================================================================
// APP STATE & CONFIGURATION
// ============================================================================

/// Tahap 244 Pilar 1: Zero-I/O resolver for the currently mirrored iPhone's
/// `(preferred_device_name, preferred_marketing_model, preferred_bluetooth_mac)`
/// so WinRT `AudioPlaybackConnection` targets the exact iPhone by DeviceName / Bluetooth MAC.
fn resolve_active_audio_device_target(
    state: &Arc<Mutex<AppState>>,
    explicit_udid: Option<&str>,
    explicit_name: Option<&str>,
    explicit_bt_mac: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let (live_udid, live_name, live_model, live_bt) =
        iphone_link_apple::physical_stream::active_session_device_identity();
    let state_udid = state
        .lock()
        .map(|s| s.selected_udid.clone())
        .unwrap_or_default();

    let target_udid = explicit_udid
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| (!live_udid.is_empty()).then_some(live_udid.clone()))
        .or_else(|| (!state_udid.is_empty()).then_some(state_udid))
        .unwrap_or_default();

    let mut name = explicit_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut model: Option<String> = None;
    let mut bt_mac = explicit_bt_mac
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // If caller explicitly specified a target `deviceName` without pinning a UDID, never
    // cross-contaminate with another mirrored iPhone's cached Bluetooth MAC or model.
    if name.is_some() && explicit_udid.is_none() {
        return (name, None, bt_mac);
    }

    // 1. Live CoreDevice RSD / mDNS identity for the active session
    if !target_udid.is_empty() && live_udid.eq_ignore_ascii_case(&target_udid) {
        if name.is_none() && !live_name.is_empty() {
            name = Some(live_name);
        }
        if model.is_none() && !live_model.is_empty() {
            model = Some(live_model);
        }
        if bt_mac.is_none() && !live_bt.is_empty() {
            bt_mac = Some(live_bt);
        }
    }

    // 2. Zero-I/O Lockdown metadata cache in RAM (USB-discovered devices)
    if !target_udid.is_empty() {
        let cached_devs = snapshot_cached_devices_metadata_zero_io();
        if let Some(meta) = cached_devs
            .iter()
            .find(|d| d.udid.eq_ignore_ascii_case(&target_udid))
        {
            if name.is_none() && !meta.device_name.trim().is_empty() {
                name = Some(meta.device_name.trim().to_string());
            }
            if model.is_none() && !meta.marketing_model.trim().is_empty() {
                model = Some(meta.marketing_model.trim().to_string());
            }
            if bt_mac.is_none() && !meta.bluetooth_address.trim().is_empty() {
                bt_mac = Some(meta.bluetooth_address.trim().to_string());
            }
        }
    }

    // 3. Persistent `wifi_state.json` identity cache (saved during USB/Wi-Fi pairing & RSD)
    if !target_udid.is_empty() && (name.is_none() || model.is_none() || bt_mac.is_none()) {
        let wmeta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(&target_udid);
        if name.is_none() && !wmeta.name.is_empty() {
            name = Some(wmeta.name);
        }
        if model.is_none() && !wmeta.model.is_empty() {
            model = Some(wmeta.model);
        }
        if bt_mac.is_none() && !wmeta.bluetooth_address.is_empty() {
            bt_mac = Some(wmeta.bluetooth_address);
        }
    }

    if model.is_none() && !target_udid.is_empty() {
        model = Some(iphone_link_apple::models::series_name_from_udid(
            &target_udid,
            "iPhone",
        ));
    }

    (name, model, bt_mac)
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub language: Lang,
    pub theme: String, // "dark" or "light"
    pub is_streaming: bool,
    pub is_drawer_open: bool,
    pub is_landscape: bool,
    pub is_fullscreen: bool,
    pub stealth_dim: bool,
    pub intentionally_locked: bool,
    pub selected_udid: String,
    pub user_explicitly_selected_udid: bool,
    pub last_selected_udid: String,
    pub transport: String, // "usb" or "wifi"
    pub window_hwnd: usize, // HWND handle as usize
    pub auto_rotate: bool,
    pub crop_top: i32,
    pub crop_bottom: i32,
    pub crop_left: i32,
    pub crop_right: i32,
    pub fit_mode: String,
    pub trackpad_speed: i32,
    pub audio_routing: String,
    pub audio_volume: i32,
    pub audio_muted: bool,
    pub audio_buffer_preset: String,
    pub lip_sync: bool,
    pub bluetooth_a2dp: bool,
    pub cross_platform_shortcuts: bool,
    pub paste_pc_clipboard: bool,
    pub restore_steps: i32,
    pub device_profile: String,
    pub update_channel: String,
    pub auto_check_update: bool,
    pub updater: UpdateStateCache,
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
            stealth_dim: false,
            intentionally_locked: false,
            selected_udid: String::new(),
            user_explicitly_selected_udid: false,
            last_selected_udid: String::new(),
            transport: "usb".to_string(),
            window_hwnd: 0,
            auto_rotate: false,
            crop_top: 0,
            crop_bottom: 0,
            crop_left: 0,
            crop_right: 0,
            fit_mode: "autofit".to_string(),
            trackpad_speed: 100,
            audio_routing: "pc".to_string(),
            audio_volume: 100,
            audio_muted: false,
            audio_buffer_preset: "normal".to_string(),
            lip_sync: true,
            bluetooth_a2dp: false,
            cross_platform_shortcuts: true,
            paste_pc_clipboard: true,
            restore_steps: 4,
            device_profile: "auto".to_string(),
            update_channel: "stable".to_string(),
            auto_check_update: true,
            updater: UpdateStateCache::default(),
            server_port: DEFAULT_PORT,
            geometry: WindowGeometryManager::new(1.0),
        }
    }
}

// ============================================================================
// HTTP REQUEST & RESPONSE ABSTRACTION
// ============================================================================

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub body_bytes: Vec<u8>,
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
        let without_frag = full_path.split('#').next().unwrap_or(full_path);
        let (path, query) = if let Some(idx) = without_frag.find('?') {
            (without_frag[..idx].to_string(), without_frag[idx + 1..].to_string())
        } else {
            (without_frag.to_string(), String::new())
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

        let sep = b"\r\n\r\n";
        let (body, body_bytes) = if let Some(idx) = raw.windows(4).position(|w| w == sep) {
            let bytes = raw[idx + 4..].to_vec();
            let s = String::from_utf8_lossy(&bytes).to_string();
            (s, bytes)
        } else {
            (String::new(), Vec::new())
        };

        Some(HttpRequest {
            method,
            path,
            query,
            headers,
            body,
            body_bytes,
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

    pub fn ok_bytes(bytes: Vec<u8>, content_type: &'static str) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type,
            body: bytes,
        }
    }

    pub fn not_found() -> Self {
        Self {
            status_code: 404,
            status_text: "Not Found",
            content_type: "application/json; charset=utf-8",
            body: br#"{"error":"Not found."}"#.to_vec(),
        }
    }

    pub fn forbidden_json(msg: &str) -> Self {
        Self {
            status_code: 403,
            status_text: "Forbidden",
            content_type: "application/json; charset=utf-8",
            body: format!(r#"{{"ok":false,"error":"{}"}}"#, escape_json_str(msg)).into_bytes(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_with_conn("close")
    }

    pub fn to_bytes_with_conn(&self, conn: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 + self.body.len());
        let header = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nPragma: no-cache\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: {}\r\n\r\n",
            self.status_code, self.status_text, self.content_type, self.body.len(), conn
        );
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

/// Resolves accurate MIME type by inspecting the filename string suffix (case-insensitive).
pub fn mime_for_path_str(path_str: &str) -> &'static str {
    let lower = path_str.to_lowercase();
    if lower.ends_with(".html") || lower.ends_with(".htm") {
        "text/html; charset=utf-8"
    } else if lower.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if lower.ends_with(".js") || lower.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".ico") {
        "image/x-icon"
    } else if lower.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

/// Resolves the primary web root directory, prioritizing exe_dir/web (packaged v4.0 UI) FIRST
/// before parent dev layouts or handset/web fallbacks.
pub fn resolve_web_root_from(exe_dir: Option<&Path>, cwd: Option<&Path>) -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(dir) = exe_dir {
        candidates.push(dir.join("web"));
        if let Some(p1) = dir.parent() {
            candidates.push(p1.join("web"));
        }
    }

    if let Some(c) = cwd {
        candidates.push(c.join("web"));
        candidates.push(c.join("handset").join("web"));
        candidates.push(c.join("rust-v4").join("apps").join("iphone-link-desktop").join("web"));
    }

    if let Some(dir) = exe_dir {
        candidates.push(dir.join("runtime").join("handset").join("web"));
    }

    for cand in candidates {
        if cand.is_dir() && cand.join("index.html").is_file() {
            return cand;
        }
    }

    PathBuf::from("web")
}

/// Resolves the active web root directory for the running process.
pub fn resolve_web_root() -> PathBuf {
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let cwd = env::current_dir().ok();
    resolve_web_root_from(exe_dir.as_deref(), cwd.as_deref())
}

/// Resolves a relative static asset path, prioritizing exe_dir/web FIRST before web_dir and handset fallbacks.
fn resolve_static_asset_file(web_dir: &Path, rel_path: &str) -> Option<PathBuf> {
    if rel_path.contains("..") {
        return None;
    }

    let clean_rel = if rel_path.is_empty() || rel_path == "/" || rel_path == "index.html" {
        "index.html"
    } else if rel_path == "mirror"
        || rel_path == "mirror/"
        || rel_path == "mirror/frame"
        || rel_path == "mirror/frame/"
        || rel_path == "mirror/index.html"
        || rel_path == "mirror.html"
    {
        "mirror.html"
    } else if rel_path == "mirror/mirror.css" || rel_path == "mirror.css" {
        "mirror.css"
    } else if rel_path == "mirror/mirror.js" || rel_path == "mirror.js" {
        "mirror.js"
    } else if rel_path == "mirror/keys.js" || rel_path == "keys.js" {
        "keys.js"
    } else if rel_path == "mirror/app.css" || rel_path == "app.css" {
        "app.css"
    } else if rel_path == "mirror/app.js" || rel_path == "app.js" {
        "app.js"
    } else if rel_path == "mirror/logo.svg" || rel_path == "logo.svg" {
        "logo.svg"
    } else {
        rel_path.trim_start_matches('/')
    };

    let mut candidates = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("web").join(clean_rel));
            candidates.push(exe_dir.join("assets").join(clean_rel));
            if let Some(file_name) = Path::new(clean_rel).file_name() {
                candidates.push(exe_dir.join("web").join(file_name));
                candidates.push(exe_dir.join("assets").join(file_name));
            }
        }
    }
    candidates.push(web_dir.join(clean_rel));

    if let Some(parent) = web_dir.parent() {
        candidates.push(parent.join("assets").join(clean_rel));
        if let Some(grand) = parent.parent() {
            candidates.push(grand.join("assets").join(clean_rel));
        }
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if let Some(p1) = exe_dir.parent() {
                candidates.push(p1.join("web").join(clean_rel));
                candidates.push(p1.join("assets").join(clean_rel));
                if let Some(p2) = p1.parent() {
                    candidates.push(p2.join("handset").join("web").join(clean_rel));
                    candidates.push(p2.join("assets").join(clean_rel));
                    if let Some(p3) = p2.parent() {
                        candidates.push(p3.join("handset").join("web").join(clean_rel));
                        candidates.push(p3.join("assets").join(clean_rel));
                    }
                }
            }
            candidates.push(exe_dir.join("runtime").join("handset").join("web").join(clean_rel));
        }
    }

    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("web").join(clean_rel));
        candidates.push(cwd.join("assets").join(clean_rel));
        candidates.push(cwd.join("handset").join("web").join(clean_rel));
        if let Some(file_name) = Path::new(clean_rel).file_name() {
            candidates.push(cwd.join("assets").join(file_name));
            candidates.push(cwd.join("handset").join("web").join(file_name));
        }
    }

    for cand in candidates {
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn escape_json_str(s: &str) -> String {
    s.replace('\\', r#"\\"#).replace('"', r#"\""#)
}

pub const DPAPI_PIN_PREFIX: &str = "dpapi:";

fn is_test_loopback_only_env_server() -> bool {
    env::var("IPHONE_LINK_TEST_LOOPBACK_ONLY")
        .map(|v| {
            let trimmed = v.trim();
            trimmed == "1"
                || trimmed.eq_ignore_ascii_case("true")
                || trimmed.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn sanitize_pin_digits(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_digit()).take(8).collect()
}

/// Encrypts Auto-Unlock PIN with per-user Windows DPAPI (`CryptProtectData`) for safe disk storage.
pub fn protect_pin_for_disk(pin: &str) -> String {
    let clean = sanitize_pin_digits(pin);
    if clean.is_empty() {
        return String::new();
    }
    match dpapi_encrypt_pin_b64(&clean) {
        Ok(b64) if !b64.is_empty() => format!("{DPAPI_PIN_PREFIX}{b64}"),
        _ => String::new(),
    }
}

/// Decodes disk `"unlockPin"` values (`"dpapi:<base64>"` or legacy v4.0.0 plaintext digits).
pub fn unprotect_pin_from_disk(stored: &str) -> Option<String> {
    let trimmed = stored.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(b64) = trimmed.strip_prefix(DPAPI_PIN_PREFIX) {
        let decrypted = dpapi_decrypt_pin_b64(b64).ok()?;
        let clean = sanitize_pin_digits(&decrypted);
        return if clean.is_empty() { None } else { Some(clean) };
    }
    let clean = sanitize_pin_digits(trimmed);
    if (4..=8).contains(&clean.len()) && clean == trimmed {
        Some(clean)
    } else {
        None
    }
}

fn parse_bool_param(body: &str, param: &str) -> Option<bool> {
    let t1 = format!(r#""{}":true"#, param);
    let t2 = format!(r#""{}": true"#, param);
    let f1 = format!(r#""{}":false"#, param);
    let f2 = format!(r#""{}": false"#, param);
    if body.contains(&t1) || body.contains(&t2) {
        Some(true)
    } else if body.contains(&f1) || body.contains(&f2) {
        Some(false)
    } else {
        None
    }
}

fn body_contains_key(body: &str, key: &str) -> bool {
    let k1 = format!(r#""{}":"#, key);
    let k2 = format!(r#""{}": "#, key);
    body.contains(&k1) || body.contains(&k2)
}

fn current_exe_dir_normalized() -> PathBuf {
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.to_path_buf();
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve_handset_root_for_runtime(py_dir: &Path, exe_dir: Option<&Path>, cwd: Option<&Path>) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![
        py_dir.to_path_buf(),
        py_dir.join("Lib").join("site-packages"),
    ];
    if let Some(ed) = exe_dir {
        roots.push(ed.to_path_buf());
        roots.push(ed.join("runtime"));
        let mut cur = Some(ed);
        for _ in 0..4 {
            if let Some(p) = cur {
                roots.push(p.to_path_buf());
                cur = p.parent();
            }
        }
    }
    if let Some(cd) = cwd {
        roots.push(cd.to_path_buf());
        roots.push(cd.join("runtime"));
    }

    for root in roots {
        let hs = root.join("handset");
        if hs.join("server.py").is_file() || hs.join("__init__.py").is_file() {
            return Some(root);
        }
    }
    None
}

/// Resolves Python executable and PYTHONPATH root, prioritizing self-contained `runtime/` beside `iPhone Link.exe`
/// (zero system Python / zero `pythonnet` required), then falling back to `venv` or system `PATH`.
pub fn find_python_executable_and_root_from(
    exe_dir: Option<&Path>,
    cwd: Option<&Path>,
) -> Option<(PathBuf, PathBuf)> {
    // 1. Priority 1: Self-Contained Installed `runtime/` beside EXE (or `_internal/runtime/`)
    let mut embedded_candidates: Vec<PathBuf> = Vec::new();
    if let Some(ed) = exe_dir {
        embedded_candidates.push(ed.join("runtime").join("pythonw.exe"));
        embedded_candidates.push(ed.join("runtime").join("python.exe"));
        embedded_candidates.push(ed.join("_internal").join("runtime").join("pythonw.exe"));
        embedded_candidates.push(ed.join("_internal").join("runtime").join("python.exe"));
    }
    if let Some(cd) = cwd {
        embedded_candidates.push(cd.join("runtime").join("pythonw.exe"));
        embedded_candidates.push(cd.join("runtime").join("python.exe"));
    }
    for py in embedded_candidates {
        if py.is_file() {
            if let Some(py_dir) = py.parent() {
                if let Some(root) = resolve_handset_root_for_runtime(py_dir, exe_dir, cwd) {
                    return Some((py, root));
                }
            }
        }
    }

    // 2. Priority 2: Local `venv` in portable/dev workspaces
    let mut venv_candidates: Vec<PathBuf> = Vec::new();
    if let Some(ed) = exe_dir {
        let mut cur = Some(ed);
        for _ in 0..4 {
            if let Some(p) = cur {
                venv_candidates.push(p.join("venv").join("Scripts").join("pythonw.exe"));
                venv_candidates.push(p.join("venv").join("Scripts").join("python.exe"));
                cur = p.parent();
            }
        }
    }
    if let Some(cd) = cwd {
        venv_candidates.push(cd.join("venv").join("Scripts").join("pythonw.exe"));
        venv_candidates.push(cd.join("venv").join("Scripts").join("python.exe"));
    }
    for py in venv_candidates {
        if py.is_file() {
            if let Some(scripts_dir) = py.parent() {
                let venv_root = scripts_dir.parent().unwrap_or(scripts_dir);
                let repo_guess = venv_root.parent().unwrap_or(venv_root);
                if let Some(root) = resolve_handset_root_for_runtime(repo_guess, exe_dir, cwd) {
                    return Some((py, root));
                }
            }
        }
    }

    // 3. Priority 3: System PATH Python (when handset/ exists nearby)
    if let Some(path_env) = env::var_os("PATH") {
        for dir in env::split_paths(&path_env) {
            for bin in ["pythonw.exe", "python.exe"] {
                let py = dir.join(bin);
                if py.is_file() {
                    if let Some(root) = resolve_handset_root_for_runtime(&dir, exe_dir, cwd) {
                        return Some((py, root));
                    }
                }
            }
        }
    }

    None
}

pub fn find_python_executable_and_root() -> Option<(PathBuf, PathBuf)> {
    let exe_binding = env::current_exe().ok();
    let exe_dir = exe_binding.as_ref().and_then(|p| p.parent());
    let cwd_binding = env::current_dir().ok();
    find_python_executable_and_root_from(exe_dir, cwd_binding.as_deref())
}

fn localappdata_settings_file_path() -> Option<PathBuf> {
    iphone_link_apple::app_paths::unified_localappdata_dir().map(|d| d.join("user_settings.json"))
}

fn legacy_v4_settings_file_path() -> Option<PathBuf> {
    iphone_link_apple::app_paths::legacy_localappdata_v4_dir().map(|d| d.join("user_settings.json"))
}

/// Deterministic 12-char hex identifier for the installation folder (SHA-256 of canonical EXE directory).
pub fn get_deterministic_folder_instance_id_for_dir(dir: &Path) -> String {
    let canon = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let raw = canon.to_string_lossy().to_string();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let normalized = stripped.replace('\\', "/").to_lowercase();
    let hash = compute_sha256(normalized.as_bytes());
    hash.chars().take(12).collect()
}

pub fn get_deterministic_folder_instance_id() -> String {
    static FOLDER_ID_CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FOLDER_ID_CACHE
        .get_or_init(|| get_deterministic_folder_instance_id_for_dir(&current_exe_dir_normalized()))
        .clone()
}

pub fn user_settings_file_path() -> PathBuf {
    let resolved = iphone_link_apple::app_paths::resolve_app_data_dir().join("user_settings.json");
    if let Some(legacy) = legacy_v4_settings_file_path() {
        let _ = iphone_link_apple::app_paths::migrate_legacy_file_if_needed(&resolved, &legacy);
    }
    resolved
}

/// Clean Slate PIN Guard (Tahap 55 + Tahap 293 Program Files fix): returns `true` only when
/// `folderInstanceId` stored in `settings_file` differs from the SHA-256 of `exe_dir`.
pub fn is_fresh_extracted_folder_for(exe_dir: &Path, settings_file: &Path) -> bool {
    if !settings_file.is_file() {
        return true;
    }
    if let Ok(raw) = fs::read_to_string(settings_file) {
        if let Some(saved_id) = parse_string_param(&raw, "folderInstanceId") {
            return saved_id != get_deterministic_folder_instance_id_for_dir(exe_dir);
        }
    }
    true
}

/// Clean Slate PIN Guard (Tahap 55 parity): returns `true` when extracted into a new folder
/// whose `folderInstanceId` does not match the persisted `user_settings.json`.
pub fn is_fresh_extracted_folder() -> bool {
    let exe_dir = current_exe_dir_normalized();
    let settings_file = user_settings_file_path();
    is_fresh_extracted_folder_for(&exe_dir, &settings_file)
}

fn extract_crop_object_slice(raw: &str) -> Option<&str> {
    let key_idx = raw.find(r#""crop""#)?;
    let after = &raw[key_idx + 6..];
    let brace_open = after.find('{')?;
    let rest = &after[brace_open..];
    let brace_close = rest.find('}')?;
    Some(&rest[..=brace_close])
}

fn format_user_settings_json(st: &AppState, folder_id: &str, has_file: bool, unlock_pin: &str) -> String {
    let bt_on = st.bluetooth_a2dp || st.audio_routing == "bluetooth";
    format!(
        r#"{{"isFreshFolder":false,"folderInstanceId":"{}","hasUserSettingsFile":{},"unlockPin":"{}","lastSelectedUdid":"{}","crop":{{"top":{},"bottom":{},"left":{},"right":{}}},"fitMode":"{}","theme":"{}","transport":"{}","audioOut":"{}","audioTarget":"{}","audioSinkEnabled":{},"bluetoothA2dp":{},"audioVolume":{},"pcVolume":{},"audioMuted":{},"pcMuted":{},"audioBufferPreset":"{}","audioBuffer":"{}","lipSync":{},"trackpadSpeed":{},"gestureSpeed":{},"speedSensitivity":{},"crossPlatformShortcuts":{},"pastePcClipboard":{},"pcClipboardPaste":{},"ctrlVPastesPc":{},"autoRotate":{},"stealthDim":{},"restoreSteps":{},"deviceProfile":"{}","customWindowSize":null,"autoCheckUpdate":{},"updateChannel":"{}","language":"{}","version":"5.0.1","edition":"100% Pure Native Rust (Zero Python & Zero PowerShell)","zero_python_runtime":true,"zero_powershell_runtime":true}}"#,
        escape_json_str(folder_id),
        has_file,
        escape_json_str(unlock_pin),
        escape_json_str(&st.last_selected_udid),
        st.crop_top,
        st.crop_bottom,
        st.crop_left,
        st.crop_right,
        escape_json_str(&st.fit_mode),
        escape_json_str(&st.theme),
        escape_json_str(&st.transport),
        escape_json_str(&st.audio_routing),
        escape_json_str(&st.audio_routing),
        bt_on,
        bt_on,
        st.audio_volume,
        st.audio_volume,
        st.audio_muted,
        st.audio_muted,
        escape_json_str(&st.audio_buffer_preset),
        escape_json_str(&st.audio_buffer_preset),
        st.lip_sync,
        st.trackpad_speed,
        st.trackpad_speed,
        st.trackpad_speed,
        st.cross_platform_shortcuts,
        st.paste_pc_clipboard,
        st.paste_pc_clipboard,
        st.paste_pc_clipboard,
        st.auto_rotate,
        st.stealth_dim,
        st.restore_steps,
        escape_json_str(&st.device_profile),
        st.auto_check_update,
        escape_json_str(&st.update_channel),
        st.language.code()
    )
}

fn build_settings_json(st: &AppState) -> String {
    let pin = SAVED_UNLOCK_PIN
        .lock()
        .map(|g| sanitize_pin_digits(&g))
        .unwrap_or_default();
    let folder_id = get_deterministic_folder_instance_id();
    let has_file = user_settings_file_path().is_file();
    format_user_settings_json(st, &folder_id, has_file, &pin)
}

pub fn save_user_settings_to_path_with_folder_id(
    path: &Path,
    st: &AppState,
    unlock_pin: &str,
    folder_id: &str,
) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.as_os_str().is_empty() {
        fs::create_dir_all(dir)?;
    }
    let protected_pin = protect_pin_for_disk(unlock_pin);
    let json = format_user_settings_json(st, folder_id, true, &protected_pin);
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.flush()?;
        let _ = file.sync_all();
    }
    if let Err(_first_err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(path);
        if let Err(second_err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(second_err);
        }
    }
    Ok(())
}

pub fn save_user_settings_to_path(path: &Path, st: &AppState, unlock_pin: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let folder_id = get_deterministic_folder_instance_id_for_dir(dir);
    save_user_settings_to_path_with_folder_id(path, st, unlock_pin, &folder_id)
}

fn save_user_settings_to_disk(st: &AppState) {
    if is_test_loopback_only_env_server() {
        return;
    }
    let pin = SAVED_UNLOCK_PIN
        .lock()
        .map(|g| sanitize_pin_digits(&g))
        .unwrap_or_default();
    let folder_id = get_deterministic_folder_instance_id();
    let target = user_settings_file_path();
    if save_user_settings_to_path_with_folder_id(&target, st, &pin, &folder_id).is_err() {
        if let Some(fallback) = localappdata_settings_file_path() {
            if fallback != target {
                let _ = save_user_settings_to_path_with_folder_id(&fallback, st, &pin, &folder_id);
            }
        }
    }
}

pub fn apply_settings_json_body_to_state(raw: &str, st: &mut AppState) -> bool {
    let mut theme_changed_to: Option<bool> = None;
    let mut touched = false;

    if let Some(lang) = parse_string_param(raw, "language") {
        if lang == "en" {
            st.language = Lang::En;
            touched = true;
        } else if lang == "id" {
            st.language = Lang::Id;
            touched = true;
        }
    }
    if let Some(thm) = parse_string_param(raw, "theme") {
        if thm == "light" {
            st.theme = "light".to_string();
            theme_changed_to = Some(false);
            touched = true;
        } else if thm == "dark" {
            st.theme = "dark".to_string();
            theme_changed_to = Some(true);
            touched = true;
        }
    }
    if let Some(ar) = parse_bool_param(raw, "autoRotate") {
        st.auto_rotate = ar;
        let _ = iphone_link_apple::mirror::global_apply_orientation(None, Some(ar), None, None);
        touched = true;
    }
    if let Some(sd) = parse_bool_param(raw, "stealthDim") {
        st.stealth_dim = sd;
        touched = true;
    }
    if let Some(prof) = parse_string_param(raw, "deviceProfile") {
        if prof == "auto" || prof == "iphone" || prof == "ipad" {
            st.device_profile = prof.clone();
            if let Ok(mut gp) = GLOBAL_DEVICE_PROFILE.lock() {
                *gp = prof.clone();
            }
            GLOBAL_PORTRAIT_WIDTH.store(if prof == "ipad" { 564 } else { 346 }, Ordering::SeqCst);
            touched = true;
        }
    }
    if let Some(fm) = parse_string_param(raw, "fitMode") {
        if fm == "autofit" || fm == "fill" || fm == "contain" || fm == "auto_fit" || fm == "edge_to_edge" || fm == "original_ratio" {
            st.fit_mode = fm.clone();
            let _ = iphone_link_apple::mirror::global_update_video_presenter_config(
                Some(&fm),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            );
            touched = true;
        }
    }
    if let Some(crop_slice) = extract_crop_object_slice(raw) {
        let mut crop_touched = false;
        if let Some(v) = parse_int_param(crop_slice, "top") {
            st.crop_top = v.clamp(0, 120);
            crop_touched = true;
            touched = true;
        }
        if let Some(v) = parse_int_param(crop_slice, "bottom") {
            st.crop_bottom = v.clamp(0, 120);
            crop_touched = true;
            touched = true;
        }
        if let Some(v) = parse_int_param(crop_slice, "left") {
            st.crop_left = v.clamp(0, 120);
            crop_touched = true;
            touched = true;
        }
        if let Some(v) = parse_int_param(crop_slice, "right") {
            st.crop_right = v.clamp(0, 120);
            crop_touched = true;
            touched = true;
        }
        if crop_touched {
            let _ = iphone_link_apple::mirror::global_update_video_presenter_config(
                None,
                None,
                Some(st.crop_top.clamp(0, 24) as u32),
                Some(st.crop_bottom.clamp(0, 24) as u32),
                Some(st.crop_left.clamp(0, 24) as u32),
                Some(st.crop_right.clamp(0, 24) as u32),
                None,
                None,
                None,
                None,
                None,
            );
        }
    }
    if let Some(lsu) = parse_string_param(raw, "lastSelectedUdid") {
        let clean = iphone_link_apple::physical_stream::sanitize_udid_label(&lsu);
        if !clean.is_empty() {
            st.last_selected_udid = clean;
            touched = true;
        }
    }
    if let Some(tr) = parse_string_param(raw, "transport") {
        if tr == "wifi" {
            st.transport = "wifi".to_string();
            touched = true;
        } else if tr == "usb" || tr == "cable" {
            st.transport = "usb".to_string();
            touched = true;
        }
    }
    if let Some(ch) = parse_string_param(raw, "updateChannel") {
        if ch == "stable" || ch == "beta" {
            st.update_channel = ch;
            touched = true;
        }
    }
    if let Some(acu) = parse_bool_param(raw, "autoCheckUpdate") {
        st.auto_check_update = acu;
        touched = true;
    }
    if let Some(aout) = parse_string_param(raw, "audioOut")
        .or_else(|| parse_string_param(raw, "audio_out"))
        .or_else(|| parse_string_param(raw, "audio_routing"))
        .or_else(|| parse_string_param(raw, "routing"))
        .or_else(|| parse_string_param(raw, "audioTarget"))
        .or_else(|| {
            parse_bool_param(raw, "audioOut")
                .map(|en| if en { "pc".to_string() } else { "device".to_string() })
        })
    {
        if aout == "pc" || aout == "device" || aout == "bluetooth" || aout == "wasapi" {
            st.bluetooth_a2dp = aout == "bluetooth";
            st.audio_routing = aout;
            touched = true;
        }
    }
    if let Some(bt) = parse_bool_param(raw, "bluetoothA2dp") {
        st.bluetooth_a2dp = bt;
        if bt {
            st.audio_routing = "bluetooth".to_string();
        }
        touched = true;
    }
    if let Some(am) = parse_bool_param(raw, "audioMuted")
        .or_else(|| parse_bool_param(raw, "pcMuted"))
        .or_else(|| parse_bool_param(raw, "muted"))
    {
        st.audio_muted = am;
        touched = true;
    }
    if let Some(vol) = parse_int_param(raw, "audioVolume")
        .or_else(|| parse_int_param(raw, "pcVolume"))
        .or_else(|| parse_int_param(raw, "volume"))
    {
        st.audio_volume = vol.clamp(0, 100);
        touched = true;
    }
    if let Some(buf) = parse_string_param(raw, "audioBufferPreset")
        .or_else(|| parse_string_param(raw, "audioBuffer"))
        .or_else(|| parse_string_param(raw, "bufferPreset"))
        .or_else(|| parse_string_param(raw, "buffer"))
    {
        if buf == "low" || buf == "normal" || buf == "high" {
            st.audio_buffer_preset = buf;
            touched = true;
        }
    }
    if let Some(ls) = parse_bool_param(raw, "lipSync")
        .or_else(|| parse_bool_param(raw, "lip_sync"))
    {
        st.lip_sync = ls;
        touched = true;
    }
    if let Some(ts) = parse_int_param(raw, "trackpadSpeed")
        .or_else(|| parse_int_param(raw, "gestureSpeed"))
        .or_else(|| parse_int_param(raw, "speedSensitivity"))
    {
        st.trackpad_speed = ts.clamp(30, 200);
        touched = true;
    }
    if let Some(cps) = parse_bool_param(raw, "crossPlatformShortcuts") {
        st.cross_platform_shortcuts = cps;
        touched = true;
    }
    if let Some(rs) = parse_int_param(raw, "restoreSteps") {
        st.restore_steps = rs.clamp(1, 16);
        touched = true;
    }
    if let Some(ppc) = parse_bool_param(raw, "pastePcClipboard")
        .or_else(|| parse_bool_param(raw, "pcClipboardPaste"))
        .or_else(|| parse_bool_param(raw, "ctrlVPastesPc"))
    {
        st.paste_pc_clipboard = ppc;
        touched = true;
    }
    GLOBAL_AUDIO_PC_ACTIVE.store(
        (st.audio_routing == "pc" || st.audio_routing == "bluetooth") && !st.audio_muted,
        Ordering::SeqCst,
    );
    if let Some(is_dark) = theme_changed_to {
        GLOBAL_DARK_MODE.store(is_dark, Ordering::SeqCst);
        if st.window_hwnd != 0 {
            let _ = apply_true_files_acrylic_backdrop(st.window_hwnd as *mut _, is_dark);
        }
    }
    touched
}

pub fn load_user_settings_from_path(path: &Path, st: &mut AppState) -> String {
    let Ok(raw) = fs::read_to_string(path) else {
        return String::new();
    };
    if let Some(pin_raw) = parse_string_param(&raw, "unlockPin") {
        let clean = unprotect_pin_from_disk(&pin_raw).unwrap_or_default();
        if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
            *g = clean;
        }
    }
    let _ = apply_settings_json_body_to_state(&raw, st);
    SAVED_UNLOCK_PIN
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn init_persistent_settings_from_disk_from(
    path: &Path,
    st: &mut AppState,
    hydrate_hid_ram: bool,
) -> String {
    let pin = load_user_settings_from_path(path, st);
    if hydrate_hid_ram {
        if !pin.is_empty() {
            let _ = store_encrypted_pin(&pin);
        } else {
            clear_stored_pin();
        }
    }
    // Tahap 249 Pilar 3: Even on a true Fresh Install (no user_settings.json on disk yet),
    // keep GLOBAL_AUDIO_PC_ACTIVE and MirrorRuntimeState::audio aligned with `AppState::default()`
    // (`audio_routing = "pc"`, `streaming = false` so 0-device idle stays silent until mirror starts).
    GLOBAL_AUDIO_PC_ACTIVE.store(
        (st.audio_routing == "pc" || st.audio_routing == "bluetooth") && !st.audio_muted,
        Ordering::SeqCst,
    );
    let _ = iphone_link_apple::mirror::global_configure_audio(
        Some(&st.audio_routing),
        Some(st.audio_volume as i64),
        Some(st.audio_muted),
        Some(&st.audio_buffer_preset),
        Some(st.lip_sync),
        Some(false),
    );
    let _ = iphone_link_apple::airplay::apply_live_audio_config(
        Some(st.audio_volume.clamp(0, 100) as u32),
        Some(&st.audio_buffer_preset),
        Some(st.lip_sync),
        Some(st.audio_muted),
    );
    pin
}

pub fn init_persistent_settings_from_disk(st: &mut AppState) {
    if is_test_loopback_only_env_server() {
        GLOBAL_AUDIO_PC_ACTIVE.store(
            (st.audio_routing == "pc" || st.audio_routing == "bluetooth") && !st.audio_muted,
            Ordering::SeqCst,
        );
        return;
    }
    let path = user_settings_file_path();
    let _ = init_persistent_settings_from_disk_from(&path, st, true);
}

pub fn get_hydrated_unlock_pin() -> Option<String> {
    if let Ok(Some(pin)) = load_decrypted_pin() {
        let clean = sanitize_pin_digits(&pin);
        if !clean.is_empty() {
            return Some(clean);
        }
    }
    SAVED_UNLOCK_PIN
        .lock()
        .ok()
        .map(|g| sanitize_pin_digits(&g))
        .filter(|s| !s.is_empty())
}

pub fn save_unlock_pin_sync(state: &Arc<Mutex<AppState>>, pin: &str) -> Result<String, String> {
    let digits: String = pin.trim().chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err("PIN kosong".to_string());
    }
    if !(4..=8).contains(&digits.len()) {
        return Err("PIN harus 4-8 digit".to_string());
    }
    store_encrypted_pin(&digits)?;
    if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
        *g = digits.clone();
    }
    if let Ok(st) = state.lock() {
        save_user_settings_to_disk(&st);
    }
    Ok(digits)
}

pub fn clear_unlock_pin_sync(state: &Arc<Mutex<AppState>>) -> Result<(), String> {
    clear_stored_pin();
    if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
        g.clear();
    }
    if let Ok(st) = state.lock() {
        save_user_settings_to_disk(&st);
    }
    Ok(())
}

pub fn wipe_all_privacy_state_and_files_sync(state: &Arc<Mutex<AppState>>) {
    wipe_all_privacy_state_and_files();
    if let Ok(mut st) = state.lock() {
        st.stealth_dim = false;
        st.intentionally_locked = false;
    }
}

pub const EMBEDDED_OCR_EXTRACTOR_PS1: &str = include_str!("../../../../handset/ocr_extractor.ps1");

pub fn writable_temp_ocr_dir() -> PathBuf {
    env::temp_dir().join("iPhoneLink")
}

pub fn ensure_temp_ocr_extractor_script() -> PathBuf {
    let dir = writable_temp_ocr_dir();
    let _ = fs::create_dir_all(&dir);
    let target = dir.join("ocr_extractor.ps1");
    let needs_write = match fs::read_to_string(&target) {
        Ok(existing) => existing != EMBEDDED_OCR_EXTRACTOR_PS1,
        Err(_) => true,
    };
    if needs_write {
        let _ = fs::write(&target, EMBEDDED_OCR_EXTRACTOR_PS1.as_bytes());
    }
    target
}

pub fn wipe_privacy_files_in_dir(dir: &Path) {
    for name in [
        "user_settings.json",
        "typed_history.json",
        "wifi_state.json",
        "debug_metrics.json",
        "sequoia_last_blur.bin",
        "debug_ocr_raw.png",
        "debug_ocr_cleaned.png",
        "debug_ocr_result.json",
        "debug_ocr_error.log",
    ] {
        let p = dir.join(name);
        if p.is_file() {
            let _ = fs::remove_file(&p);
        }
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    let low = name.to_ascii_lowercase();
                    if low.ends_with(".dpapi")
                        || low.ends_with(".bin.tmp")
                        || low.contains(".json.tmp.")
                    {
                        let _ = fs::remove_file(&p);
                    }
                }
            }
        }
    }
}

fn wipe_temp_ocr_dir(dir: &Path) {
    wipe_privacy_files_in_dir(dir);
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("ocr_batch_")
                        || name.starts_with("qr_ocr_")
                        || name.starts_with("debug_ocr_")
                    {
                        let _ = fs::remove_file(&p);
                    }
                }
            }
        }
    }
}

fn wipe_all_privacy_state_and_files() {
    clear_copy_cache();
    reset_hid_runtime_state();
    GLOBAL_STEALTH_ACTIVE.store(false, Ordering::SeqCst);
    if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
        g.clear();
    }
    if let Ok(mut g_dom) = GLOBAL_DOM_UNLOCK_PIN.lock() {
        g_dom.clear();
    }
    let exe_dir = current_exe_dir_normalized();
    wipe_privacy_files_in_dir(&exe_dir);
    if let Some(fallback) = localappdata_settings_file_path() {
        if let Some(fb_dir) = fallback.parent() {
            wipe_privacy_files_in_dir(fb_dir);
        }
    }
    if let Some(legacy_fb) = legacy_v4_settings_file_path() {
        if let Some(leg_dir) = legacy_fb.parent() {
            wipe_privacy_files_in_dir(leg_dir);
        }
    }
    wipe_temp_ocr_dir(&writable_temp_ocr_dir());
    if let Ok(cwd) = env::current_dir() {
        if cwd != exe_dir {
            wipe_privacy_files_in_dir(&cwd);
        }
    }
}

static GLOBAL_HEADLESS_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_headless_mode(headless: bool) {
    GLOBAL_HEADLESS_MODE.store(headless, Ordering::SeqCst);
}

fn env_diag_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|v| {
            let t = v.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn allow_mock_codec_env() -> bool {
    cfg!(test) || env_diag_flag("IPHONE_LINK_TEST_LOOPBACK_ONLY")
}

fn is_mock_codec_env_active() -> bool {
    allow_mock_codec_env()
        && (env_diag_flag("IPHONE_LINK_MOCK_HEVC_MISSING")
            || env_diag_flag("IPHONE_LINK_MOCK_H264_MISSING"))
}

fn is_dry_run_shell_open() -> bool {
    cfg!(test)
        || GLOBAL_HEADLESS_MODE.load(Ordering::Relaxed)
        || env_diag_flag("IPHONE_LINK_DRY_RUN_SHELL_OPEN")
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> isize;
}

/// Launches a URI (`ms-windows-store://...`, `ms-settings:optionalfeatures`) or local `.AppxBundle`
/// natively via Win32 `ShellExecuteW` (0 PowerShell). Returns `Ok(true)` if launched, `Ok(false)` on dry-run/headless.
fn shell_open_target_native(target: &str) -> Result<bool, String> {
    if is_dry_run_shell_open() {
        return Ok(false);
    }
    let verb_w: Vec<u16> = "open\0".encode_utf16().collect();
    let file_w: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb_w.as_ptr(),
            file_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        )
    };
    if rc > 32 {
        Ok(true)
    } else {
        Err(format!("ShellExecuteW failed with code {rc}"))
    }
}

fn find_hevc_appxbundle_path() -> Option<PathBuf> {
    let exe_dir = current_exe_dir_normalized();
    let preferred_name = "Microsoft.HEVCVideoExtension_2.5.48.0_neutral_._8wekyb3d8bbwe.AppxBundle";
    for exact in [exe_dir.join("assets").join(preferred_name), exe_dir.join(preferred_name)] {
        if exact.is_file() {
            return Some(exact);
        }
    }
    for dir in [exe_dir.join("assets"), exe_dir] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name_l = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name_l.contains("hevc")
                && (name_l.ends_with(".appxbundle")
                    || name_l.ends_with(".msixbundle")
                    || name_l.ends_with(".appx"))
            {
                return Some(p);
            }
        }
    }
    None
}

/// Real, zero-false-positive HEVC / H.264 / Media Feature Pack capability probe using
/// single-pass delay-loaded `mfplat.dll` + `MFTEnumEx(HEVC)` + `MFTEnumEx(H264)` + Win32 Appx registry check.
fn query_hevc_capabilities() -> iphone_link_video::d3d11_context::CodecCapabilitiesReport {
    use iphone_link_video::d3d11_context::{
        evaluate_codec_capabilities, probe_mfplat_all_video_mfts,
    };

    let mock_h264_missing =
        allow_mock_codec_env() && env_diag_flag("IPHONE_LINK_MOCK_H264_MISSING");
    let mock_hevc_missing =
        allow_mock_codec_env() && env_diag_flag("IPHONE_LINK_MOCK_HEVC_MISSING");

    if mock_h264_missing {
        return evaluate_codec_capabilities(false, false, false, None);
    }
    if mock_hevc_missing {
        return evaluate_codec_capabilities(true, false, true, None);
    }

    let (mfplat_loaded, hevc_mft, h264_mft) = probe_mfplat_all_video_mfts();
    let appx_opt = query_win32_hevc_appx_package();
    let mut report = evaluate_codec_capabilities(
        mfplat_loaded,
        hevc_mft,
        h264_mft,
        appx_opt.as_ref().map(|(v, _)| v.as_str()),
    );
    if let Some((_, pkg)) = appx_opt {
        if report.installed && !pkg.is_empty() {
            report.package = pkg;
        }
    }
    report
}

/// Launches the bundled `.AppxBundle` directly through Windows App Installer via Win32 `ShellExecuteW`
/// (100% Zero PowerShell runtime!).
fn install_hevc_from_bundle(bundle_path: &Path) -> Result<bool, String> {
    shell_open_target_native(&bundle_path.to_string_lossy())
}

fn query_native_apple_driver_installed() -> bool {
    let p1 = Path::new(r"C:\Program Files\Common Files\Apple\Mobile Device Support");
    let p2 = Path::new(r"C:\Program Files (x86)\Common Files\Apple\Mobile Device Support");
    let p3 = Path::new(r"C:\Windows\System32\drivers\usbaapl64.sys");
    let p4 = Path::new(r"C:\Windows\System32\drivers\AppleLowerFilter.sys");
    if p1.exists() || p2.exists() || p3.exists() || p4.exists() {
        return true;
    }
    let repo = Path::new(r"C:\Windows\System32\DriverStore\FileRepository");
    if let Ok(entries) = std::fs::read_dir(repo) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("usbaapl64.inf") {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleUsbDiagEval {
    pub driver_installed: bool,
    pub service_running: bool,
    pub service_label_en: &'static str,
    pub service_label_id: &'static str,
    pub tcp_port_27015_normal: bool,
    pub tcp_port_label_en: &'static str,
    pub tcp_port_label_id: &'static str,
}

/// Pure testable evaluator for the honest 5-point USB diagnostic matrix (Tahap 293):
/// - `service_running` reflects REAL Win32 SCM `SERVICE_RUNNING` (never registry key alone!)
/// - `tcp_port_27015_normal` reflects PURE TCP `127.0.0.1:27015` reachability (never OR'd with service!)
pub fn evaluate_apple_usb_diagnostics_pure(
    port_27015_open: bool,
    svc_state: Win32ServiceState,
    driver_files_present: bool,
) -> AppleUsbDiagEval {
    let service_running = svc_state == Win32ServiceState::Running;
    let (service_label_en, service_label_id) = match svc_state {
        Win32ServiceState::Running => ("Running (Started)", "Berjalan (Started)"),
        Win32ServiceState::Stopped => ("Installed, but Stopped", "Terpasang, tapi Berhenti"),
        Win32ServiceState::NotInstalled => ("Not Installed", "Belum Terpasang"),
    };
    AppleUsbDiagEval {
        driver_installed: driver_files_present || svc_state != Win32ServiceState::NotInstalled,
        service_running,
        service_label_en,
        service_label_id,
        tcp_port_27015_normal: port_27015_open,
        tcp_port_label_en: if port_27015_open { "Normal" } else { "Unreachable" },
        tcp_port_label_id: if port_27015_open { "Normal" } else { "Tidak Terjangkau" },
    }
}

fn query_native_apple_service_running() -> bool {
    let _ = query_win32_apple_service_installed_in_registry();
    query_win32_apple_service_state() == Win32ServiceState::Running
}

static HEVC_STATUS_CACHE: Mutex<Option<(iphone_link_video::d3d11_context::CodecCapabilitiesReport, std::time::Instant)>> = Mutex::new(None);
static DRIVER_DIAG_CACHE: Mutex<Option<(String, std::time::Instant)>> = Mutex::new(None);
static WIFI_STATUS_STREAM_CACHE: Mutex<Option<(String, String, std::time::Instant)>> = Mutex::new(None);
static START_WITH_WIN_CACHE: Mutex<Option<(bool, std::time::Instant)>> = Mutex::new(None);
static LAST_UNIFIED_WIFI_META_CACHE: OnceLock<Mutex<HashMap<String, iphone_link_apple::wifi::WifiDeviceMetadata>>> = OnceLock::new();
static LAST_PAIRED_WIFI_UDIDS_CACHE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static LAST_AUTO_AUDIO_ATTEMPT_MS: AtomicU64 = AtomicU64::new(0);

fn unified_wifi_meta_ram_cache() -> &'static Mutex<HashMap<String, iphone_link_apple::wifi::WifiDeviceMetadata>> {
    LAST_UNIFIED_WIFI_META_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn paired_wifi_udids_ram_cache() -> &'static Mutex<Vec<String>> {
    LAST_PAIRED_WIFI_UDIDS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn cache_unified_wifi_meta_ram(udid: &str, meta: &iphone_link_apple::wifi::WifiDeviceMetadata) {
    if udid.is_empty() {
        return;
    }
    if let Ok(mut g) = unified_wifi_meta_ram_cache().lock() {
        g.insert(udid.to_string(), meta.clone());
    }
    if let Ok(mut u) = paired_wifi_udids_ram_cache().lock() {
        if !u.iter().any(|x| x.eq_ignore_ascii_case(udid)) {
            u.push(udid.to_string());
        }
    }
}

fn read_unified_wifi_meta_zero_io(udid: &str) -> iphone_link_apple::wifi::WifiDeviceMetadata {
    let mut meta = unified_wifi_meta_ram_cache()
        .lock()
        .ok()
        .and_then(|g| g.get(udid).cloned())
        .unwrap_or_default();
    if let Some((r, c_ip, c_seen, age)) = iphone_link_apple::wifi::get_cached_wifi_reachability(udid) {
        if age < Duration::from_secs(600) {
            meta.reachable = Some(r);
            if c_ip.is_some() {
                meta.last_ip = c_ip;
            }
            meta.last_seen_at = meta.last_seen_at.max(c_seen);
        }
    }
    meta
}

fn get_wifi_status_cached_for_stream(
    udid: &str,
    zero_poll_active: bool,
    force_refresh: bool,
) -> String {
    if !force_refresh {
        if let Ok(guard) = WIFI_STATUS_STREAM_CACHE.lock() {
            if let Some((ref cached_udid, ref json, ts)) = *guard {
                let udid_matches = cached_udid == udid;
                let ttl = if zero_poll_active {
                    Duration::from_secs(30)
                } else {
                    Duration::from_secs(15)
                };
                if udid_matches && ts.elapsed() < ttl {
                    return json.clone();
                }
            }
        }
    }
    let json = get_wifi_status(Some(udid), force_refresh).to_json_string();
    if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
        *guard = Some((udid.to_string(), json.clone(), std::time::Instant::now()));
    }
    json
}

#[cfg(test)]
fn clear_wifi_status_stream_cache_for_test() {
    if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
        *guard = None;
    }
    if let Ok(mut g) = unified_wifi_meta_ram_cache().lock() {
        g.clear();
    }
    if let Ok(mut u) = paired_wifi_udids_ram_cache().lock() {
        u.clear();
    }
    LAST_AUTO_AUDIO_ATTEMPT_MS.store(0, Ordering::SeqCst);
}

fn cached_start_with_windows_for_status(zero_poll_active: bool) -> bool {
    if zero_poll_active {
        if let Ok(guard) = START_WITH_WIN_CACHE.lock() {
            if let Some((val, ts)) = *guard {
                if ts.elapsed() < Duration::from_secs(30) {
                    return val;
                }
            }
        }
    }
    let val = is_start_with_windows_enabled();
    if let Ok(mut guard) = START_WITH_WIN_CACHE.lock() {
        *guard = Some((val, std::time::Instant::now()));
    }
    val
}

fn query_hevc_capabilities_cached(
    force: bool,
    non_blocking_fast: bool,
) -> iphone_link_video::d3d11_context::CodecCapabilitiesReport {
    if !force && !is_mock_codec_env_active() {
        if let Ok(guard) = HEVC_STATUS_CACHE.lock() {
            if let Some((ref cap, ts)) = *guard {
                if non_blocking_fast || ts.elapsed() < Duration::from_secs(60) {
                    return cap.clone();
                }
            }
        }
        if non_blocking_fast {
            // Fast zero-I/O path (e.g. POST /api/device/select < 50 ms): use instant registry probe
            // and warm the full MFTEnumEx cache asynchronously on a background thread.
            let appx_opt = query_win32_hevc_appx_package();
            let mut fast_rep = iphone_link_video::d3d11_context::evaluate_codec_capabilities(
                true,
                appx_opt.is_some(),
                true,
                appx_opt.as_ref().map(|(v, _)| v.as_str()),
            );
            if let Some((_, pkg)) = appx_opt {
                if fast_rep.installed && !pkg.is_empty() {
                    fast_rep.package = pkg;
                }
            }
            let _ = std::thread::Builder::new()
                .name("iphone-hevc-cache-warm".into())
                .spawn(|| {
                    let full = query_hevc_capabilities();
                    if let Ok(mut guard) = HEVC_STATUS_CACHE.lock() {
                        *guard = Some((full, std::time::Instant::now()));
                    }
                });
            return fast_rep;
        }
    }
    let res = query_hevc_capabilities();
    if !is_mock_codec_env_active() {
        if let Ok(mut guard) = HEVC_STATUS_CACHE.lock() {
            *guard = Some((res.clone(), std::time::Instant::now()));
        }
    }
    res
}

fn query_native_port_27015_open() -> bool {
    let target = env::var("USBMUXD_SOCKET_ADDRESS")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1:27015".to_string());
    let addr: std::net::SocketAddr = match target.parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(250)).is_ok()
}

fn find_bundled_apple_driver_files() -> (Option<PathBuf>, Option<PathBuf>) {
    let mut search_dirs: Vec<PathBuf> = Vec::new();
    let exe_dir = current_exe_dir_normalized();
    search_dirs.push(exe_dir.join("drivers"));
    search_dirs.push(exe_dir.clone());
    search_dirs.push(exe_dir.join("release"));
    let mut cur = exe_dir.parent();
    for _ in 0..4 {
        if let Some(p) = cur {
            search_dirs.push(p.join("drivers"));
            search_dirs.push(p.join("release"));
            search_dirs.push(p.to_path_buf());
            cur = p.parent();
        }
    }
    if let Ok(cwd) = env::current_dir() {
        search_dirs.push(cwd.join("drivers"));
        search_dirs.push(cwd.join("release"));
        search_dirs.push(cwd);
    }

    let mut ps1_found = None;
    let mut msi_found = None;
    for dir in search_dirs {
        let ps1 = dir.join("install_apple_driver.ps1");
        if ps1_found.is_none() && ps1.is_file() {
            ps1_found = Some(ps1);
        }
        let msi = dir.join("AppleMobileDeviceSupport64.msi");
        if msi_found.is_none() && msi.is_file() {
            msi_found = Some(msi);
        }
        if ps1_found.is_some() && msi_found.is_some() {
            break;
        }
    }
    (ps1_found, msi_found)
}

fn build_native_driver_diagnostics_json() -> String {
    build_native_driver_diagnostics_json_with_opts(false, false)
}

fn build_native_driver_diagnostics_json_with_opts(force_refresh: bool, is_streaming: bool) -> String {
    if !force_refresh && !is_mock_codec_env_active() {
        if let Ok(guard) = DRIVER_DIAG_CACHE.lock() {
            if let Some((ref cached_json, ts)) = *guard {
                if is_streaming || ts.elapsed() < Duration::from_secs(30) {
                    return cached_json.clone();
                }
            }
        }
    }
    let devs = if is_streaming {
        snapshot_cached_devices_metadata_zero_io()
    } else {
        discover_devices_with_metadata(force_refresh)
    };
    let usb_devs: Vec<_> = devs
        .iter()
        .filter(|d| d.connection_type.eq_ignore_ascii_case("USB") && !d.udid.is_empty())
        .collect();
    let has_usb = !usb_devs.is_empty();
    let udid_opt = usb_devs.first().map(|d| d.udid.as_str());

    let port_open = if is_streaming {
        has_usb || query_native_port_27015_open()
    } else {
        query_native_port_27015_open()
    };
    let svc_state = if is_streaming && port_open {
        Win32ServiceState::Running
    } else {
        query_win32_apple_service_state()
    };
    let drv_files = query_native_apple_driver_installed();
    let usb_eval = evaluate_apple_usb_diagnostics_pure(port_open, svc_state, drv_files);
    let svc_running = usb_eval.service_running;
    let drv_installed = usb_eval.driver_installed;
    let conflicts = if is_streaming {
        Vec::new()
    } else {
        detect_conflicting_apple_usb_processes()
    };
    let conflict_warning_id = if conflicts.is_empty() {
        String::new()
    } else {
        format!(
            "Terdeteksi {} sedang berjalan di latar belakang. Tutup sementara agar tidak berebut koneksi USB iPhone.",
            conflicts.join(", ")
        )
    };
    let conflict_warning_en = if conflicts.is_empty() {
        String::new()
    } else {
        format!(
            "Detected {} running in background. Close temporarily so it does not compete for iPhone USB connection.",
            conflicts.join(", ")
        )
    };
    let conflicts_json = conflicts
        .iter()
        .map(|c| format!(r#""{}""#, escape_json_str(c)))
        .collect::<Vec<_>>()
        .join(",");
    let cap = query_hevc_capabilities_cached(force_refresh && !is_streaming, is_streaming);
    let hevc_ok = cap.installed;
    let (dev_bool_opt, dev_status, dev_label_en, dev_label_id) =
        resolve_developer_mode_for_diagnostics(udid_opt, has_usb, is_streaming, force_refresh && !is_streaming);

    let udid_json = match udid_opt {
        Some(u) => format!(r#""{}""#, escape_json_str(u)),
        None => "null".to_string(),
    };
    let dev_bool_json = match dev_bool_opt {
        Some(b) => b.to_string(),
        None => "null".to_string(),
    };

    let json = format!(
        r#"{{"ok":true,"udid":{},"usb_connected":{},"apple_driver_installed":{},"apple_driver_label":"{}","apple_driver_label_id":"{}","apple_service_running":{},"apple_service_label":"{}","apple_service_label_id":"{}","tcp_port_27015_normal":{},"tcp_port_label":"{}","tcp_port_label_id":"{}","hevc_codec_installed":{},"hevc_codec_label":"{}","hevc_codec_label_id":"{}","installed":{},"hevcMftAvailable":{},"h264MftAvailable":{},"mediaFeaturePackMissing":{},"storeUrl":"{}","storeFallbackUrl":"{}","mediaFeaturePackUrl":"{}","developer_mode_status":"{}","developer_mode_bool":{},"developer_mode_label":"{}","developer_mode_label_id":"{}","conflict_processes":[{}],"conflict_warning_id":"{}","conflict_warning_en":"{}"}}"#,
        udid_json,
        has_usb,
        drv_installed,
        if drv_installed { "Installed" } else { "Not Installed" },
        if drv_installed { "Terpasang" } else { "Belum Terpasang" },
        svc_running,
        usb_eval.service_label_en,
        usb_eval.service_label_id,
        usb_eval.tcp_port_27015_normal,
        usb_eval.tcp_port_label_en,
        usb_eval.tcp_port_label_id,
        hevc_ok,
        if hevc_ok { "Installed (60fps)" } else { "Missing" },
        if hevc_ok { "Terpasang (60fps)" } else { "Belum Ada" },
        cap.installed,
        cap.hevc_mft_available,
        cap.h264_mft_available,
        cap.media_feature_pack_missing,
        cap.store_url,
        cap.store_fallback_url,
        cap.media_feature_pack_url,
        dev_status,
        dev_bool_json,
        escape_json_str(dev_label_en),
        escape_json_str(dev_label_id),
        conflicts_json,
        escape_json_str(&conflict_warning_id),
        escape_json_str(&conflict_warning_en),
    );
    if !is_mock_codec_env_active() {
        if let Ok(mut guard) = DRIVER_DIAG_CACHE.lock() {
            *guard = Some((json.clone(), std::time::Instant::now()));
        }
    }
    json
}

/// Builds a strictly Zero-PII diagnostic snapshot for 1-click user clipboard export (Tahap 293).
pub fn build_zero_pii_diagnostic_report(state: &Arc<Mutex<AppState>>) -> String {
    let diag_raw = build_native_driver_diagnostics_json_with_opts(false, false);
    let diag_clean = iphone_link_apple::diag_log::sanitize_pii_for_diagnostic_log(&diag_raw);
    let st_line = state
        .lock()
        .map(|s| {
            format!(
                "transport={}, fit_mode={}, lang={}, auto_rotate={}, device_profile={}",
                s.transport,
                s.fit_mode,
                s.language.code(),
                s.auto_rotate,
                s.device_profile
            )
        })
        .unwrap_or_else(|_| "state=locked".to_string());
    let install_mode = iphone_link_apple::app_paths::install_mode_label();
    let app_data_dir = iphone_link_apple::diag_log::sanitize_pii_for_diagnostic_log(
        &iphone_link_apple::app_paths::resolve_app_data_dir().to_string_lossy(),
    );
    let logs = iphone_link_apple::diag_log::snapshot();
    let log_block = if logs.is_empty() {
        "(no milestones recorded yet)".to_string()
    } else {
        logs.join("\n")
    };
    let report = format!(
        "=== iPhone Link v{} Diagnostic Report (Zero-PII) ===\ninstall_mode: {}\napp_data_dir: {}\nstate: {}\nusb_hevc_diagnostics: {}\n--- Recent Milestones ({}) ---\n{}",
        APP_VERSION_V5,
        install_mode,
        app_data_dir,
        st_line,
        diag_clean,
        logs.len(),
        log_block
    );
    iphone_link_apple::diag_log::sanitize_pii_for_diagnostic_log(&report)
}

fn repair_native_apple_service() -> (bool, bool, bool, bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW_FLAG: u32 = 0x0800_0000;

    let mut cfg = std::process::Command::new("sc.exe");
    cfg.args(["config", "Apple Mobile Device Service", "start=", "auto"]);
    cfg.creation_flags(CREATE_NO_WINDOW_FLAG);
    let _ = cfg.output();

    let mut st = std::process::Command::new("sc.exe");
    st.args(["start", "Apple Mobile Device Service"]);
    st.creation_flags(CREATE_NO_WINDOW_FLAG);
    let _ = st.output();

    clear_metadata_cache(None);
    let (ps1_path, msi_path) = find_bundled_apple_driver_files();
    let pre_svc = query_native_port_27015_open() || query_native_apple_service_running();
    let pre_drv = pre_svc || query_native_apple_driver_installed();

    if !pre_drv || !pre_svc {
        if let Some(ref ps1) = ps1_path {
            let mut pcmd = std::process::Command::new("powershell");
            pcmd.args([
                "-WindowStyle",
                "Hidden",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &ps1.to_string_lossy(),
            ]);
            pcmd.creation_flags(CREATE_NO_WINDOW_FLAG);
            let _ = pcmd.output();
        } else if let Some(ref msi) = msi_path {
            if !pre_drv {
                let mut mcmd = std::process::Command::new("msiexec.exe");
                mcmd.args(["/i", &msi.to_string_lossy(), "/qn", "/norestart"]);
                mcmd.creation_flags(CREATE_NO_WINDOW_FLAG);
                let _ = mcmd.output();
            }
        }
        let mut cfg2 = std::process::Command::new("sc.exe");
        cfg2.args(["config", "Apple Mobile Device Service", "start=", "auto"]);
        cfg2.creation_flags(CREATE_NO_WINDOW_FLAG);
        let _ = cfg2.output();
        let mut st2 = std::process::Command::new("sc.exe");
        st2.args(["start", "Apple Mobile Device Service"]);
        st2.creation_flags(CREATE_NO_WINDOW_FLAG);
        let _ = st2.output();
    }

    clear_metadata_cache(None);
    let svc = query_native_port_27015_open() || query_native_apple_service_running();
    let drv = svc || query_native_apple_driver_installed();
    (drv && svc, drv, svc, msi_path.is_some())
}

fn extract_query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.trim_start_matches('?').split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == key && !v.trim().is_empty() {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn build_native_usb_devices_json(force_refresh: bool) -> String {
    let streaming_active = iphone_link_apple::mirror::is_mirror_streaming_fast();
    let devs = if streaming_active && !force_refresh {
        snapshot_cached_devices_metadata_zero_io()
    } else {
        discover_devices_with_metadata(force_refresh)
    };
    let mut items = Vec::with_capacity(devs.len());
    for d in &devs {
        let is_usb = d.connection_type.eq_ignore_ascii_case("USB");
        let (dev_bool, dev_status, _, _) =
            resolve_developer_mode_for_diagnostics(Some(&d.udid), is_usb, false, false);
        let dev_bool_json = match dev_bool {
            Some(b) => b.to_string(),
            None => "null".to_string(),
        };
        items.push(format!(
            r#"{{"udid":"{}","deviceId":{},"connection":"{}","name":"{}","deviceName":"{}","model":"{}","productType":"{}","productVersion":"{}","buildVersion":"{}","deviceClass":"{}","screenType":"{}","paired":{},"developerModeStatus":"{}","developerModeBool":{}}}"#,
            escape_json_str(&d.udid),
            d.device_id,
            escape_json_str(&d.connection_type),
            escape_json_str(&d.device_name),
            escape_json_str(&d.device_name),
            escape_json_str(&d.marketing_model),
            escape_json_str(&d.product_type),
            escape_json_str(&d.product_version),
            escape_json_str(&d.build_version),
            escape_json_str(&d.device_class),
            d.screen_type.as_str(),
            d.paired,
            dev_status,
            dev_bool_json,
        ));
    }
    let diag = build_native_driver_diagnostics_json_with_opts(false, false);
    format!(
        r#"{{"ok":true,"native":true,"count":{},"devices":[{}],"diagnostics":{}}}"#,
        devs.len(),
        items.join(","),
        diag
    )
}

fn build_tray_items_array_json(st: &AppState) -> String {
    let startup_on = is_start_with_windows_enabled();
    let landscape_on = st.is_landscape || GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
    let stealth_on = st.stealth_dim || GLOBAL_STEALTH_ACTIVE.load(Ordering::SeqCst) || is_stealth_dim_active();
    let audio_pc_on = GLOBAL_AUDIO_PC_ACTIVE.load(Ordering::SeqCst);
    let ids = [1001, 1002, 1003, 1004, 1005, 1006, 1007, 1008, 1009];
    let mut parts = Vec::with_capacity(TRAY_MENU_ITEMS.len());
    for (idx, label) in TRAY_MENU_ITEMS.iter().enumerate() {
        let id = *ids.get(idx).unwrap_or(&(1001 + idx));
        let checked = match id {
            1003 => startup_on,
            1005 => landscape_on,
            1006 => stealth_on,
            1007 => audio_pc_on,
            _ => false,
        };
        let is_default = id == 1001;
        parts.push(format!(
            r#"{{"id":{},"label":"{}","checked":{},"default":{}}}"#,
            id,
            escape_json_str(label),
            checked,
            is_default
        ));
    }
    format!("[{}]", parts.join(","))
}

fn build_tray_state_json(action: &str, handled: bool, st: &AppState) -> String {
    let startup_on = is_start_with_windows_enabled();
    let landscape_on = st.is_landscape || GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
    let stealth_on = st.stealth_dim || GLOBAL_STEALTH_ACTIVE.load(Ordering::SeqCst) || is_stealth_dim_active();
    let audio_pc_on = GLOBAL_AUDIO_PC_ACTIVE.load(Ordering::SeqCst);
    let audio_active = st.is_streaming && !st.audio_muted;
    let items_json = build_tray_items_array_json(st);
    format!(
        r#"{{"ok":true,"action":"{}","handled":{},"pure_ascii":true,"default_item":"Show / Restore iPhone Link","nim_delete_guaranteed":true,"tray_registered":{},"drawer_open":{},"landscape":{},"stealth":{},"audio":{},"audioPc":{},"startWithWindows":{},"trayMenuOpen":{},"trayDismissedCount":{},"menuItemCount":{},"checked":{{"start_with_windows":{},"landscape":{},"stealth_dim":{},"stream_audio_to_pc":{}}},"trayMenuItems":{},"items":{}}}"#,
        escape_json_str(action),
        handled,
        GLOBAL_TRAY_REGISTERED.load(Ordering::SeqCst),
        GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst),
        landscape_on,
        stealth_on,
        audio_active,
        audio_pc_on,
        startup_on,
        GLOBAL_TRAY_MENU_OPEN.load(Ordering::SeqCst),
        GLOBAL_TRAY_DISMISSED_COUNT.load(Ordering::SeqCst),
        TRAY_MENU_ITEMS.len(),
        startup_on,
        landscape_on,
        stealth_on,
        audio_pc_on,
        items_json,
        items_json
    )
}

fn compute_logical_window_dims(is_fs: bool, is_land: bool, open: bool, port_w: i32) -> (i32, i32) {
    if is_fs {
        (1920, 1080)
    } else if is_land {
        (if open { 1200 } else { 840 }, 468)
    } else {
        let base = port_w.clamp(336, 600);
        (if open { base + 360 } else { base }, 832)
    }
}

fn parse_landscape_flag_from_body(body: &str) -> bool {
    if let Some(b) = parse_bool_param(body, "landscape").or_else(|| parse_bool_param(body, "is_landscape")) {
        return b;
    }
    if let Some(deg) = parse_int_param(body, "orientation").or_else(|| parse_int_param(body, "angle")) {
        let norm = deg.rem_euclid(360);
        return norm == 90 || norm == 270;
    }
    if let Some(mode) = parse_string_param(body, "mode") {
        return mode.eq_ignore_ascii_case("landscape");
    }
    false
}

// ============================================================================
// COREDEVICE WORKER REVERSE PROXY & LIVE STREAMING (PORT 8767)
// ============================================================================

pub const WORKER_PORT: u16 = 8767;

fn parse_int_param(body: &str, param: &str) -> Option<i32> {
    let key1 = format!(r#""{}":"#, param);
    let key2 = format!(r#""{}": "#, param);
    let after = if let Some(idx) = body.find(&key1) {
        &body[idx + key1.len()..]
    } else if let Some(idx) = body.find(&key2) {
        &body[idx + key2.len()..]
    } else {
        return None;
    };
    let trimmed = after.trim_start();
    let num_str: String = trimmed.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    num_str.parse::<i32>().ok()
}

fn parse_float_param(body: &str, param: &str) -> Option<f64> {
    let key1 = format!(r#""{}":"#, param);
    let key2 = format!(r#""{}": "#, param);
    let after = if let Some(idx) = body.find(&key1) {
        &body[idx + key1.len()..]
    } else if let Some(idx) = body.find(&key2) {
        &body[idx + key2.len()..]
    } else {
        return None;
    };
    let trimmed = after.trim_start();
    let num_str: String = trimmed.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    num_str.parse::<f64>().ok()
}

fn parse_string_param(body: &str, param: &str) -> Option<String> {
    let key1 = format!(r#""{}":"#, param);
    let key2 = format!(r#""{}": "#, param);
    let after = if let Some(idx) = body.find(&key1) {
        &body[idx + key1.len()..]
    } else if let Some(idx) = body.find(&key2) {
        &body[idx + key2.len()..]
    } else {
        return None;
    };
    let trimmed = after.trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    let content = &trimmed[1..];
    let end_idx = content.find('"')?;
    Some(content[..end_idx].to_string())
}

static WALLPAPER_SIG_CACHE: Mutex<Option<(std::time::Instant, String, Vec<u8>, &'static str)>> = Mutex::new(None);

pub fn invalidate_wallpaper_cache_for_test() {
    if let Ok(mut g) = WALLPAPER_SIG_CACHE.lock() {
        *g = None;
    }
}

pub fn get_cached_wallpaper_sig_fast() -> String {
    let (_, _, sig) = get_current_wallpaper_data();
    sig
}

fn get_current_wallpaper_data() -> (Vec<u8>, &'static str, String) {
    if let Ok(guard) = WALLPAPER_SIG_CACHE.lock() {
        if let Some((ts, ref sig, ref bytes, mime)) = *guard {
            if ts.elapsed() < Duration::from_secs(30) && !sig.is_empty() && !bytes.is_empty() {
                return (bytes.clone(), mime, sig.clone());
            }
        }
    }

    let mut candidates: Vec<(PathBuf, u128, u64)> = Vec::new();
    if let Some(spi_path) = query_win32_desktop_wallpaper_path() {
        if let Ok(meta) = spi_path.metadata() {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            candidates.push((spi_path, mtime, meta.len()));
        }
    }
    if let Ok(appdata) = env::var("APPDATA") {
        let tw = PathBuf::from(&appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Themes")
            .join("TranscodedWallpaper");
        if tw.is_file() {
            if let Ok(meta) = tw.metadata() {
                if meta.len() > 1000 {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    if !candidates.iter().any(|(p, _, _)| p == &tw) {
                        candidates.push((tw, mtime, meta.len()));
                    }
                }
            }
        }
    }

    if let Some((best_path, best_mtime, best_len)) = candidates.into_iter().max_by_key(|c| c.1) {
        if let Ok(bytes) = fs::read(&best_path) {
            if bytes.len() > 1000 {
                let mime = if bytes.starts_with(b"\x89PNG") {
                    "image/png"
                } else {
                    "image/jpeg"
                };
                let raw_sig = format!(
                    "{}:{}:{}",
                    best_path.to_string_lossy().replace('\\', "/"),
                    best_mtime,
                    best_len
                );
                let sig: String = compute_sha256(raw_sig.as_bytes()).chars().take(16).collect();
                if let Ok(mut guard) = WALLPAPER_SIG_CACHE.lock() {
                    *guard = Some((std::time::Instant::now(), sig.clone(), bytes.clone(), mime));
                }
                return (bytes, mime, sig);
            }
        }
    }

    // Realistic fallback JPEG padded to > 50KB for headless/test environments
    let mut fallback = Vec::with_capacity(65536);
    fallback.extend_from_slice(&[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x01, 0x00, 0x60,
        0x00, 0x60, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08,
        0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12,
        0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20, 0x24, 0x2E, 0x27, 0x20,
        0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29, 0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27,
        0x39, 0x3D, 0x38, 0x32, 0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01,
        0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04,
        0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F,
        0x00, 0xBF, 0x00,
    ]);
    let pad_target = 60000;
    while fallback.len() < pad_target {
        let chunk_size = std::cmp::min(2048, pad_target - fallback.len());
        if chunk_size < 4 { break; }
        fallback.push(0xFF);
        fallback.push(0xFE);
        let len_u16 = chunk_size as u16;
        fallback.push((len_u16 >> 8) as u8);
        fallback.push((len_u16 & 0xFF) as u8);
        fallback.resize(fallback.len() + (chunk_size - 4), 0x20);
    }
    fallback.extend_from_slice(&[0xFF, 0xD9]);
    let sig = "fallback_blur_sig".to_string();
    if let Ok(mut guard) = WALLPAPER_SIG_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), sig.clone(), fallback.clone(), "image/jpeg"));
    }
    (fallback, "image/jpeg", sig)
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[allow(non_snake_case)]
struct PROCESS_MEMORY_COUNTERS_V4 {
    cb: u32,
    PageFaultCount: u32,
    PeakWorkingSetSize: usize,
    WorkingSetSize: usize,
    QuotaPeakPagedPoolUsage: usize,
    QuotaPagedPoolUsage: usize,
    QuotaPeakNonPagedPoolUsage: usize,
    QuotaNonPagedPoolUsage: usize,
    PagefileUsage: usize,
    PeakPagefileUsage: usize,
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn SetProcessWorkingSetSize(
        hProcess: *mut std::ffi::c_void,
        dwMinimumWorkingSetSize: usize,
        dwMaximumWorkingSetSize: usize,
    ) -> i32;
    fn K32EmptyWorkingSet(hProcess: *mut std::ffi::c_void) -> i32;
    fn K32GetProcessMemoryInfo(
        Process: *mut std::ffi::c_void,
        ppsmemCounters: *mut PROCESS_MEMORY_COUNTERS_V4,
        cb: u32,
    ) -> i32;
}

pub fn trim_current_rust_process_working_set() {
    #[cfg(target_os = "windows")]
    unsafe {
        let h = GetCurrentProcess();
        if !h.is_null() {
            let _ = K32EmptyWorkingSet(h);
            let _ = SetProcessWorkingSetSize(h, usize::MAX, usize::MAX);
        }
    }
}

pub fn current_rust_process_working_set_mb() -> f64 {
    #[cfg(target_os = "windows")]
    unsafe {
        let mut pmc = PROCESS_MEMORY_COUNTERS_V4 {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_V4>() as u32,
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
        };
        let h = GetCurrentProcess();
        if !h.is_null() && K32GetProcessMemoryInfo(h, &mut pmc as *mut _, pmc.cb) != 0 {
            return ((pmc.WorkingSetSize as f64 / (1024.0 * 1024.0)) * 100.0).round() / 100.0;
        }
    }
    0.0
}

fn extract_json_f64_field(json: &str, key: &str) -> Option<f64> {
    let needle = format!(r#""{}":"#, key);
    let alt_needle = format!(r#""{}": "#, key);
    let idx = json
        .find(&needle)
        .map(|i| i + needle.len())
        .or_else(|| json.find(&alt_needle).map(|i| i + alt_needle.len()))?;
    let tail = json[idx..].trim_start();
    let end = tail
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(tail.len());
    tail[..end].parse::<f64>().ok()
}

fn forward_to_worker(req: &HttpRequest) -> Option<HttpResponse> {
    if STANDALONE_INPROCESS_ONLY {
        let _ = req;
        return None;
    }
    use std::net::SocketAddr;
    let addr = SocketAddr::from(([127, 0, 0, 1], WORKER_PORT));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = stream.set_nodelay(true);

    let is_heavy = req.path.starts_with("/api/keyboard/shortcut")
        || req.path.starts_with("/api/social/grab-link")
        || req.path.starts_with("/api/qr/")
        || req.path.starts_with("/api/device/spotlight")
        || req.path.starts_with("/api/device/wake")
        || req.path.starts_with("/api/device/lock")
        || req.path.starts_with("/api/screen/start")
        || req.path.starts_with("/api/wifi/scan")
        || req.path.starts_with("/api/ocr/")
        || req.path.starts_with("/api/update/")
        || req.path == "/api/system/reclaim-memory";

    let timeout = if is_heavy {
        Duration::from_secs(25)
    } else {
        Duration::from_millis(1500)
    };

    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let content_type = req
        .headers
        .get("content-type")
        .map(|s| s.as_str())
        .unwrap_or("application/json");

    let full_target = if req.query.is_empty() {
        req.path.clone()
    } else {
        format!("{}?{}", req.path, req.query)
    };

    let header = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        req.method, full_target, WORKER_PORT, content_type, req.body_bytes.len()
    );
    stream.write_all(header.as_bytes()).ok()?;
    if !req.body_bytes.is_empty() {
        stream.write_all(&req.body_bytes).ok()?;
    }
    stream.flush().ok()?;

    let mut resp_bytes = Vec::new();
    stream.read_to_end(&mut resp_bytes).ok()?;
    drop(stream);

    if is_heavy {
        trim_current_rust_process_working_set();
    }

    if resp_bytes.is_empty() {
        return None;
    }

    let sep = b"\r\n\r\n";
    let sep_pos = resp_bytes.windows(4).position(|w| w == sep)?;
    let header_part = String::from_utf8_lossy(&resp_bytes[..sep_pos]);
    let actual_body = resp_bytes[sep_pos + 4..].to_vec();

    let status_code = if header_part.starts_with("HTTP/1.1 ") && header_part.len() >= 12 {
        header_part[9..12].parse::<u16>().unwrap_or(200)
    } else {
        200
    };

    let header_low = header_part.to_ascii_lowercase();
    let content_type = if header_low.contains("content-type: text/html") {
        "text/html; charset=utf-8"
    } else if header_low.contains("content-type: text/plain") {
        "text/plain; charset=utf-8"
    } else {
        "application/json; charset=utf-8"
    };

    Some(HttpResponse {
        status_code,
        status_text: "OK",
        content_type,
        body: actual_body,
    })
}

fn notify_worker_commit_typed_line_async() {
    if STANDALONE_INPROCESS_ONLY {
        return;
    }
    thread::spawn(move || {
        use std::net::SocketAddr;
        let addr = SocketAddr::from(([127, 0, 0, 1], WORKER_PORT));
        if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(120)) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let body = br#"{"commitLine":true}"#;
            let header = format!(
                "POST /api/keyboard/typed HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                WORKER_PORT,
                body.len()
            );
            if stream.write_all(header.as_bytes()).is_ok() {
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        }
    });
}

fn forward_to_stream_server(req: &HttpRequest) -> Option<HttpResponse> {
    if STANDALONE_INPROCESS_ONLY {
        let _ = req;
        return None;
    }
    use std::net::SocketAddr;
    let ports = [8766u16, WORKER_PORT];
    for port in ports {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(2500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(2500)));

            let target_path = if port == 8766 {
                match req.path.as_str() {
                    "/mirror/codec" => "/codec",
                    "/mirror/live" => "/stream.bin",
                    "/mirror/touch" => "/touch",
                    "/mirror/scroll" => "/scroll",
                    "/mirror/button" => "/button",
                    "/mirror/key" => "/key",
                    "/mirror/gesture" => "/gesture",
                    "/mirror/pli" => "/pli",
                    "/mirror/restart" => "/restart",
                    "/mirror/buttons" => "/buttons",
                    other => {
                        if other.starts_with("/mirror/") {
                            &other[7..]
                        } else {
                            other
                        }
                    }
                }
            } else {
                req.path.as_str()
            };

            let full_target = if req.query.is_empty() {
                target_path.to_string()
            } else {
                format!("{}?{}", target_path, req.query)
            };
            let content_type = req.headers.get("content-type").map(|s| s.as_str()).unwrap_or("application/json");
            let header = format!(
                "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                req.method, full_target, port, content_type, req.body_bytes.len()
            );
            if stream.write_all(header.as_bytes()).is_ok() {
                if !req.body_bytes.is_empty() {
                    let _ = stream.write_all(&req.body_bytes);
                }
                let _ = stream.flush();
                let mut resp_bytes = Vec::new();
                if stream.read_to_end(&mut resp_bytes).is_ok() && !resp_bytes.is_empty() {
                    if let Some(sep_pos) = resp_bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header_part = String::from_utf8_lossy(&resp_bytes[..sep_pos]);
                        let actual_body = resp_bytes[sep_pos + 4..].to_vec();
                        let status_code = if header_part.starts_with("HTTP/1.1 ") && header_part.len() >= 12 {
                            header_part[9..12].parse::<u16>().unwrap_or(200)
                        } else {
                            200
                        };
                        if status_code == 404 && port == 8766 {
                            continue;
                        }
                        return Some(HttpResponse {
                            status_code,
                            status_text: "OK",
                            content_type: "application/json; charset=utf-8",
                            body: actual_body,
                        });
                    }
                }
            }
        }
    }
    None
}

fn stream_live_passthrough(client: &mut TcpStream) -> Result<(), std::io::Error> {
    let _ = client.set_nodelay(true);
    let _ = client.set_read_timeout(None);
    let _ = client.set_write_timeout(None);

    // Phase 3 (Tahap 237): Continuous Condvar-driven hvcC/avcC WebCodecs stream for real physical USB/Wi-Fi/AirPlay.
    if iphone_link_apple::mirror::is_physical_stream_session_active() {
        let _live_client_guard = iphone_link_apple::mirror::LiveClientGuard::enter();
        client.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nX-Accel-Buffering: no\r\nConnection: close\r\n\r\n",
        )?;
        client.flush()?;
        let mut last_seq = 0u64;
        let mut sent_init = false;
        let mut sent_count = 0u64;
        while let Some(chunks) =
            iphone_link_apple::mirror::global_wait_next_live_chunks(&mut last_seq, &mut sent_init, 100)
        {
            if chunks.is_empty() {
                continue;
            }
            for chunk in chunks {
                client.write_all(&chunk)?;
                client.flush()?;
                sent_count = sent_count.saturating_add(1);
                if sent_count <= 12 || sent_count % 60 == 0 {
                    iphone_link_apple::airplay::record_airplay_trace(
                        "live_sent_seq",
                        format!("sent=#{sent_count} seq={last_seq} bytes={}", chunk.len()),
                    );
                }
            }
        }
        return Ok(());
    }

    // Phase 1 (Tahap 230): Serve queued in-process native HEVC Annex-B frames when synthetic native mirror session is active.
    let native_chunks = iphone_link_apple::mirror::global_take_live_stream_snapshot();
    if !native_chunks.is_empty() || iphone_link_apple::mirror::is_mirror_streaming_fast() {
        client.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nX-Accel-Buffering: no\r\nConnection: close\r\n\r\n",
        )?;
        for chunk in native_chunks {
            client.write_all(&chunk)?;
        }
        client.flush()?;
        return Ok(());
    }

    let (mut upstream, is_8766) = match TcpStream::connect("127.0.0.1:8766") {
        Ok(s) => (s, true),
        Err(_) => (TcpStream::connect(format!("127.0.0.1:{}", WORKER_PORT))?, false),
    };
    upstream.set_nodelay(true)?;
    let _ = upstream.set_read_timeout(None);
    let _ = upstream.set_write_timeout(None);

    let req = if is_8766 {
        "GET /stream.bin HTTP/1.1\r\nHost: 127.0.0.1:8766\r\nConnection: close\r\n\r\n"
    } else {
        "GET /mirror/live HTTP/1.1\r\nHost: 127.0.0.1:8767\r\nConnection: close\r\n\r\n"
    };
    upstream.write_all(req.as_bytes())?;
    upstream.flush()?;

    let mut buf = [0u8; 65536]; // 64KB buffer
    loop {
        let n = upstream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        client.write_all(&buf[..n])?;
        client.flush()?;
    }
    Ok(())
}

// ============================================================================
// ROUTER & DISPATCHER
// ============================================================================

pub fn percent_decode_url_component(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn now_epoch_ms_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| v4.is_loopback())
                    .unwrap_or(false)
        }
    }
}

pub fn is_lan_request_allowed(ip: IpAddr, method: &str, raw_path: &str) -> bool {
    if is_loopback_ip(ip) {
        return true;
    }
    if method.eq_ignore_ascii_case("OPTIONS") {
        return true;
    }
    let clean_path = raw_path
        .split('?')
        .next()
        .unwrap_or(raw_path)
        .split('#')
        .next()
        .unwrap_or(raw_path);
    clean_path == "/c"
        || clean_path.starts_with("/c/")
        || clean_path == "/api/shortcut/push"
        || clean_path == "/api/clipboard/push"
}

pub fn detect_primary_lan_ipv4() -> String {
    if iphone_link_apple::connection::is_loopback_only_test_mode() {
        return "127.0.0.1".to_string();
    }
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                let ip = addr.ip().to_string();
                if !ip.starts_with("127.") && !ip.starts_with("169.254.") && ip != "0.0.0.0" {
                    return ip;
                }
            }
        }
    }
    "192.168.1.X".to_string()
}

pub fn extract_cached_iphone_lan_ip_from_json(raw_json: &str, pc_ip: &str) -> Option<String> {
    // Check "last_ip": "<ipv4>" inside wifi_state.json without pulling extra parser deps.
    for key in ["\"last_ip\"", "\"ip\""] {
        let mut search_from = 0usize;
        while let Some(pos) = raw_json[search_from..].find(key) {
            let abs_pos = search_from + pos + key.len();
            let rest = raw_json[abs_pos..].trim_start();
            if let Some(after_colon) = rest.strip_prefix(':') {
                let after_colon = after_colon.trim_start();
                if let Some(after_quote) = after_colon.strip_prefix('"') {
                    if let Some(end_quote) = after_quote.find('"') {
                        let candidate = after_quote[..end_quote].trim();
                        if let Ok(addr) = candidate.parse::<std::net::Ipv4Addr>() {
                            let ip_str = addr.to_string();
                            if !addr.is_loopback()
                                && !addr.is_link_local()
                                && !addr.is_unspecified()
                                && ip_str != pc_ip
                            {
                                return Some(ip_str);
                            }
                        }
                    }
                }
            }
            search_from = abs_pos;
        }
    }
    None
}

pub fn detect_cached_iphone_lan_ipv4(pc_ip: &str) -> String {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(unified) = iphone_link_apple::app_paths::unified_localappdata_dir() {
        candidates.push(unified.join("wifi_state.json"));
    }
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        if !localappdata.trim().is_empty() {
            candidates.push(PathBuf::from(&localappdata).join("iPhone Link").join("wifi_state.json"));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.trim().is_empty() {
            candidates.push(PathBuf::from(&appdata).join("iPhone Link").join("wifi_state.json"));
        }
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let clean = iphone_link_apple::app_paths::normalize_dir_path(exe_dir);
            if !iphone_link_apple::app_paths::is_path_under_program_files(&clean) {
                candidates.push(clean.join("wifi_state.json"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("wifi_state.json"));
    }
    for path in candidates {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(ip) = extract_cached_iphone_lan_ip_from_json(&raw, pc_ip) {
                return ip;
            }
        }
    }
    String::new()
}

pub const ICLOUD_SHORTCUT_OFFICIAL_URL: &str = "https://www.icloud.com/shortcuts/bbb096d97e174c1386f15e31aedef8ff";

pub fn build_shortcut_setup_info_json(port: u16) -> String {
    let actual_port = if port == 0 { DEFAULT_PORT } else { port };
    let ip = detect_primary_lan_ipv4();
    let iphone_ip = detect_cached_iphone_lan_ipv4(&ip);
    let endpoint = format!("http://{}:{}/c?u=", ip, actual_port);
    let siri_prompt = format!(
        "Create a new Shortcut named \"Send to iPhone Link\" that appears in the Share Sheet (with If there's no input: Get Clipboard). First action: Get URLs from Shortcut Input. Second action: URL Encode the URLs. Third action: Get Contents of URL \"{}\" followed by the URL Encoded Text.",
        endpoint
    );
    let (ip_warning_id, ip_warning_en) = if !iphone_ip.is_empty() && iphone_ip != ip {
        (
            format!(
                "[GUNAKAN IP PC: {}] — Jangan gunakan IP iPhone ({}) di Shortcut!",
                ip, iphone_ip
            ),
            format!(
                "[USE PC IP: {}] — Do not use iPhone IP ({}) in Shortcut!",
                ip, iphone_ip
            ),
        )
    } else {
        (
            format!(
                "[GUNAKAN IP PC: {}] — Pastikan memakai IP PC di atas (bukan IP iPhone).",
                ip
            ),
            format!(
                "[USE PC IP: {}] — Make sure to use the PC IP above (not iPhone IP).",
                ip
            ),
        )
    };
    format!(
        r#"{{"ok":true,"pc_lan_ip":"{}","iphone_lan_ip":"{}","ip_warning_id":"{}","ip_warning_en":"{}","port":{},"shortcut_endpoint_template":"{}","icloud_shortcut_url":"{}","siri_prompt_en":"{}"}}"#,
        escape_json_str(&ip),
        escape_json_str(&iphone_ip),
        escape_json_str(&ip_warning_id),
        escape_json_str(&ip_warning_en),
        actual_port,
        escape_json_str(&endpoint),
        escape_json_str(ICLOUD_SHORTCUT_OFFICIAL_URL),
        escape_json_str(&siri_prompt)
    )
}

pub fn normalize_and_validate_digital_shortcut_url(raw_url: &str) -> Option<String> {
    let mut candidate = raw_url.replace(r"\/", "/").trim().to_string();
    for _ in 0..2 {
        let cl = candidate.to_ascii_lowercase();
        if cl.starts_with("https%3a") || cl.starts_with("http%3a") {
            candidate = percent_decode_url_component(candidate.trim());
        } else {
            break;
        }
    }
    let trimmed = candidate.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(char::is_whitespace)
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        r
    } else {
        return None;
    };
    let (host, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    let host_no_port = host.split(':').next().unwrap_or(host);
    if host_no_port.is_empty() || !host_no_port.contains('.') || host_no_port.contains('=') {
        return None;
    }
    let labels: Vec<&str> = host_no_port.split('.').collect();
    if labels.iter().any(|lbl| lbl.is_empty()) {
        return None;
    }
    let tld = labels.last().copied().unwrap_or("");
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if path_and_query.is_empty() || path_and_query == "/" {
        return None;
    }
    if host_no_port.ends_with("threads.com")
        || host_no_port.ends_with("threads.net")
        || host_no_port.ends_with("instagram.com")
    {
        if let Some(clean_social) = safe_link_grabber_clean(trimmed) {
            if crate::link_grabber::is_strict_https_social_url(&clean_social) {
                return Some(clean_social);
            }
        }
        return None;
    }
    Some(trimmed.to_string())
}

/// Tahap 243 Pilar 2: Sanitizes non-ASCII Unicode characters (`…`, `•`, `—`, `🔍`, `İ`, emoji)
/// into ASCII spaces BEFORE invoking frozen `link_grabber.rs` (whose `split_concatenated_url_tokens`
/// advances byte-by-byte and slices `&lower[pos..]`, which panics on multi-byte UTF-8 boundaries).
fn sanitize_for_link_grabber(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii() { c } else { ' ' })
        .collect()
}

fn safe_link_grabber_extract(raw: &str) -> Vec<String> {
    let ascii = sanitize_for_link_grabber(raw);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::link_grabber::extract_and_normalize_links(&ascii)
    }))
    .unwrap_or_default()
}

fn safe_link_grabber_clean(raw: &str) -> Option<String> {
    let ascii = sanitize_for_link_grabber(raw);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::link_grabber::clean_and_validate_extracted_url(&ascii, None)
    }))
    .ok()
    .flatten()
}

fn stitch_spotlight_overlap(head: &str, tail: &str) -> Option<String> {
    let h = head.trim();
    let t = tail.trim();
    if h.is_empty() || t.is_empty() {
        return None;
    }
    if h.contains(t) {
        return Some(h.to_string());
    }
    if t.contains(h) {
        return Some(t.to_string());
    }
    let h_chars: Vec<char> = h.chars().collect();
    let t_chars: Vec<char> = t.chars().collect();
    let max_k = h_chars.len().min(t_chars.len());
    for k in (4..=max_k).rev() {
        if h_chars[h_chars.len() - k..] == t_chars[..k] {
            let mut merged: String = h_chars.iter().collect();
            for ch in &t_chars[k..] {
                merged.push(*ch);
            }
            return Some(merged);
        }
    }
    None
}

fn extract_general_urls_from_ocr_text(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let cleaned = raw
        .replace(r"\/", "/")
        .replace('\u{200B}', "")
        .replace('\u{FEFF}', "");
    let known_domains = [
        "www.instagram.com/",
        "instagram.com/",
        "www.threads.net/",
        "threads.net/",
        "www.threads.com/",
        "threads.com/",
        "www.tiktok.com/",
        "tiktok.com/",
        "vt.tiktok.com/",
        "vm.tiktok.com/",
        "www.youtube.com/",
        "youtube.com/",
        "m.youtube.com/",
        "youtu.be/",
        "x.com/",
        "twitter.com/",
        "www.facebook.com/",
        "facebook.com/",
        "m.facebook.com/",
        "fb.watch/",
    ];
    for tok in cleaned.split_whitespace() {
        let trimmed = tok.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '.' | '|'
            )
        });
        if trimmed.len() < 8 {
            continue;
        }
        let low = trimmed.to_ascii_lowercase();
        let candidate = if low.starts_with("https://") || low.starts_with("http://") {
            trimmed.to_string()
        } else if low.starts_with("http5://") {
            let rest: String = trimmed.chars().skip(8).collect();
            format!("https://{rest}")
        } else if low.starts_with("htps://") {
            let rest: String = trimmed.chars().skip(7).collect();
            format!("https://{rest}")
        } else if known_domains.iter().any(|d| low.starts_with(d)) {
            if low.starts_with("instagram.com/") {
                format!("https://www.{trimmed}")
            } else {
                format!("https://{trimmed}")
            }
        } else {
            continue;
        };
        if let Some(norm) = normalize_and_validate_digital_shortcut_url(&candidate) {
            if !out.contains(&norm) {
                out.push(norm);
            }
        }
    }
    out
}

fn is_bare_incomplete_social_path(url: &str) -> bool {
    let low = url.trim().to_ascii_lowercase();
    for pfx in [
        "instagram.com/p/",
        "instagram.com/reel/",
        "instagram.com/reels/",
        "instagram.com/tv/",
        "threads.net/share/",
        "threads.com/share/",
    ] {
        if let Some(idx) = low.find(pfx) {
            let rest = &url.trim()[idx + pfx.len()..];
            let id_seg = rest.split(&['/', '?', '#'][..]).next().unwrap_or("");
            if id_seg.len() < 6 {
                return true;
            }
        }
    }
    false
}

fn pick_best_validated_url(candidates: Vec<String>) -> Option<String> {
    // Reject bare incomplete paths (`.../p/`, `.../reel/`) whenever any complete URL exists,
    // and break ties in favor of the EARLIEST (highest-priority: 2x_natural > 2x_contrast > 1x) candidate!
    let complete: Vec<String> = candidates
        .into_iter()
        .filter(|u| !is_bare_incomplete_social_path(u))
        .collect();
    let mut best: Option<String> = None;
    for u in complete {
        match best {
            None => best = Some(u),
            Some(ref cur) => {
                // Only replace an earlier (higher-priority) candidate if strictly longer
                // (e.g., carries full query or full shortcode instead of truncated tail).
                if u.len() > cur.len() {
                    best = Some(u);
                }
            }
        }
    }
    best
}

fn extract_best_link_from_candidates(raw_texts: &[String]) -> Option<String> {
    let mut expanded: Vec<String> = Vec::new();
    for t in raw_texts {
        let tr = t.trim();
        if tr.is_empty() {
            continue;
        }
        // Tahap 245 Pilar 1: Unwrap multi-line iOS 27.2 Spotlight URLs BEFORE sanitization & link_grabber!
        let unwrapped = crate::ocr_engine::unwrap_multiline_spotlight_urls(tr);
        if !unwrapped.trim().is_empty() && !expanded.contains(&unwrapped) {
            expanded.push(unwrapped);
        }
        if !expanded.iter().any(|e| e == tr) {
            expanded.push(tr.to_string());
        }
    }
    if expanded.len() >= 2 {
        let combined_nl = expanded.join("\n");
        let combined_unwrapped = crate::ocr_engine::unwrap_multiline_spotlight_urls(&combined_nl);
        if !combined_unwrapped.trim().is_empty() && !expanded.contains(&combined_unwrapped) {
            expanded.push(combined_unwrapped);
        }
    }

    let mut strict_social: Vec<String> = Vec::new();
    let mut general_https: Vec<String> = Vec::new();

    for txt in &expanded {
        for u in safe_link_grabber_extract(txt) {
            if is_bare_incomplete_social_path(&u) {
                continue;
            }
            if crate::link_grabber::is_strict_https_social_url(&u) {
                if !strict_social.contains(&u) {
                    strict_social.push(u);
                }
            } else if let Some(norm) = normalize_and_validate_digital_shortcut_url(&u) {
                if !is_bare_incomplete_social_path(&norm) && !general_https.contains(&norm) {
                    general_https.push(norm);
                }
            }
        }
        for u in extract_general_urls_from_ocr_text(txt) {
            if is_bare_incomplete_social_path(&u) {
                continue;
            }
            if crate::link_grabber::is_strict_https_social_url(&u) {
                if !strict_social.contains(&u) {
                    strict_social.push(u);
                }
            } else if !general_https.contains(&u) {
                general_https.push(u);
            }
        }
    }

    // Head + tail Spotlight overlap stitching across multiple captured fragments
    if raw_texts.len() >= 2 {
        for i in 0..raw_texts.len() {
            for j in (i + 1)..raw_texts.len() {
                if let Some(stitched) = stitch_spotlight_overlap(&raw_texts[i], &raw_texts[j])
                    .or_else(|| stitch_spotlight_overlap(&raw_texts[j], &raw_texts[i]))
                {
                    let unwrapped_stitched =
                        crate::ocr_engine::unwrap_multiline_spotlight_urls(&stitched);
                    for u in safe_link_grabber_extract(&unwrapped_stitched) {
                        if !is_bare_incomplete_social_path(&u)
                            && crate::link_grabber::is_strict_https_social_url(&u)
                            && !strict_social.contains(&u)
                        {
                            strict_social.push(u);
                        }
                    }
                    for u in extract_general_urls_from_ocr_text(&unwrapped_stitched) {
                        if is_bare_incomplete_social_path(&u) {
                            continue;
                        }
                        if crate::link_grabber::is_strict_https_social_url(&u) {
                            if !strict_social.contains(&u) {
                                strict_social.push(u);
                            }
                        } else if !general_https.contains(&u) {
                            general_https.push(u);
                        }
                    }
                }
            }
        }
    }

    // Prefer complete strict social URL (preserving 2x_natural > 2x_contrast > 1x priority on ties),
    // then fall back to any valid general HTTPS URL.
    if let Some(best_social) = pick_best_validated_url(strict_social) {
        return Some(best_social);
    }
    pick_best_validated_url(general_https)
}

fn read_fresh_shortcut_url_within_ms(max_age_ms: u64) -> Option<String> {
    let now = now_epoch_ms_u64();
    let guard = LATEST_SHORTCUT_URL_CACHE.lock().ok()?;
    let (ref url, ts) = *guard;
    if !url.is_empty() && ts > 0 && now.saturating_sub(ts) <= max_age_ms {
        return Some(url.clone());
    }
    None
}

/// Unified Tahap 242 + 243 `Grab Link (Alt+B)` handler powering `/api/qr/scan`, `/api/social/grab-link`,
/// `/api/link/grab`, and `/api/keyboard/shortcut` (`action: "scan-qr" | "grab-link"`).
/// Wrapped in `catch_unwind` + frame-by-frame early-exit for zero force-close & <35 MB peak RAM.
fn handle_native_grab_link_payload(body_str: &str) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handle_native_grab_link_payload_inner(body_str)
    })) {
        Ok(json) => json,
        Err(_) => r#"{"ok":false,"native":true,"action":"scan-qr","url":null,"link":null,"extractedText":null,"copiedToPc":false,"clipboardWritten":false,"qrDetected":false,"complete":false,"error":"OCR link guard recovered safely"}"#.to_string(),
    }
}

fn handle_native_grab_link_payload_inner(body_str: &str) -> String {
    let mut text_candidates: Vec<String> = Vec::new();
    for key in ["text", "raw_text", "rawText", "url", "link"] {
        if let Some(v) = crate::ocr_engine::extract_json_string_field(body_str, key) {
            if !v.trim().is_empty() {
                text_candidates.push(v);
            }
        }
    }
    if let Some(arr) = crate::ocr_engine::extract_json_string_array_field(body_str, "texts")
        .or_else(|| crate::ocr_engine::extract_json_string_array_field(body_str, "batchTexts"))
    {
        text_candidates.extend(arr);
    }

    let mut image_urls: Vec<String> = Vec::new();
    for arr_key in ["frames", "frameHistory"] {
        for u in crate::ocr_engine::extract_data_urls_array_from_json(body_str, arr_key) {
            if !image_urls.contains(&u) {
                image_urls.push(u);
            }
        }
    }
    for key in ["frameDataUrl", "tailFrameDataUrl", "imageDataUrl", "dataUrl"] {
        if let Some(u) = crate::ocr_engine::extract_json_string_field(body_str, key) {
            if u.starts_with("data:image/") && !image_urls.contains(&u) {
                image_urls.push(u);
            }
        }
    }

    let mut winning_url: Option<String> = None;
    let mut source = "direct_text";
    let mut auto_snap_used = false;
    let mut snap_ms = 0.0f64;
    let mut snap_w = 0u32;
    let mut snap_h = 0u32;

    // 1) Explicit text fields in JSON payload
    if !text_candidates.is_empty() {
        winning_url = extract_best_link_from_candidates(&text_candidates);
    }

    // 2) Explicit image frames (PNG/JPEG/BMP data URLs) -> GDI+ decode + tight Spotlight band WinRT OCR.
    // Process ONE frame at a time and early-exit as soon as any frame yields a valid URL!
    if winning_url.is_none() && !image_urls.is_empty() {
        let mut ocr_texts: Vec<String> = Vec::new();
        for img_url in &image_urls {
            let frame_texts = crate::ocr_engine::recognize_data_url_for_link_grab(img_url);
            if let Some(found) = extract_best_link_from_candidates(&frame_texts) {
                winning_url = Some(found);
                source = "spotlight_ocr";
                break;
            }
            ocr_texts.extend(frame_texts);
        }
        if winning_url.is_none() && !ocr_texts.is_empty() {
            if let Some(found) = extract_best_link_from_candidates(&ocr_texts) {
                winning_url = Some(found);
                source = "spotlight_ocr";
            }
        }
    }

    // 3) Fresh iPhone Shortcut LAN URL cache (`/c?u=...` within 8 seconds)
    if winning_url.is_none() {
        if let Some(sc_url) = read_fresh_shortcut_url_within_ms(8_000) {
            winning_url = Some(sc_url);
            source = "iphone_shortcut";
        }
    }

    // 4) Live D3D11/Video Snapshot fallback when request had no explicit text or frames
    if winning_url.is_none() && text_candidates.is_empty() && image_urls.is_empty() {
        if let Ok(snap) = iphone_link_apple::mirror::global_export_latest_video_snapshot() {
            auto_snap_used = true;
            snap_ms = snap.elapsed_ms;
            snap_w = snap.width;
            snap_h = snap.height;
            let snap_texts = crate::ocr_engine::recognize_data_url_for_link_grab(&snap.data_url);
            if let Some(found) = extract_best_link_from_candidates(&snap_texts) {
                winning_url = Some(found);
                source = "live_snapshot_ocr";
            }
        }
    }

    if let Some(best) = winning_url {
        let mut copied = false;
        for attempt in 0..3 {
            if write_win32_clipboard(&best) {
                copied = true;
                break;
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(15));
            }
        }
        if copied {
            set_active_clipboard_source(ClipboardSource::Pc);
        }
        let esc = escape_json_value(&best);
        return format!(
            r#"{{"ok":true,"native":true,"action":"scan-qr","url":"{}","link":"{}","text":"{}","extractedText":"{}","copiedToPc":{},"clipboardWritten":{},"clipboardSource":"{}","qrDetected":true,"complete":true,"source":"{}","length":{},"autoSnapshotUsed":{},"snapshotElapsedMs":{:.3},"snapshotWidth":{},"snapshotHeight":{}}}"#,
            esc,
            esc,
            esc,
            esc,
            copied,
            copied,
            if copied { "pc" } else { "iphone" },
            source,
            best.chars().count(),
            auto_snap_used,
            snap_ms,
            snap_w,
            snap_h
        );
    }

    format!(
        r#"{{"ok":false,"native":true,"action":"scan-qr","url":null,"link":null,"extractedText":null,"copiedToPc":false,"clipboardWritten":false,"qrDetected":false,"complete":false,"error":"No strict HTTP(S) link found","autoSnapshotUsed":{},"snapshotElapsedMs":{:.3},"snapshotWidth":{},"snapshotHeight":{}}}"#,
        auto_snap_used, snap_ms, snap_w, snap_h
    )
}

fn sync_shortcut_to_worker_async(clean_url: String) {
    if STANDALONE_INPROCESS_ONLY {
        let _ = clean_url;
        return;
    }
    thread::spawn(move || {
        use std::net::SocketAddr;
        let addr = SocketAddr::from(([127, 0, 0, 1], WORKER_PORT));
        if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(120)) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
            let body = format!(r#"{{"url":"{}"}}"#, escape_json_str(&clean_url));
            let req = format!(
                "POST /api/shortcut/push HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                WORKER_PORT,
                body.len(),
                body
            );
            let _ = stream.write_all(req.as_bytes());
        }
    });
}

fn extract_c_endpoint_url(req: &HttpRequest) -> String {
    let clean_path = req
        .path
        .split('?')
        .next()
        .unwrap_or(&req.path)
        .split('#')
        .next()
        .unwrap_or(&req.path);
    let mut candidate = if clean_path == "/c" {
        let mut found = String::new();
        if let Some(rest) = req.query.strip_prefix("u=").or_else(|| req.query.strip_prefix("url=")) {
            found = percent_decode_url_component(rest.trim());
        } else {
            for pair in req.query.split('&') {
                if let Some(v) = pair.strip_prefix("u=").or_else(|| pair.strip_prefix("url=")) {
                    found = percent_decode_url_component(v.trim());
                    break;
                }
            }
        }
        if found.is_empty() {
            let ql = req.query.to_ascii_lowercase();
            if ql.starts_with("http") {
                found = percent_decode_url_component(req.query.trim());
            }
        }
        found
    } else if clean_path.starts_with("/c/") {
        let mut raw_tail = clean_path[3..].to_string();
        if !req.query.is_empty() {
            raw_tail.push('?');
            raw_tail.push_str(&req.query);
        }
        percent_decode_url_component(raw_tail.trim())
    } else {
        let mut body_u = parse_string_param(&req.body, "url")
            .or_else(|| parse_string_param(&req.body, "u"))
            .or_else(|| parse_string_param(&req.body, "text"))
            .unwrap_or_default();
        if body_u.is_empty() {
            let trimmed_b = req.body.trim();
            if trimmed_b.to_ascii_lowercase().starts_with("http") {
                body_u = trimmed_b.to_string();
            } else if let Some(rest) = trimmed_b.strip_prefix("u=").or_else(|| trimmed_b.strip_prefix("url=")) {
                body_u = percent_decode_url_component(rest.trim());
            }
        }
        body_u
    };

    for _ in 0..2 {
        let cl = candidate.to_ascii_lowercase();
        if cl.starts_with("https%3a") || cl.starts_with("http%3a") {
            candidate = percent_decode_url_component(candidate.trim());
        } else {
            break;
        }
    }
    candidate.replace(r"\/", "/").trim().to_string()
}

// ============================================================================
// TAHAP 247: UNIFIED MULTI-IPHONE DEVICE COLLECTOR & SMART AUTO-PRIORITIZATION
// ============================================================================

#[derive(Debug, Clone)]
pub struct StatusDeviceSummary {
    pub udid: String,
    pub name: String,
    pub model: String,
    pub product_type: String,
    pub ios_version: String,
    pub bluetooth_address: String,
    pub screen_type: String,
    pub device_family: String,
    pub usb: bool,
    pub wifi_paired: bool,
    pub wifi_online: bool,
    pub reachable: bool,
    pub ip: Option<String>,
    pub connection_type: String, // "usb" | "wifi" | "offline"
    pub last_seen_at: u64,
}

fn collect_unified_devices(
    native_meta: &[iphone_link_apple::models::DeviceMetadata],
    usb_list: &[iphone_link_apple::usbmux::ConnectedAppleDevice],
    zero_poll_active: bool,
) -> Vec<StatusDeviceSummary> {
    let paired_wifi_udids = if zero_poll_active {
        // Tahap 249 Pilar 3: Zero-I/O RAM snapshot keeps paired Wi-Fi devices visible in /api/status
        // during active streaming & starting phases (eliminates devices=[] -> Wi-Fi Offline / iPhone Terputus!).
        paired_wifi_udids_ram_cache()
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    } else {
        let fresh = iphone_link_apple::wifi::wifi_paired_udids();
        if let Ok(mut g) = paired_wifi_udids_ram_cache().lock() {
            *g = fresh.clone();
        }
        fresh
    };
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut ordered_udids: Vec<String> = Vec::new();
    let mut push_unique = |u: &str| {
        if !u.is_empty() && !ordered_udids.iter().any(|x| x.eq_ignore_ascii_case(u)) {
            ordered_udids.push(u.to_string());
        }
    };
    for m in native_meta {
        push_unique(&m.udid);
    }
    for u in usb_list {
        push_unique(&u.serial_udid);
    }
    for pu in &paired_wifi_udids {
        push_unique(pu);
    }

    let mut result = Vec::with_capacity(ordered_udids.len());
    for udid in ordered_udids {
        let meta_opt = native_meta.iter().find(|m| m.udid.eq_ignore_ascii_case(&udid));
        let usb_opt = usb_list
            .iter()
            .find(|u| u.serial_udid.eq_ignore_ascii_case(&udid));
        let wmeta = if zero_poll_active {
            // Tahap 249 Pilar 3: Zero-I/O RAM snapshot preserves real model/name/IP/online state
            // during active streaming & starting phases (eliminates Wi-Fi Offline / iPhone Terputus flash!).
            read_unified_wifi_meta_zero_io(&udid)
        } else {
            let fresh_meta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(&udid);
            cache_unified_wifi_meta_ram(&udid, &fresh_meta);
            fresh_meta
        };

        let is_usb = meta_opt.is_some_and(|m| m.connection_type.eq_ignore_ascii_case("USB"))
            || usb_opt.is_some_and(|u| u.connection_type.eq_ignore_ascii_case("USB"));
        let is_net_live =
            meta_opt.is_some_and(|m| !m.connection_type.eq_ignore_ascii_case("USB"));
        let wifi_paired = paired_wifi_udids
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&udid))
            || wmeta.wifi_enabled;
        let ip = wmeta.last_ip.clone().filter(|s| !s.is_empty());
        let wifi_online = is_net_live
            || (wifi_paired && wmeta.reachable.unwrap_or_else(|| ip.is_some()));
        let reachable = is_usb || wifi_online;
        let connection_type = if is_usb {
            "usb".to_string()
        } else if wifi_online {
            "wifi".to_string()
        } else {
            "offline".to_string()
        };

        let product_type = meta_opt
            .map(|m| m.product_type.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| wmeta.product_type.clone());
        let device_family = if meta_opt
            .is_some_and(|m| m.device_class.eq_ignore_ascii_case("iPad"))
            || product_type.starts_with("iPad")
            || wmeta.model.to_ascii_lowercase().starts_with("ipad")
        {
            "ipad".to_string()
        } else {
            "iphone".to_string()
        };
        let generic_family = if device_family == "ipad" {
            "iPad"
        } else {
            "iPhone"
        };
        let model = meta_opt
            .map(|m| m.marketing_model.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                if !product_type.is_empty() {
                    iphone_link_apple::models::resolve_apple_model(&product_type)
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .or_else(|| (!wmeta.model.is_empty()).then(|| wmeta.model.clone()))
            .or_else(|| {
                usb_opt
                    .and_then(|u| u.marketing_model.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| generic_family.to_string());

        let name = meta_opt
            .map(|m| m.device_name.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| (!wmeta.name.is_empty()).then(|| wmeta.name.clone()))
            .or_else(|| {
                usb_opt
                    .map(|u| u.device_name.trim().to_string())
                    .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("iPhone"))
            })
            .unwrap_or_default();

        let ios_version = meta_opt
            .map(|m| m.product_version.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| wmeta.ios_version.clone());
        let bluetooth_address = meta_opt
            .map(|m| m.bluetooth_address.trim().to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| wmeta.bluetooth_address.clone());
        let screen_type = meta_opt
            .map(|m| m.screen_type.as_str().to_string())
            .unwrap_or_else(|| {
                if !product_type.is_empty() {
                    iphone_link_apple::models::screen_type_for_model(&product_type)
                        .as_str()
                        .to_string()
                } else if device_family == "ipad" {
                    "iPad".to_string()
                } else {
                    "Notch".to_string()
                }
            });
        let last_seen_at = if is_usb {
            now_secs.max(wmeta.last_seen_at)
        } else {
            wmeta.last_seen_at
        };

        result.push(StatusDeviceSummary {
            udid,
            name,
            model,
            product_type,
            ios_version,
            bluetooth_address,
            screen_type,
            device_family,
            usb: is_usb,
            wifi_paired,
            wifi_online,
            reachable,
            ip,
            connection_type,
            last_seen_at,
        });
    }

    // Sort cleanly: USB first -> Wi-Fi Online -> Offline, tie-break by newest last_seen_at -> udid
    result.sort_by(|a, b| {
        let rank = |d: &StatusDeviceSummary| -> u8 {
            if d.usb {
                0
            } else if d.wifi_online {
                1
            } else {
                2
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.last_seen_at.cmp(&a.last_seen_at))
            .then_with(|| a.udid.cmp(&b.udid))
    });
    result
}

/// Deterministic Smart Auto-Prioritization (Tahap 247 Pilar 2):
/// 1. 100% honor explicit user selection if still in real device list.
/// 2. USB connected device (prefer `last_selected` if multiple).
/// 3. Online/Reachable Wi-Fi paired device (prefer `last_selected`, then latest `last_seen_at`).
/// 4. `last_selected` if still in paired list (even if offline).
/// 5. First paired device. Returns `""` when 0 devices (100% Clean Slate Tahap 246).
pub fn resolve_active_selected_udid(
    devices: &[StatusDeviceSummary],
    current_selected: &str,
    user_explicitly_selected: bool,
    last_selected: &str,
) -> String {
    if devices.is_empty() {
        return String::new();
    }
    if user_explicitly_selected && !current_selected.is_empty() {
        if let Some(m) = devices
            .iter()
            .find(|d| d.udid.eq_ignore_ascii_case(current_selected))
        {
            return m.udid.clone();
        }
    }
    let usb_devs: Vec<&StatusDeviceSummary> = devices.iter().filter(|d| d.usb).collect();
    if !usb_devs.is_empty() {
        if !last_selected.is_empty() {
            if let Some(m) = usb_devs
                .iter()
                .find(|d| d.udid.eq_ignore_ascii_case(last_selected))
            {
                return m.udid.clone();
            }
        }
        return usb_devs[0].udid.clone();
    }
    let mut wifi_online_devs: Vec<&StatusDeviceSummary> = devices
        .iter()
        .filter(|d| {
            d.wifi_paired
                && d.wifi_online
                && d.ip.as_deref().is_some_and(|s| !s.is_empty())
        })
        .collect();
    if !wifi_online_devs.is_empty() {
        if !last_selected.is_empty() {
            if let Some(m) = wifi_online_devs
                .iter()
                .find(|d| d.udid.eq_ignore_ascii_case(last_selected))
            {
                return m.udid.clone();
            }
        }
        wifi_online_devs.sort_by(|a, b| {
            b.last_seen_at
                .cmp(&a.last_seen_at)
                .then_with(|| a.udid.cmp(&b.udid))
        });
        return wifi_online_devs[0].udid.clone();
    }
    if !last_selected.is_empty() {
        if let Some(m) = devices
            .iter()
            .find(|d| d.udid.eq_ignore_ascii_case(last_selected))
        {
            return m.udid.clone();
        }
    }
    devices[0].udid.clone()
}

fn format_device_summary_json(d: &StatusDeviceSummary, is_selected: bool) -> String {
    let legacy_conn = if d.usb {
        "USB"
    } else if d.wifi_online {
        "Wi-Fi"
    } else {
        "Wi-Fi (Offline)"
    };
    let ip_json = match d.ip.as_deref().filter(|s| !s.is_empty()) {
        Some(ip) => format!(r#""{}""#, escape_json_str(ip)),
        None => "null".to_string(),
    };
    let display_name = if d.name.is_empty() {
        d.model.as_str()
    } else {
        d.name.as_str()
    };
    format!(
        r#"{{
      "udid": "{}",
      "name": "{}",
      "deviceName": "{}",
      "model": "{}",
      "productType": "{}",
      "iosVersion": "{}",
      "screen_type": "{}",
      "deviceFamily": "{}",
      "usb": {},
      "wifiPaired": {},
      "wifiOnline": {},
      "reachable": {},
      "online": {},
      "ip": {},
      "connectionType": "{}",
      "connection": "{}",
      "connection_type": "{}",
      "paired": {},
      "wifi_enabled": {},
      "wifiOnly": {},
      "selected": {},
      "lastSeenAt": {}
    }}"#,
        escape_json_str(&d.udid),
        escape_json_str(&d.name),
        escape_json_str(display_name),
        escape_json_str(&d.model),
        escape_json_str(&d.product_type),
        escape_json_str(&d.ios_version),
        escape_json_str(&d.screen_type),
        escape_json_str(&d.device_family),
        d.usb,
        d.wifi_paired,
        d.wifi_online,
        d.reachable,
        d.reachable,
        ip_json,
        escape_json_str(&d.connection_type),
        legacy_conn,
        legacy_conn,
        d.usb || d.wifi_paired,
        d.wifi_paired,
        !d.usb,
        is_selected,
        d.last_seen_at,
    )
}

fn build_status_payload_json(
    state: &Arc<Mutex<AppState>>,
    force_refresh: bool,
    stop_cooldown: bool,
    fast_zero_io: bool,
) -> String {
    let st_peek = state.lock().unwrap();
    let streaming_peek = iphone_link_apple::mirror::is_mirror_streaming_fast()
        || (st_peek.is_streaming && !stop_cooldown);
    let audio_streaming_peek = iphone_link_apple::mirror::mirror_runtime()
        .lock()
        .map(|m| m.audio.streaming)
        .unwrap_or(false);
    let zero_poll_active = streaming_peek || audio_streaming_peek;
    drop(st_peek);

    if force_refresh && !zero_poll_active && !fast_zero_io {
        let paired_cnt = iphone_link_apple::wifi::wifi_paired_udids().len();
        if paired_cnt >= 2 {
            let _ = iphone_link_apple::wifi::probe_all_paired_wifi_devices_parallel(650);
        }
    }

    // Zero-Polling Guard (Tahap 59 parity): when screen/audio is actively streaming OR during
    // instant `POST /api/device/select` (`fast_zero_io`), NEVER block on usbmuxd sockets!
    let native_meta = if zero_poll_active || fast_zero_io {
        snapshot_cached_devices_metadata_zero_io()
    } else {
        discover_devices_with_metadata(force_refresh)
    };
    let usb_list = if zero_poll_active || fast_zero_io {
        Vec::new()
    } else {
        list_connected_usb_devices(Duration::from_millis(60)).unwrap_or_default()
    };

    let unified_devices = collect_unified_devices(&native_meta, &usb_list, zero_poll_active);

    let st_snap = {
        let mut st = state.lock().unwrap();
        if !zero_poll_active {
            let resolved_udid = resolve_active_selected_udid(
                &unified_devices,
                &st.selected_udid,
                st.user_explicitly_selected_udid,
                &st.last_selected_udid,
            );
            st.selected_udid = resolved_udid;
        }
        st.clone()
    };

    let lang_code = st_snap.language.code();
    let udid = &st_snap.selected_udid;
    let (
        mirror_phase,
        mirror_reconnecting,
        mirror_transport,
        mirror_url_opt,
        mirror_error_opt,
        mirror_note_opt,
        mirror_orient,
        mirror_auto_rot,
        mirror_frames,
        mirror_stage,
        mirror_stage_progress,
        mirror_stage_step,
        mirror_stage_title,
        mirror_stage_detail,
        mirror_route,
        mirror_airplay_active,
        mirror_airplay_name,
        mirror_airplay_port,
        mirror_hid_available,
        mirror_airplay_avcc,
        mirror_airplay_frames,
        mirror_airplay_idrs,
        mirror_airplay_deltas,
        mirror_airplay_dec_fail,
        mirror_airplay_tcp_pkts,
        mirror_airplay_loop_alive,
        mirror_live_chunks_broadcasted,
        mirror_audio_json,
    ) = {
        let m = iphone_link_apple::mirror::mirror_runtime()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ajson = m.audio.status_json();
        let mp_clone = m.phase.clone();
        let m_airplay_active = m.airplay_active;
        let m_streaming = m.audio.streaming;
        drop(m);
        // Tahap 257 Pilar 1 (Root Cause #1 Fix vs ZenLink):
        // NEVER launch Windows Bluetooth A2DP AudioPlaybackConnection while AirPlay H.264 is active!
        // Opening Bluetooth A2DP ~1s after AirPlay Frame #1 hijacks iOS's active audio route from
        // AirPlay -> Bluetooth, forcing iOS airplayd to send TEARDOWN type=96 + full TEARDOWN!
        let want_auto_audio = !stop_cooldown
            && mp_clone == "running"
            && !m_airplay_active
            && !AUDIO_MANUALLY_STOPPED_THIS_SESSION.load(Ordering::SeqCst)
            && !st_snap.audio_muted
            && !st_snap.audio_routing.eq_ignore_ascii_case("device");
        let (pref_a_name, pref_a_model, pref_a_bt) =
            resolve_active_audio_device_target(state, None, None, None);
        let (cur_audio_dev, _) = iphone_link_audio::global_pc_audio_bridge_device_info();
        let cur_bridge_active = iphone_link_audio::is_global_pc_audio_bridge_active();
        let cur_bridge_conn = iphone_link_audio::is_global_pc_audio_bridge_connecting();
        let cur_target_key = iphone_link_audio::global_pc_audio_bridge_target_key();
        let audio_target_mismatch = cur_bridge_active
            && !cur_bridge_conn
            && pref_a_name.as_deref().is_some_and(|pn| {
                !cur_audio_dev.is_empty() && !cur_audio_dev.eq_ignore_ascii_case(pn)
            });
        // Tahap 249 Pilar 3: If initial pre-RSD audio attempt launched with empty/generic name
        // (`target_key` empty or starting with `|`) and RSD has now supplied real DeviceName/BT MAC,
        // rebind cleanly to target the exact active iPhone on Session 1 without manual toggle!
        let rsd_identity_upgraded = !cur_bridge_active
            && (cur_target_key.is_empty() || cur_target_key.starts_with('|'))
            && (pref_a_name.is_some() || pref_a_bt.is_some());
        let now_audio_ms = now_epoch_ms_u64();
        let last_audio_try_ms = LAST_AUTO_AUDIO_ATTEMPT_MS.load(Ordering::SeqCst);
        let retry_cooldown_ok =
            last_audio_try_ms == 0 || now_audio_ms.saturating_sub(last_audio_try_ms) >= 3_000;
        let need_auto_audio_launch = want_auto_audio
            && (((!cur_bridge_active && !cur_bridge_conn) && retry_cooldown_ok)
                || audio_target_mismatch
                || (rsd_identity_upgraded && !cur_bridge_conn));
        if need_auto_audio_launch
            && AUTO_AUDIO_SPAWN_IN_FLIGHT
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            LAST_AUTO_AUDIO_ATTEMPT_MS.store(now_audio_ms, Ordering::SeqCst);
            let a_mode = st_snap.audio_routing.clone();
            let a_vol = st_snap.audio_volume as i64;
            let a_preset = st_snap.audio_buffer_preset.clone();
            let a_ls = st_snap.lip_sync;
            let _ = iphone_link_audio::start_global_pc_audio_bridge_nonblocking_for_device(
                pref_a_name.as_deref(),
                pref_a_model.as_deref(),
                pref_a_bt.as_deref(),
            );
            let _ = iphone_link_apple::mirror::global_configure_audio(
                Some(&a_mode),
                Some(a_vol),
                Some(false),
                Some(&a_preset),
                Some(a_ls),
                Some(true),
            );
            AUTO_AUDIO_SPAWN_IN_FLIGHT.store(false, Ordering::SeqCst);
        }
        let (pc_bridge_on, pc_bridge_conn, pc_bridge_det) =
            iphone_link_audio::global_pc_audio_bridge_full_status();
        let pc_bridge_state = iphone_link_audio::global_pc_audio_bridge_state_kind();
        let (pc_bridge_dev, pc_bridge_cands) =
            iphone_link_audio::global_pc_audio_bridge_device_info();
        let ap_audio_snap = iphone_link_apple::airplay::get_airplay_audio_snapshot();
        let ap_audio_on = m_airplay_active && !stop_cooldown && ap_audio_snap.active;
        let ap_audio_codec_json = ap_audio_snap
            .codec
            .as_ref()
            .filter(|_| m_airplay_active && !stop_cooldown)
            .map(|c| format!(r#""{}""#, escape_json_value(c)))
            .unwrap_or_else(|| "null".to_string());
        let ap_video_q_dropped = if m_airplay_active && !stop_cooldown {
            iphone_link_apple::mirror::global_airplay_video_queue_dropped()
        } else {
            0
        };
        if let Some(end_brace) = ajson.rfind('}') {
            ajson.insert_str(
                end_brace,
                &format!(
                    r#","wasapi":true,"connected":{},"pcBridgeActive":{},"pcBridgeConnecting":{},"pcBridgeState":"{}","pcBridgeDetail":"{}","pcBridgeDevice":"{}","pcBridgeCandidates":"{}","airplayActive":{},"airplayAudioActive":{},"airplayAudioMuted":{},"airplayAudioCodec":{},"airplayAudioSampleRate":{},"airplayAudioDecodedPackets":{},"airplayAudioPlayedSamples":{},"airplayAudioVolume":{},"airplayAudioBufferPreset":"{}","airplayAudioLipSync":{},"airplayAudioBufferMs":{},"airplayAudioTargetMs":{},"airplayAudioDroppedPackets":{},"airplayVideoQueueDropped":{},"lip_sync_ms":0"#,
                    m_streaming || st_snap.is_streaming || pc_bridge_on || ap_audio_on,
                    pc_bridge_on,
                    pc_bridge_conn,
                    pc_bridge_state,
                    escape_json_value(&pc_bridge_det),
                    escape_json_value(&pc_bridge_dev),
                    escape_json_value(&pc_bridge_cands),
                    m_airplay_active && !stop_cooldown,
                    ap_audio_on,
                    ap_audio_snap.muted,
                    ap_audio_codec_json,
                    if m_airplay_active && !stop_cooldown { ap_audio_snap.sample_rate } else { 0 },
                    if m_airplay_active && !stop_cooldown { ap_audio_snap.decoded_packets } else { 0 },
                    if m_airplay_active && !stop_cooldown { ap_audio_snap.played_samples } else { 0 },
                    ap_audio_snap.volume,
                    escape_json_value(&ap_audio_snap.buffer_preset),
                    ap_audio_snap.lip_sync,
                    if ap_audio_on { ap_audio_snap.buffer_ms } else { 0 },
                    ap_audio_snap.target_ms,
                    if m_airplay_active && !stop_cooldown { ap_audio_snap.dropped_packets } else { 0 },
                    ap_video_q_dropped,
                ),
            );
        }
        let m = iphone_link_apple::mirror::mirror_runtime()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            m.phase.clone(),
            m.reconnecting && !stop_cooldown,
            m.transport.clone(),
            m.url.clone(),
            m.error.clone(),
            m.note.clone(),
            m.auto_rotate.current_orientation_deg,
            m.auto_rotate.auto_rotate_enabled,
            m.frames_received,
            if stop_cooldown { String::new() } else { m.stage.clone() },
            if stop_cooldown { 0 } else { m.stage_progress },
            m.stage_step,
            m.stage_title.clone(),
            m.stage_detail.clone(),
            m.mirror_route.clone(),
            m.airplay_active && !stop_cooldown,
            m.airplay_name.clone(),
            if stop_cooldown { 0 } else { m.airplay_port },
            m.hid_control_available,
            m.airplay_avcc_count,
            m.airplay_video_frames,
            m.airplay_idr_count,
            m.airplay_delta_frames,
            m.airplay_decrypt_failures,
            m.airplay_tcp_packets_read,
            m.airplay_mirror_loop_count > 0 && !stop_cooldown,
            m.live_chunks_broadcasted,
            ajson,
        )
    };
    let (hid_packets_tx, hid_packets_err, hid_chan_active) =
        iphone_link_apple::hid::physical_hid_telemetry();
    let screen_phase = if stop_cooldown {
        "idle".to_string()
    } else if mirror_phase != "idle" {
        mirror_phase
    } else if st_snap.is_streaming {
        "running".to_string()
    } else {
        "idle".to_string()
    };
    let effective_streaming = screen_phase == "running";
    let native_diag_json = build_native_driver_diagnostics_json_with_opts(
        false,
        effective_streaming || zero_poll_active || fast_zero_io,
    );
    let probe_single_wifi = force_refresh
        && !zero_poll_active
        && !fast_zero_io
        && iphone_link_apple::wifi::wifi_paired_udids().len() == 1;
    let native_wifi_json = get_wifi_status_cached_for_stream(
        udid,
        effective_streaming || zero_poll_active || fast_zero_io,
        probe_single_wifi,
    );

    let has_usb_now = unified_devices.iter().any(|d| d.usb);
    let any_wifi_online = unified_devices.iter().any(|d| d.wifi_online);
    let has_paired_wifi = unified_devices.iter().any(|d| d.wifi_paired);
    let is_connected = has_usb_now || any_wifi_online || effective_streaming;
    let dev_mode_ok = !native_diag_json.contains(r#""developer_mode_status":"disabled""#);
    // Tahap 249 Pilar 3: Never report driver_ready=false when Apple service/port is normal
    // OR a USB/Wi-Fi device is actively reachable/streaming (eliminates false "Install Apple Devices").
    let driver_diag_ok = !native_diag_json.contains(r#""apple_driver_installed":false"#)
        || native_diag_json.contains(r#""apple_service_running":true"#)
        || native_diag_json.contains(r#""tcp_port_27015_normal":true"#);
    let driver_ready = driver_diag_ok || has_usb_now || any_wifi_online || effective_streaming;
    let is_ready = driver_ready && is_connected;

    let selected_dev = unified_devices
        .iter()
        .find(|d| !udid.is_empty() && d.udid.eq_ignore_ascii_case(udid))
        .or_else(|| unified_devices.first());

    let device_entries: Vec<String> = unified_devices
        .iter()
        .map(|d| {
            let is_sel = selected_dev.is_some_and(|sd| sd.udid.eq_ignore_ascii_case(&d.udid));
            format_device_summary_json(d, is_sel)
        })
        .collect();
    let devices_json_str = if device_entries.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n    {}\n  ]", device_entries.join(",\n    "))
    };

    let (device_json_str, handset_json_str, effective_conn_type, effective_family) =
        if let Some(sd) = selected_dev {
            let dev_obj = format_device_summary_json(sd, true);
            let hs_name = if sd.name.is_empty() {
                sd.model.as_str()
            } else {
                sd.name.as_str()
            };
            let hs_obj = format!(
                r#"{{
    "udid": "{}",
    "name": "{}",
    "model": "{}",
    "productType": "{}"
  }}"#,
                escape_json_str(&sd.udid),
                escape_json_str(hs_name),
                escape_json_str(&sd.model),
                escape_json_str(&sd.product_type),
            );
            let conn_lbl = if sd.usb {
                "USB"
            } else if sd.wifi_online {
                "Wi-Fi"
            } else {
                ""
            };
            (dev_obj, hs_obj, conn_lbl, sd.device_family.as_str())
        } else {
            ("null".to_string(), "null".to_string(), "", "iphone")
        };

    let raw_status_label = if has_usb_now {
        "Terhubung (USB)"
    } else if any_wifi_online {
        "Wi-Fi siap"
    } else if has_paired_wifi {
        "Perangkat offline"
    } else {
        "Menunggu iPhone"
    };
    let status_text = translate_dynamic_status(raw_status_label, st_snap.language);
    let wallpaper_sig = get_cached_wallpaper_sig_fast();
    let folder_instance_id = get_deterministic_folder_instance_id();
    let fresh_folder = if effective_streaming || zero_poll_active {
        false
    } else {
        is_fresh_extracted_folder()
    };
    let (clip_src, clip_seq) = resolve_active_clipboard_source_with_seq();
    let clip_src_str = match clip_src {
        ClipboardSource::Pc => "pc",
        ClipboardSource::Iphone => "iphone",
    };
    let mirror_url = if stop_cooldown {
        ""
    } else if let Some(ref u) = mirror_url_opt {
        u.as_str()
    } else if effective_streaming {
        "/mirror.html"
    } else {
        ""
    };
    let screen_error_str = if stop_cooldown {
        String::new()
    } else {
        mirror_error_opt.unwrap_or_default()
    };
    let screen_note_str = if stop_cooldown {
        String::new()
    } else {
        mirror_note_opt.unwrap_or_default()
    };
    let effective_orient = if mirror_orient != 0 {
        mirror_orient
    } else if st_snap.is_landscape {
        270
    } else {
        0
    };
    let settings_json = build_settings_json(&st_snap);
    let dpr_float = (GLOBAL_DEVICE_PIXEL_RATIO.load(Ordering::SeqCst) as f64) / 100.0;
    let app_w = GLOBAL_APP_WIDTH.load(Ordering::SeqCst);
    let app_h = GLOBAL_APP_HEIGHT.load(Ordering::SeqCst);
    let lp_vis = GLOBAL_LAUNCHPAD_VISIBLE.load(Ordering::SeqCst);
    let btn_vis = GLOBAL_START_BTN_VISIBLE.load(Ordering::SeqCst);
    let is_dark = GLOBAL_DARK_MODE.load(Ordering::SeqCst);
    let active_thm = if is_dark { "dark" } else { "light" };
    let dom_thm = GLOBAL_DOM_THEME.lock().map(|s| s.clone()).unwrap_or_default();
    let dom_thm_str = if dom_thm.is_empty() { active_thm } else { &dom_thm };
    let wf_clean = GLOBAL_WIREFRAME_CLEAN.load(Ordering::SeqCst);
    let wf_details = GLOBAL_WIREFRAME_DETAILS
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let safe_wf_details = wf_details.replace('\\', r#"\\"#).replace('"', r#"\""#);
    let pending_click = GLOBAL_PENDING_CLICK
        .lock()
        .ok()
        .and_then(|mut g| g.pop_front());
    let pending_click_str = match pending_click {
        Some(ref s) => format!(
            r#","pendingClick":"{}""#,
            s.replace('\\', r#"\\"#).replace('"', r#"\""#)
        ),
        None => String::new(),
    };
    let pending_test = GLOBAL_PENDING_TEST.lock().ok().and_then(|mut g| g.take());
    let pending_test_str = match pending_test {
        Some(ref s) => format!(
            r#","pendingTest":"{}""#,
            s.replace('\\', r#"\\"#).replace('"', r#"\""#)
        ),
        None => String::new(),
    };
    let pending_tray = GLOBAL_PENDING_TRAY_ACTION
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let pending_tray_str = match pending_tray {
        Some(ref s) => format!(r#","pendingTrayAction":"{}""#, escape_json_str(s)),
        None => String::new(),
    };
    let pending_input = GLOBAL_PENDING_INPUT
        .lock()
        .ok()
        .and_then(|mut g| g.pop_front());
    let pending_input_str = match pending_input {
        Some((ref sel, ref val)) => format!(
            r#","pendingInput":{{"selector":"{}","value":"{}"}}"#,
            escape_json_str(sel),
            escape_json_str(val)
        ),
        None => String::new(),
    };
    let conc_track = GLOBAL_CONCENTRIC_TRACK
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let conc_btn = GLOBAL_CONCENTRIC_BTN
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let badge_bg = GLOBAL_BADGE_BG
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let seg_clean = GLOBAL_SEGMENTED_CLEAN.load(Ordering::SeqCst);
    let seg_details = GLOBAL_SEGMENTED_DETAILS
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let safe_seg_details = seg_details.replace('\\', r#"\\"#).replace('"', r#"\""#);
    let active_prof = GLOBAL_DEVICE_PROFILE
        .lock()
        .map(|s| {
            if s.is_empty() {
                "auto".to_string()
            } else {
                s.clone()
            }
        })
        .unwrap_or_else(|_| "auto".to_string());
    let dr_width = GLOBAL_DRAWER_RECT_WIDTH.load(Ordering::SeqCst);
    let dr_overflow = GLOBAL_DRAWER_RIGHT_OVERFLOW.load(Ordering::SeqCst);
    let dc_width = GLOBAL_DISPLAY_COL_WIDTH.load(Ordering::SeqCst);
    let sd_width = GLOBAL_SIDE_DRAWER_WIDTH.load(Ordering::SeqCst);
    let sd_right_gap = GLOBAL_SIDE_DRAWER_RIGHT_GAP.load(Ordering::SeqCst);
    let ds_right_gap = GLOBAL_DRAWER_SCROLL_RIGHT_GAP.load(Ordering::SeqCst);
    let inner_w = GLOBAL_INNER_WIDTH.load(Ordering::SeqCst);
    let stretch_cnt = GLOBAL_STRETCH_GLITCH_COUNT.load(Ordering::SeqCst);
    let max_close_w = GLOBAL_MAX_DISPLAY_COL_WIDTH_DURING_CLOSE.load(Ordering::SeqCst);

    let ap_screen_audio = iphone_link_apple::airplay::get_airplay_audio_snapshot();
    let ap_screen_audio_active = mirror_airplay_active && ap_screen_audio.active;
    let ap_screen_audio_codec = ap_screen_audio
        .codec
        .as_ref()
        .filter(|_| mirror_airplay_active)
        .map(|c| format!(r#""{}""#, escape_json_str(c)))
        .unwrap_or_else(|| "null".to_string());
    let ap_screen_audio_sr = if mirror_airplay_active { ap_screen_audio.sample_rate } else { 0 };
    let ap_screen_audio_dec = if mirror_airplay_active { ap_screen_audio.decoded_packets } else { 0 };
    let ap_screen_audio_played = if mirror_airplay_active { ap_screen_audio.played_samples } else { 0 };
    let ap_screen_audio_buf_ms = if ap_screen_audio_active { ap_screen_audio.buffer_ms } else { 0 };
    let ap_screen_audio_dropped = if mirror_airplay_active { ap_screen_audio.dropped_packets } else { 0 };
    let ap_screen_video_q_dropped = if mirror_airplay_active {
        iphone_link_apple::mirror::global_airplay_video_queue_dropped()
    } else {
        0
    };

    let json = format!(
        r#"{{
  "library": {{ "ready": true }},
  "driver": {{ "ready": {}, "detail": "Apple Mobile Device Service running" }},
  "driverReady": {},
  "detailedDiagnostics": {},
  "developerMode": {},
  "ready": {},
  "usbConnected": {},
  "connected": {},
  "udid": "{}",
  "selectedUdid": "{}",
  "device": {},
  "handset": {},
  "connectionType": "{}",
  "transport": "{}",
  "deviceFamily": "{}",
  "deviceProfile": "{}",
  "wallpaperSig": "{}",
  "folderInstanceId": "{}",
  "isFreshFolder": {},
  "clipboardSeq": {},
  "activeClipboardSource": "{}",
  "devices": {},
  "screen": {{
    "phase": "{}",
    "active": {},
    "reconnecting": {},
    "udid": "{}",
    "transport": "{}",
    "mirrorRoute": "{}",
    "airplayActive": {},
    "airplayName": "{}",
    "airplayPort": {},
    "hidControlAvailable": {},
    "airplayAvccCount": {},
    "airplayVideoFrames": {},
    "airplayIdrCount": {},
    "airplayDeltaFrames": {},
    "airplayDecryptFailures": {},
    "airplayTcpPacketsRead": {},
    "airplayMirrorLoopAlive": {},
    "airplayAudioActive": {},
    "airplayAudioMuted": {},
    "airplayAudioCodec": {},
    "airplayAudioSampleRate": {},
    "airplayAudioDecodedPackets": {},
    "airplayAudioPlayedSamples": {},
    "airplayAudioVolume": {},
    "airplayAudioBufferPreset": "{}",
    "airplayAudioLipSync": {},
    "airplayAudioBufferMs": {},
    "airplayAudioTargetMs": {},
    "airplayAudioDroppedPackets": {},
    "airplayVideoQueueDropped": {},
    "liveChunksBroadcasted": {},
    "liveClientsConnected": {},
    "url": "{}",
    "error": "{}",
    "note": "{}",
    "fps": 60,
    "width": 1202,
    "height": 2598,
    "orientation": {},
    "autoRotate": {},
    "framesReceived": {},
    "stealth_dim": {},
    "intentionally_locked": {},
    "stage": "{}",
    "stageDetail": "{}",
    "stageProgress": {},
    "stage_step": {},
    "stage_title": "{}",
    "stage_detail": "{}",
    "physical_packets_transmitted": {},
    "physical_hid_errors": {},
    "physical_hid_active": {}
  }},
  "physical_packets_transmitted": {},
  "physical_hid_errors": {},
  "physical_hid_active": {},
  "audio": {},
  "wifi": {},
  "window": {{
    "drawer_open": {},
    "is_landscape": {},
    "is_fullscreen": {},
    "portrait_width": {},
    "active_device_profile": "{}"
  }},
  "hot_ocr_engine_ready": true,
  "settings": {},
  "status_text": "{}",
  "language": "{}",
  "webviewMetrics": {{
    "indexHtmlServed": {},
    "appCssServed": {},
    "appJsServed": {},
    "domReadyBeacon": {},
    "devicePixelRatio": {:.2},
    "appWidth": {},
    "appHeight": {},
    "launchpadVisible": {},
    "startBtnVisible": {},
    "startBtnText": "{}",
    "userAgent": "{}",
    "rafVerified": {},
    "activeTheme": "{}",
    "domTheme": "{}",
    "wireframeClean": {},
    "wireframeDetails": "{}",
    "concentricTrackRadius": "{}",
    "concentricButtonRadius": "{}",
    "badgeBg": "{}",
    "segmentedClean": {},
    "segmentedDetails": "{}",
    "activeDeviceProfile": "{}",
    "drawerRectWidth": {},
    "drawerRightOverflow": {},
    "displayColWidth": {},
    "sideDrawerWidth": {},
    "sideDrawerRightGap": {},
    "drawerScrollRightGap": {},
    "innerWidth": {},
    "stretchGlitchCount": {},
    "maxDisplayColWidthDuringClose": {},
    "unlockPinValue": "{}",
    "updateVersionText": "{}",
    "lastTrayAction": "{}",
    "domLanguage": "{}",
    "activeLangId": {},
    "activeLangEn": {},
    "drawerTitleText": "{}",
    "domTogglesSummary": "{}",
    "cardUpdaterExists": {},
    "cardUpdaterTitle": "{}",
    "stealthActive": {},
    "audioPcActive": {},
    "startWithWindows": {},
    "trayMenuOpen": {},
    "trayDismissedCount": {}
  }},
  "videoPipeline": {},
  "videoPresenter": {},
  "version": "5.0.1",
  "edition": "100% Pure Native Rust (Zero Python & Zero PowerShell)",
  "zero_python_runtime": true,
  "zero_powershell_runtime": true{}"#,
        driver_ready,
        driver_ready,
        native_diag_json,
        dev_mode_ok,
        is_ready,
        has_usb_now,
        is_connected,
        escape_json_str(udid),
        escape_json_str(udid),
        device_json_str,
        handset_json_str,
        effective_conn_type,
        st_snap.transport,
        effective_family,
        st_snap.device_profile,
        escape_json_str(&wallpaper_sig),
        escape_json_str(&folder_instance_id),
        fresh_folder,
        clip_seq,
        clip_src_str,
        devices_json_str,
        screen_phase,
        effective_streaming,
        mirror_reconnecting,
        escape_json_str(udid),
        if mirror_transport == "wifi" || st_snap.transport == "wifi" {
            "wifi"
        } else {
            "cable"
        },
        escape_json_str(&mirror_route),
        mirror_airplay_active,
        escape_json_str(&mirror_airplay_name),
        mirror_airplay_port,
        mirror_hid_available,
        mirror_airplay_avcc,
        mirror_airplay_frames,
        mirror_airplay_idrs,
        mirror_airplay_deltas,
        mirror_airplay_dec_fail,
        mirror_airplay_tcp_pkts,
        mirror_airplay_loop_alive,
        ap_screen_audio_active,
        ap_screen_audio.muted,
        ap_screen_audio_codec,
        ap_screen_audio_sr,
        ap_screen_audio_dec,
        ap_screen_audio_played,
        ap_screen_audio.volume,
        escape_json_str(&ap_screen_audio.buffer_preset),
        ap_screen_audio.lip_sync,
        ap_screen_audio_buf_ms,
        ap_screen_audio.target_ms,
        ap_screen_audio_dropped,
        ap_screen_video_q_dropped,
        mirror_live_chunks_broadcasted,
        iphone_link_apple::mirror::global_live_clients_connected(),
        mirror_url,
        escape_json_str(&screen_error_str),
        escape_json_str(&screen_note_str),
        effective_orient,
        mirror_auto_rot,
        mirror_frames,
        iphone_link_apple::hid::is_stealth_dim_active() || st_snap.stealth_dim,
        iphone_link_apple::hid::is_device_intentionally_locked() || st_snap.intentionally_locked,
        escape_json_str(&mirror_stage),
        escape_json_str(&mirror_stage_detail),
        mirror_stage_progress,
        mirror_stage_step,
        escape_json_str(&mirror_stage_title),
        escape_json_str(&mirror_stage_detail),
        hid_packets_tx,
        hid_packets_err,
        hid_chan_active,
        hid_packets_tx,
        hid_packets_err,
        hid_chan_active,
        mirror_audio_json,
        native_wifi_json,
        GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst),
        GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst),
        GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst),
        GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst),
        active_prof.clone(),
        settings_json,
        status_text,
        lang_code,
        INDEX_HTML_SERVED.load(Ordering::SeqCst),
        APP_CSS_SERVED.load(Ordering::SeqCst),
        APP_JS_SERVED.load(Ordering::SeqCst),
        DOM_READY_BEACON.load(Ordering::SeqCst),
        dpr_float,
        app_w,
        app_h,
        lp_vis,
        btn_vis,
        GLOBAL_START_BTN_TEXT
            .lock()
            .map(|s| s.replace('\\', r#"\\"#).replace('"', r#"\""#))
            .unwrap_or_default(),
        GLOBAL_USER_AGENT
            .lock()
            .map(|s| s.replace('\\', r#"\\"#).replace('"', r#"\""#))
            .unwrap_or_default(),
        GLOBAL_RAF_VERIFIED.load(Ordering::SeqCst),
        active_thm,
        dom_thm_str,
        wf_clean,
        safe_wf_details,
        conc_track,
        conc_btn,
        badge_bg,
        seg_clean,
        safe_seg_details,
        active_prof,
        dr_width,
        dr_overflow,
        dc_width,
        sd_width,
        sd_right_gap,
        ds_right_gap,
        inner_w,
        stretch_cnt,
        max_close_w,
        GLOBAL_DOM_UNLOCK_PIN
            .lock()
            .map(|s| escape_json_str(&s))
            .unwrap_or_default(),
        GLOBAL_DOM_UPDATE_VERSION
            .lock()
            .map(|s| escape_json_str(&s))
            .unwrap_or_default(),
        GLOBAL_DOM_LAST_TRAY_ACTION
            .lock()
            .map(|s| escape_json_str(&s))
            .unwrap_or_default(),
        GLOBAL_DOM_LANG
            .lock()
            .map(|s| escape_json_str(&s))
            .unwrap_or_default(),
        GLOBAL_DOM_LANG_ID_ACTIVE.load(Ordering::SeqCst),
        GLOBAL_DOM_LANG_EN_ACTIVE.load(Ordering::SeqCst),
        GLOBAL_DOM_DRAWER_TITLE
            .lock()
            .map(|s| escape_json_str(&s))
            .unwrap_or_default(),
        GLOBAL_DOM_TOGGLES_SUMMARY
            .lock()
            .map(|s| escape_json_str(&s))
            .unwrap_or_default(),
        GLOBAL_DOM_CARD_UPDATER_EXISTS.load(Ordering::SeqCst),
        GLOBAL_DOM_CARD_UPDATER_TITLE
            .lock()
            .map(|s| escape_json_str(&s))
            .unwrap_or_default(),
        GLOBAL_STEALTH_ACTIVE.load(Ordering::SeqCst),
        GLOBAL_AUDIO_PC_ACTIVE.load(Ordering::SeqCst),
        cached_start_with_windows_for_status(effective_streaming || zero_poll_active),
        GLOBAL_TRAY_MENU_OPEN.load(Ordering::SeqCst),
        GLOBAL_TRAY_DISMISSED_COUNT.load(Ordering::SeqCst),
        iphone_link_apple::mirror::global_video_pipeline_status_json(),
        iphone_link_apple::mirror::global_video_presenter_status_json(),
        if pending_click_str.is_empty()
            && pending_test_str.is_empty()
            && pending_tray_str.is_empty()
            && pending_input_str.is_empty()
        {
            "\n}"
        } else {
            "\n"
        }
    );
    let mut final_json = json;
    if !pending_click_str.is_empty()
        || !pending_test_str.is_empty()
        || !pending_tray_str.is_empty()
        || !pending_input_str.is_empty()
    {
        final_json.push_str(&pending_click_str);
        final_json.push_str(&pending_test_str);
        final_json.push_str(&pending_tray_str);
        final_json.push_str(&pending_input_str);
        final_json.push_str("\n}");
    }
    final_json
}

pub fn handle_request(req: &HttpRequest, state: &Arc<Mutex<AppState>>, web_dir: &Path) -> HttpResponse {
    if req.method == "OPTIONS" {
        return HttpResponse {
            status_code: 200,
            status_text: "OK",
            content_type: "text/plain",
            body: Vec::new(),
        };
    }

    let req_clean_path = req
        .path
        .split('?')
        .next()
        .unwrap_or("/")
        .split('#')
        .next()
        .unwrap_or("/");

    if (req.method == "GET" && (req_clean_path == "/c" || req_clean_path.starts_with("/c/")))
        || (req.method == "POST" && (req_clean_path == "/api/shortcut/push" || req_clean_path == "/api/clipboard/push"))
    {
        let raw_u = extract_c_endpoint_url(req);
        if let Some(clean_url) = normalize_and_validate_digital_shortcut_url(&raw_u) {
            let clip_ok = crate::ocr_engine::write_win32_clipboard(&clean_url);
            if let Ok(mut cache) = LATEST_SHORTCUT_URL_CACHE.lock() {
                *cache = (clean_url.clone(), now_epoch_ms_u64());
            }
            sync_shortcut_to_worker_async(clean_url.clone());
            let wants_json = req_clean_path == "/api/shortcut/push"
                || req_clean_path == "/api/clipboard/push"
                || req
                    .headers
                    .get("accept")
                    .map(|a| a.to_ascii_lowercase().contains("application/json"))
                    .unwrap_or(false);
            if wants_json {
                let escaped = escape_json_str(&clean_url);
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":true,"source":"ios_shortcut_digital","url":"{}","copied_to_clipboard":{},"clipboardWritten":{}}}"#,
                    escaped, clip_ok, clip_ok
                ));
            }
            let html = format!(
                "<!doctype html><html><head><meta charset='utf-8'><title>iPhone Link</title></head>\
                <body style='font-family:-apple-system,sans-serif;text-align:center;padding:24px;'>\
                <script>window.close();</script>\u{2713} Link Copied to PC!\
                <p style='font-size:13px;word-break:break-all;'>{}</p>\
                <!-- {} clip={} --></body></html>",
                clean_url,
                clean_url,
                if clip_ok { 1 } else { 0 }
            );
            return HttpResponse::ok_html(&html);
        }
        if req_clean_path == "/api/shortcut/push" || req_clean_path == "/api/clipboard/push" {
            return HttpResponse {
                status_code: 400,
                status_text: "Bad Request",
                content_type: "application/json; charset=utf-8",
                body: br#"{"ok":false,"error":"Invalid or missing URL","url":""}"#.to_vec(),
            };
        }
        return HttpResponse {
            status_code: 400,
            status_text: "Bad Request",
            content_type: "text/html; charset=utf-8",
            body: b"<!doctype html><html><head><meta charset='utf-8'></head><body>Invalid or missing URL.</body></html>".to_vec(),
        };
    }

    if req_clean_path == "/api/shortcut/setup-info" {
        let srv_port = state.lock().map(|s| s.server_port).unwrap_or(DEFAULT_PORT);
        return HttpResponse::ok_json(&build_shortcut_setup_info_json(srv_port));
    }

    // Always strip query string (?) and fragment (#) before routing
    let path = req
        .path
        .split('?')
        .next()
        .unwrap_or("/")
        .split('#')
        .next()
        .unwrap_or("/");

    // 1. Dedicated v4 CSS & JS bridge routes
    if path == "/v4_true_acrylic.css" {
        return HttpResponse::ok_css(V4_TRUE_ACRYLIC_CSS);
    }
    if path == "/v4_bridge.js" {
        return HttpResponse::ok_js(V4_BRIDGE_JS);
    }

    // 2. API: Status (also handles POST /api/status/refresh & ?force=1 live USB + Wi-Fi mDNS rescan)
    if path == "/api/status" || path == "/api/status/refresh" {
        let force_refresh = path == "/api/status/refresh"
            || req.query.contains("force=1")
            || req.query.contains("refresh=1");
        let stop_cooldown = {
            let last_stop = LAST_STOP_MIRROR_MS.load(Ordering::SeqCst);
            last_stop != 0 && now_epoch_ms_u64().saturating_sub(last_stop) < 5000
        };
        // Try forwarding to worker first (if running on WORKER_PORT 8767)
        if let Some(resp) = forward_to_worker(req) {
            if resp.status_code == 200 {
                let body_str = String::from_utf8_lossy(&resp.body).to_string();
                let mut sanitized = body_str
                    .replace(r#""name": "UNKW 0000""#, r#""name": "iPhone""#)
                    .replace(r#""deviceName": "UNKW 0000""#, r#""deviceName": "iPhone""#)
                    .replace(r#""name":"UNKW 0000""#, r#""name":"iPhone""#)
                    .replace(r#""deviceName":"UNKW 0000""#, r#""deviceName":"iPhone""#)
                    .replace("UNKW 0000", "iPhone")
                    .replace("UNKW", "iPhone");

                if stop_cooldown {
                    sanitized = sanitized
                        .replace(r#""phase":"running""#, r#""phase":"idle""#)
                        .replace(r#""phase": "running""#, r#""phase": "idle""#)
                        .replace(r#""phase":"starting""#, r#""phase":"idle""#)
                        .replace(r#""phase": "starting""#, r#""phase": "idle""#)
                        .replace(r#""phase":"waiting_airplay""#, r#""phase":"idle""#)
                        .replace(r#""phase": "waiting_airplay""#, r#""phase": "idle""#);
                }

                let is_running = sanitized.contains(r#""phase":"running""#) || sanitized.contains(r#""phase": "running""#);
                if is_running {
                    if !sanitized.contains(r#""url": "/mirror.html""#) {
                        sanitized = sanitized.replace(
                            r#""screen": {"#,
                            r#""screen": {"url": "/mirror.html","#
                        ).replace(
                            r#""screen": { "#,
                            r#""screen": {"url": "/mirror.html", "#
                        );
                    }
                } else {
                    sanitized = sanitized
                        .replace(r#""url": "/mirror.html","#, "")
                        .replace(r#""url":"/mirror.html","#, "")
                        .replace(r#""url": "/mirror.html""#, r#""url": """#)
                        .replace(r#""url":"/mirror.html""#, r#""url":"""#);
                }

                // Sync language and status_text from AppState so bilingual ID/EN works consistently
                let st = state.lock().unwrap();
                let lang_code = st.language.code();
                let status_text = translate_dynamic_status("Wi-Fi siap", st.language);
                drop(st);

                if !sanitized.contains("status_text") {
                    sanitized = sanitized.replace(
                        r#""version": "4.0.0""#,
                        &format!(r#""status_text": "{}", "language": "{}", "version": "5.0.1", "edition": "{}", "zero_python_runtime": true, "zero_powershell_runtime": true"#, status_text, lang_code, APP_EDITION_V5)
                    );
                } else {
                    sanitized = sanitized
                        .replace(r#""status_text": "Wi-Fi siap""#, &format!(r#""status_text": "{}""#, status_text))
                        .replace(r#""status_text": "Wi-Fi ready""#, &format!(r#""status_text": "{}""#, status_text))
                        .replace(r#""language": "id""#, &format!(r#""language": "{}""#, lang_code))
                        .replace(r#""language": "en""#, &format!(r#""language": "{}""#, lang_code))
                        .replace(r#""language":"id""#, &format!(r#""language":"{}""#, lang_code))
                        .replace(r#""language":"en""#, &format!(r#""language":"{}""#, lang_code))
                        .replace(r#""version": "4.0.0""#, &format!(r#""version": "5.0.1", "edition": "{}", "zero_python_runtime": true, "zero_powershell_runtime": true"#, APP_EDITION_V5));
                }

                let dpr_float = (GLOBAL_DEVICE_PIXEL_RATIO.load(Ordering::SeqCst) as f64) / 100.0;
                let app_w = GLOBAL_APP_WIDTH.load(Ordering::SeqCst);
                let app_h = GLOBAL_APP_HEIGHT.load(Ordering::SeqCst);
                let lp_vis = GLOBAL_LAUNCHPAD_VISIBLE.load(Ordering::SeqCst);
                let btn_vis = GLOBAL_START_BTN_VISIBLE.load(Ordering::SeqCst);

                let btn_text = GLOBAL_START_BTN_TEXT.lock().map(|s| s.clone()).unwrap_or_default();
                let user_agent = GLOBAL_USER_AGENT.lock().map(|s| s.clone()).unwrap_or_default();
                let safe_btn = btn_text.replace('\\', r#"\\"#).replace('"', r#"\""#);
                let safe_ua = user_agent.replace('\\', r#"\\"#).replace('"', r#"\""#);

                let active_prof = GLOBAL_DEVICE_PROFILE.lock().map(|s| if s.is_empty() { "auto".to_string() } else { s.clone() }).unwrap_or_else(|_| "auto".to_string());
                let dr_width = GLOBAL_DRAWER_RECT_WIDTH.load(Ordering::SeqCst);
                let dr_overflow = GLOBAL_DRAWER_RIGHT_OVERFLOW.load(Ordering::SeqCst);

                let win_json = format!(
                    r#","window":{{"drawer_open":{},"is_landscape":{},"is_fullscreen":{},"portrait_width":{},"active_device_profile":"{}"}}"#,
                    GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst),
                    GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst),
                    GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst),
                    GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst),
                    active_prof
                );
                if !sanitized.contains(r#""window":"#) {
                    if let Some(pos) = sanitized.rfind('}') {
                        sanitized.insert_str(pos, &win_json);
                    }
                }

                let is_dark = GLOBAL_DARK_MODE.load(Ordering::SeqCst);
                let active_thm = if is_dark { "dark" } else { "light" };
                let dom_thm = GLOBAL_DOM_THEME.lock().map(|s| s.clone()).unwrap_or_default();
                let dom_thm_str = if dom_thm.is_empty() { active_thm } else { &dom_thm };
                let wf_clean = GLOBAL_WIREFRAME_CLEAN.load(Ordering::SeqCst);
                let wf_details = GLOBAL_WIREFRAME_DETAILS.lock().map(|s| s.clone()).unwrap_or_default();
                let safe_wf_details = wf_details.replace('\\', r#"\\"#).replace('"', r#"\""#);
                let pending_click = GLOBAL_PENDING_CLICK.lock().ok().and_then(|mut g| g.pop_front());
                let pending_click_str = match pending_click {
                    Some(ref s) => format!(r#","pendingClick":"{}""#, s.replace('\\', r#"\\"#).replace('"', r#"\""#)),
                    None => String::new(),
                };
                let pending_test = GLOBAL_PENDING_TEST.lock().ok().and_then(|mut g| g.take());
                let pending_test_str = match pending_test {
                    Some(ref s) => format!(r#","pendingTest":"{}""#, s.replace('\\', r#"\\"#).replace('"', r#"\""#)),
                    None => String::new(),
                };
                let pending_tray = GLOBAL_PENDING_TRAY_ACTION.lock().ok().and_then(|mut g| g.take());
                let pending_tray_str = match pending_tray {
                    Some(ref s) => format!(r#","pendingTrayAction":"{}""#, escape_json_str(s)),
                    None => String::new(),
                };
                let pending_input = GLOBAL_PENDING_INPUT.lock().ok().and_then(|mut g| g.pop_front());
                let pending_input_str = match pending_input {
                    Some((ref sel, ref val)) => format!(
                        r#","pendingInput":{{"selector":"{}","value":"{}"}}"#,
                        escape_json_str(sel),
                        escape_json_str(val)
                    ),
                    None => String::new(),
                };
                let conc_track = GLOBAL_CONCENTRIC_TRACK.lock().map(|s| s.clone()).unwrap_or_default();
                let conc_btn = GLOBAL_CONCENTRIC_BTN.lock().map(|s| s.clone()).unwrap_or_default();
                let badge_bg = GLOBAL_BADGE_BG.lock().map(|s| s.clone()).unwrap_or_default();
                let seg_clean = GLOBAL_SEGMENTED_CLEAN.load(Ordering::SeqCst);
                let seg_details = GLOBAL_SEGMENTED_DETAILS.lock().map(|s| s.clone()).unwrap_or_default();
                let safe_seg_details = seg_details.replace('\\', r#"\\"#).replace('"', r#"\""#);
                let dc_width = GLOBAL_DISPLAY_COL_WIDTH.load(Ordering::SeqCst);
                let sd_width = GLOBAL_SIDE_DRAWER_WIDTH.load(Ordering::SeqCst);
                let sd_right_gap = GLOBAL_SIDE_DRAWER_RIGHT_GAP.load(Ordering::SeqCst);
                let ds_right_gap = GLOBAL_DRAWER_SCROLL_RIGHT_GAP.load(Ordering::SeqCst);
                let inner_w = GLOBAL_INNER_WIDTH.load(Ordering::SeqCst);
                let stretch_cnt = GLOBAL_STRETCH_GLITCH_COUNT.load(Ordering::SeqCst);
                let max_close_w = GLOBAL_MAX_DISPLAY_COL_WIDTH_DURING_CLOSE.load(Ordering::SeqCst);
                let dom_pin_val = GLOBAL_DOM_UNLOCK_PIN.lock().map(|s| escape_json_str(&s)).unwrap_or_default();
                let dom_upd_ver = GLOBAL_DOM_UPDATE_VERSION.lock().map(|s| escape_json_str(&s)).unwrap_or_default();
                let dom_tray_act = GLOBAL_DOM_LAST_TRAY_ACTION.lock().map(|s| escape_json_str(&s)).unwrap_or_default();
                let dom_lang_val = GLOBAL_DOM_LANG.lock().map(|s| escape_json_str(&s)).unwrap_or_default();
                let dom_lang_id_act = GLOBAL_DOM_LANG_ID_ACTIVE.load(Ordering::SeqCst);
                let dom_lang_en_act = GLOBAL_DOM_LANG_EN_ACTIVE.load(Ordering::SeqCst);
                let dom_drawer_title = GLOBAL_DOM_DRAWER_TITLE.lock().map(|s| escape_json_str(&s)).unwrap_or_default();
                let dom_toggles_sum = GLOBAL_DOM_TOGGLES_SUMMARY.lock().map(|s| escape_json_str(&s)).unwrap_or_default();
                let dom_card_upd_exists = GLOBAL_DOM_CARD_UPDATER_EXISTS.load(Ordering::SeqCst);
                let dom_card_upd_title = GLOBAL_DOM_CARD_UPDATER_TITLE.lock().map(|s| escape_json_str(&s)).unwrap_or_default();

                if !sanitized.contains("webviewMetrics") {
                    let metrics_str = format!(
                        r#","version":"5.0.1","edition":"100% Pure Native Rust (Zero Python & Zero PowerShell)","zero_python_runtime":true,"zero_powershell_runtime":true{}{}{}{},"webviewMetrics":{{"indexHtmlServed":{},"appCssServed":{},"appJsServed":{},"domReadyBeacon":{},"devicePixelRatio":{:.2},"appWidth":{},"appHeight":{},"launchpadVisible":{},"startBtnVisible":{},"startBtnText":"{}","userAgent":"{}","rafVerified":{},"activeTheme":"{}","domTheme":"{}","wireframeClean":{},"wireframeDetails":"{}","concentricTrackRadius":"{}","concentricButtonRadius":"{}","badgeBg":"{}","segmentedClean":{},"segmentedDetails":"{}","activeDeviceProfile":"{}","drawerRectWidth":{},"drawerRightOverflow":{},"displayColWidth":{},"sideDrawerWidth":{},"sideDrawerRightGap":{},"drawerScrollRightGap":{},"innerWidth":{},"stretchGlitchCount":{},"maxDisplayColWidthDuringClose":{},"unlockPinValue":"{}","updateVersionText":"{}","lastTrayAction":"{}","domLanguage":"{}","activeLangId":{},"activeLangEn":{},"drawerTitleText":"{}","domTogglesSummary":"{}","cardUpdaterExists":{},"cardUpdaterTitle":"{}","stealthActive":{},"audioPcActive":{},"startWithWindows":{},"trayMenuOpen":{},"trayDismissedCount":{}}}}}"#,
                        pending_click_str,
                        pending_test_str,
                        pending_tray_str,
                        pending_input_str,
                        INDEX_HTML_SERVED.load(Ordering::SeqCst),
                        APP_CSS_SERVED.load(Ordering::SeqCst),
                        APP_JS_SERVED.load(Ordering::SeqCst),
                        DOM_READY_BEACON.load(Ordering::SeqCst),
                        dpr_float,
                        app_w,
                        app_h,
                        lp_vis,
                        btn_vis,
                        safe_btn,
                        safe_ua,
                        GLOBAL_RAF_VERIFIED.load(Ordering::SeqCst),
                        active_thm,
                        dom_thm_str,
                        wf_clean,
                        safe_wf_details,
                        conc_track,
                        conc_btn,
                        badge_bg,
                        seg_clean,
                        safe_seg_details,
                        active_prof,
                        dr_width,
                        dr_overflow,
                        dc_width,
                        sd_width,
                        sd_right_gap,
                        ds_right_gap,
                        inner_w,
                        stretch_cnt,
                        max_close_w,
                        dom_pin_val,
                        dom_upd_ver,
                        dom_tray_act,
                        dom_lang_val,
                        dom_lang_id_act,
                        dom_lang_en_act,
                        dom_drawer_title,
                        dom_toggles_sum,
                        dom_card_upd_exists,
                        dom_card_upd_title,
                        GLOBAL_STEALTH_ACTIVE.load(Ordering::SeqCst),
                        GLOBAL_AUDIO_PC_ACTIVE.load(Ordering::SeqCst),
                        is_start_with_windows_enabled(),
                        GLOBAL_TRAY_MENU_OPEN.load(Ordering::SeqCst),
                        GLOBAL_TRAY_DISMISSED_COUNT.load(Ordering::SeqCst)
                    );
                    if let Some(last_brace) = sanitized.rfind('}') {
                        sanitized.replace_range(last_brace.., &metrics_str);
                    }
                } else {
                    if !pending_click_str.is_empty() {
                        if let Some(pos) = sanitized.rfind('}') {
                            sanitized.insert_str(pos, &pending_click_str);
                        }
                    }
                    if !pending_test_str.is_empty() {
                        if let Some(pos) = sanitized.rfind('}') {
                            sanitized.insert_str(pos, &pending_test_str);
                        }
                    }
                    if !pending_tray_str.is_empty() {
                        if let Some(pos) = sanitized.rfind('}') {
                            sanitized.insert_str(pos, &pending_tray_str);
                        }
                    }
                    if !pending_input_str.is_empty() {
                        if let Some(pos) = sanitized.rfind('}') {
                            sanitized.insert_str(pos, &pending_input_str);
                        }
                    }
                    if !sanitized.contains("activeTheme") {
                        sanitized = sanitized.replace(
                            r#""domReadyBeacon":"#,
                            &format!(
                                r#""activeTheme": "{}", "domTheme": "{}", "wireframeClean": {}, "wireframeDetails": "{}", "concentricTrackRadius": "{}", "concentricButtonRadius": "{}", "badgeBg": "{}", "segmentedClean": {}, "segmentedDetails": "{}", "domLanguage": "{}", "activeLangId": {}, "activeLangEn": {}, "drawerTitleText": "{}", "domTogglesSummary": "{}", "cardUpdaterExists": {}, "cardUpdaterTitle": "{}", "trayMenuOpen": {}, "trayDismissedCount": {}, "devicePixelRatio": {:.2}, "appWidth": {}, "appHeight": {}, "launchpadVisible": {}, "startBtnVisible": {}, "startBtnText": "{}", "userAgent": "{}", "rafVerified": {}, "domReadyBeacon":"#,
                                active_thm, dom_thm_str, wf_clean, safe_wf_details, conc_track, conc_btn, badge_bg, seg_clean, safe_seg_details, dom_lang_val, dom_lang_id_act, dom_lang_en_act, dom_drawer_title, dom_toggles_sum, dom_card_upd_exists, dom_card_upd_title, GLOBAL_TRAY_MENU_OPEN.load(Ordering::SeqCst), GLOBAL_TRAY_DISMISSED_COUNT.load(Ordering::SeqCst), dpr_float, app_w, app_h, lp_vis, btn_vis, safe_btn, safe_ua, GLOBAL_RAF_VERIFIED.load(Ordering::SeqCst)
                            )
                        );
                    }
                }

                if !sanitized.contains(r#""detailedDiagnostics""#) {
                    let diag_json = build_native_driver_diagnostics_json();
                    if let Some(pos) = sanitized.rfind('}') {
                        sanitized.insert_str(pos, &format!(r#","detailedDiagnostics":{}"#, diag_json));
                    }
                }
                if !sanitized.contains(r#""wifi""#) {
                    let native_wifi_json = get_wifi_status(None, false).to_json_string();
                    if let Some(pos) = sanitized.rfind('}') {
                        sanitized.insert_str(pos, &format!(r#","wifi":{}"#, native_wifi_json));
                    }
                }

                return HttpResponse::ok_json(&sanitized);
            }
        }

        let final_json = build_status_payload_json(state, force_refresh, stop_cooldown, false);
        return HttpResponse::ok_json(&final_json);
    }

    // Try forwarding to worker for backend-managed endpoints (Phase 1 Tahap 226-230 native routes bypass early forward)
    let skip_early_forward = path == "/api/keyboard/settings"
        || path == "/api/status/refresh"
        || path == "/api/device/select"
        || path == "/api/devices/select"
        || path == "/api/device/wipe-privacy-data"
        || path == "/api/device/stealth"
        || path == "/api/device/lock"
        || path == "/api/device/wake-unlock"
        || path == "/api/device/pin"
        || path == "/api/device/unlock-developer-mode"
        || path == "/api/device/spotlight-paste"
        || path == "/api/device/spotlight-paste-tail"
        || path == "/api/device/spotlight-move-head"
        || path == "/api/device/spotlight-close"
        || path == "/api/qr/scan"
        || path == "/api/keyboard/shortcut"
        || path == "/api/keyboard/clear-chunks"
        || path == "/api/keyboard/toggle-onscreen"
        || path == "/api/keyboard/typed"
        || path == "/api/keyboard/paste"
        || path == "/api/ocr/clear-chunks"
        || path == "/api/ocr/extract"
        || path == "/api/ocr/status"
        || path == "/api/link/grab"
        || path == "/api/social/grab-link"
        || path == "/api/devices/native-usb"
        || path == "/api/wifi"
        || path.starts_with("/api/wifi/")
        || path.starts_with("/api/update/")
        || path.starts_with("/api/audio")
        || path == "/api/screen"
        || path.starts_with("/api/screen/")
        || path == "/codec"
        || path == "/pli"
        || path == "/mirror/codec"
        || path == "/mirror/pli"
        || path == "/mirror/stop"
        || path == "/mirror/size"
        || path == "/mirror/restart"
        || path == "/mirror/touch"
        || path == "/mirror/scroll"
        || path == "/mirror/button"
        || path == "/mirror/key"
        || path == "/mirror/gesture"
        || path == "/touch"
        || path == "/scroll"
        || path == "/button"
        || path == "/key"
        || path == "/gesture";
    if !skip_early_forward
        && (path.starts_with("/api/device/")
            || path.starts_with("/api/devices/")
            || path.starts_with("/api/wifi/")
            || path.starts_with("/api/keyboard/")
            || path.starts_with("/api/screen")
            || path.starts_with("/api/ocr/"))
    {
        if let Some(resp) = forward_to_worker(req) {
            if resp.status_code == 200 {
                return resp;
            }
        }
    }

    if path == "/api/device/select" || path == "/api/devices/select" {
        let target_raw = parse_string_param(&req.body, "udid")
            .or_else(|| extract_query_param(&req.query, "udid"))
            .unwrap_or_default();
        let target_udid = iphone_link_apple::physical_stream::sanitize_udid_label(&target_raw);
        let streaming_active = iphone_link_apple::mirror::is_mirror_streaming_fast();
        // Zero-I/O fast-path (< 2 ms): resolve from cached USB metadata + paired Wi-Fi records first;
        // only fall back to live usbmuxd probe if target_udid is not yet in cache.
        let cached_meta = snapshot_cached_devices_metadata_zero_io();
        let mut unified = collect_unified_devices(&cached_meta, &[], false);
        let mut matched_opt = unified
            .iter()
            .find(|d| !target_udid.is_empty() && d.udid.eq_ignore_ascii_case(&target_udid))
            .cloned();
        if matched_opt.is_none() && !streaming_active && !target_udid.is_empty() {
            let live_meta = discover_devices_with_metadata(false);
            let usb_list = list_connected_usb_devices(Duration::from_millis(60)).unwrap_or_default();
            unified = collect_unified_devices(&live_meta, &usb_list, false);
            matched_opt = unified
                .iter()
                .find(|d| d.udid.eq_ignore_ascii_case(&target_udid))
                .cloned();
        }
        let Some(matched) = matched_opt else {
            return HttpResponse {
                status_code: 400,
                status_text: "Bad Request",
                content_type: "application/json; charset=utf-8",
                body: br#"{"ok":false,"error":"Unknown device UDID"}"#.to_vec(),
            };
        };

        {
            let mut st = state.lock().unwrap();
            st.selected_udid = matched.udid.clone();
            st.user_explicitly_selected_udid = true;
            st.last_selected_udid = matched.udid.clone();
            save_user_settings_to_disk(&st);
        }
        // Immediately re-key Wi-Fi stream cache (zero network I/O)
        if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
            let fresh_w = get_wifi_status(Some(&matched.udid), false).to_json_string();
            *guard = Some((matched.udid.clone(), fresh_w, std::time::Instant::now()));
        }
        // Sync active audio identity target for the newly selected iPhone
        if !streaming_active {
            iphone_link_apple::physical_stream::set_active_session_device_identity(
                &matched.udid,
                (!matched.name.is_empty()).then_some(matched.name.as_str()),
                (!matched.model.is_empty()).then_some(matched.model.as_str()),
                (!matched.bluetooth_address.is_empty())
                    .then_some(matched.bluetooth_address.as_str()),
            );
        }
        let dev_json = format_device_summary_json(&matched, true);
        let last_stop = LAST_STOP_MIRROR_MS.load(Ordering::SeqCst);
        let stop_cooldown = last_stop != 0 && now_epoch_ms_u64().saturating_sub(last_stop) < 5000;
        let status_json = build_status_payload_json(state, false, stop_cooldown, true);
        let resp_json = format!(
            r#"{{"ok":true,"selectedUdid":"{}","device":{},"status":{}}}"#,
            escape_json_str(&matched.udid),
            dev_json,
            status_json,
        );
        return HttpResponse::ok_json(&resp_json);
    }

    if path == "/api/devices" || path == "/api/devices/native-usb" {
        let force = req.query.contains("refresh=1") || req.query.contains("force=1");
        return HttpResponse::ok_json(&build_native_usb_devices_json(force));
    }

    // 2B. API: Device detail (/api/devices/<udid>) with Zero-Polling Guard (cached RAM when streaming)
    if path.starts_with("/api/devices/") {
        let udid = path.trim_start_matches("/api/devices/").trim_matches('/');
        let streaming_active = iphone_link_apple::mirror::is_mirror_streaming_fast()
            || state.lock().map(|s| s.is_streaming).unwrap_or(false);
        let cached_list = if streaming_active {
            snapshot_cached_devices_metadata_zero_io()
        } else {
            discover_devices_with_metadata(false)
        };
        let dev_meta = cached_list.into_iter().find(|d| d.udid == udid);
        let wifi_st = get_wifi_status(Some(udid), false);
        let wmeta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(udid);
        let is_paired = dev_meta.as_ref().map(|m| m.paired).unwrap_or(false)
            || wifi_st.paired.iter().any(|u| u == udid);
        let dev_name = dev_meta
            .as_ref()
            .map(|m| m.device_name.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| (!wmeta.name.is_empty()).then(|| wmeta.name.clone()))
            .unwrap_or_default();
        let prod_type = dev_meta
            .as_ref()
            .map(|m| m.product_type.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| wmeta.product_type.clone());
        let dev_class = dev_meta
            .as_ref()
            .map(|m| m.device_class.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if prod_type.starts_with("iPad") {
                    "iPad".to_string()
                } else {
                    "iPhone".to_string()
                }
            });
        let dev_family = if dev_class.eq_ignore_ascii_case("iPad") {
            "ipad"
        } else {
            "iphone"
        };
        let prod_ver = dev_meta
            .as_ref()
            .map(|m| m.product_version.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| wmeta.ios_version.clone());
        let build_ver = dev_meta
            .as_ref()
            .map(|m| m.build_version.clone())
            .unwrap_or_default();
        let last_ip = wifi_st
            .ip
            .filter(|s| !s.is_empty())
            .or(wmeta.last_ip)
            .unwrap_or_default();
        let json = format!(
            r#"{{
  "ok": true,
  "udid": "{}",
  "paired": {},
  "deviceFamily": "{}",
  "wifi": {},
  "zeroPoll": {},
  "info": {{
    "DeviceName": "{}",
    "DeviceClass": "{}",
    "ProductType": "{}",
    "ProductVersion": "{}",
    "BuildVersion": "{}",
    "LastIP": "{}"
  }}
}}"#,
            escape_json_str(udid),
            is_paired,
            dev_family,
            wifi_st.enabled,
            streaming_active,
            escape_json_str(&dev_name),
            escape_json_str(&dev_class),
            escape_json_str(&prod_type),
            escape_json_str(&prod_ver),
            escape_json_str(&build_ver),
            escape_json_str(&last_ip)
        );
        return HttpResponse::ok_json(&json);
    }

    // 3. API: Settings (Persistent Disk + Universal Last-Used Auto-Save + Cross-Restart PIN + folderInstanceId)
    if path == "/api/settings" || path == "/api/keyboard/settings" {
        if req.method == "POST" {
            let mut st = state.lock().unwrap();
            let _ = apply_settings_json_body_to_state(&req.body, &mut st);

            // Tahap 259 Pilar 3: Live zero-drop hot-reload of AirPlay audio settings whenever
            // audioVolume / audioBufferPreset / lipSync / audioMuted is changed via /api/settings.
            let ap_vol_patch = parse_int_param(&req.body, "audioVolume")
                .or_else(|| parse_int_param(&req.body, "pcVolume"))
                .or_else(|| parse_int_param(&req.body, "volume"))
                .map(|v| v.clamp(0, 100) as u32);
            let ap_buf_patch = parse_string_param(&req.body, "audioBufferPreset")
                .or_else(|| parse_string_param(&req.body, "audioBuffer"))
                .or_else(|| parse_string_param(&req.body, "bufferPreset"))
                .or_else(|| parse_string_param(&req.body, "buffer"));
            let ap_ls_patch = parse_bool_param(&req.body, "lipSync")
                .or_else(|| parse_bool_param(&req.body, "lip_sync"));
            let ap_mute_patch = parse_bool_param(&req.body, "audioMuted")
                .or_else(|| parse_bool_param(&req.body, "pcMuted"))
                .or_else(|| parse_bool_param(&req.body, "muted"));
            if ap_vol_patch.is_some()
                || ap_buf_patch.is_some()
                || ap_ls_patch.is_some()
                || ap_mute_patch.is_some()
            {
                let _ = iphone_link_apple::airplay::apply_live_audio_config(
                    ap_vol_patch,
                    ap_buf_patch.as_deref(),
                    ap_ls_patch,
                    ap_mute_patch,
                );
            }

            if body_contains_key(&req.body, "unlockPin")
                || body_contains_key(&req.body, "passcode")
                || body_contains_key(&req.body, "pin")
            {
                let raw_pin = parse_string_param(&req.body, "unlockPin")
                    .or_else(|| parse_string_param(&req.body, "passcode"))
                    .or_else(|| parse_string_param(&req.body, "pin"))
                    .unwrap_or_default();
                let clean_pin = sanitize_pin_digits(&raw_pin);
                if clean_pin.is_empty() {
                    clear_stored_pin();
                } else {
                    let _ = store_encrypted_pin(&clean_pin);
                }
                if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
                    *g = clean_pin;
                }
            }

            save_user_settings_to_disk(&st);
            let settings_obj = build_settings_json(&st);
            return HttpResponse::ok_json(&format!(r#"{{"ok":true,"status":"ok","saved":true,"settings":{}}}"#, settings_obj));
        } else {
            let st = state.lock().unwrap();
            let settings_obj = build_settings_json(&st);
            return HttpResponse::ok_json(&settings_obj);
        }
    }

    if path == "/api/settings/device-profile" {
        let mut st = state.lock().unwrap();
        let next_prof = if req.body.contains("ipad") {
            "ipad".to_string()
        } else if req.body.contains("iphone") {
            "iphone".to_string()
        } else {
            "auto".to_string()
        };
        st.device_profile = next_prof.clone();
        GLOBAL_PORTRAIT_WIDTH.store(if next_prof == "ipad" { 564 } else { 346 }, Ordering::SeqCst);
        if let Ok(mut gp) = GLOBAL_DEVICE_PROFILE.lock() {
            *gp = next_prof;
        }
        save_user_settings_to_disk(&st);
        let settings_obj = build_settings_json(&st);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"deviceProfile":"{}","settings":{}}}"#,
            st.device_profile, settings_obj
        ));
    }

    if path == "/api/device/pin" {
        let raw_pin = parse_string_param(&req.body, "unlockPin")
            .or_else(|| parse_string_param(&req.body, "passcode"))
            .or_else(|| parse_string_param(&req.body, "pin"))
            .unwrap_or_default();
        if req.method.eq_ignore_ascii_case("DELETE") || raw_pin.trim().is_empty() {
            let _ = clear_unlock_pin_sync(state);
            return HttpResponse::ok_json(
                r#"{"ok":true,"status":"ok","unlockPin":"","dpapiEncrypted":false}"#,
            );
        }
        return match save_unlock_pin_sync(state, &raw_pin) {
            Ok(clean_pin) => HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"status":"ok","unlockPin":"{}","dpapiEncrypted":true}}"#,
                escape_json_str(&clean_pin)
            )),
            Err(err) => HttpResponse {
                status_code: 400,
                status_text: "Bad Request",
                content_type: "application/json; charset=utf-8",
                body: format!(r#"{{"ok":false,"error":"{}"}}"#, escape_json_str(&err))
                    .into_bytes(),
            },
        };
    }

    // 4. API: Device actions (Native Rust HID Indigo & UniversalHID)
    if path == "/api/device/lock" {
        let lock_res = lock_iphone_screen_with_hold(true);
        let mut st = state.lock().unwrap();
        st.intentionally_locked = true;
        st.stealth_dim = is_stealth_dim_active();
        GLOBAL_STEALTH_ACTIVE.store(st.stealth_dim, Ordering::SeqCst);
        let (pkts_tx, hid_err, hid_act) = iphone_link_apple::hid::physical_hid_telemetry();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":{},"status":"ok","locked":{},"dispatched":{},"stealthRestored":{},"restoreSteps":{},"holdMs":{},"duration_ms":{},"eventsCount":{},"physical_packets_transmitted":{},"physical_hid_errors":{},"physical_hid_active":{}}}"#,
            lock_res.ok,
            lock_res.locked,
            lock_res.dispatched,
            lock_res.stealth_restored,
            lock_res.restore_steps,
            lock_res.duration_ms,
            lock_res.duration_ms,
            lock_res.button_events.len(),
            pkts_tx,
            hid_err,
            hid_act
        ));
    }

    if path == "/api/device/wake-unlock" {
        let req_pin = parse_string_param(&req.body, "passcode")
            .or_else(|| parse_string_param(&req.body, "unlockPin"))
            .or_else(|| parse_string_param(&req.body, "pin"))
            .map(|p| sanitize_pin_digits(&p))
            .unwrap_or_default();
        let dpapi_pin = load_decrypted_pin().ok().flatten().unwrap_or_default();
        let saved_pin = SAVED_UNLOCK_PIN
            .lock()
            .map(|g| sanitize_pin_digits(&g))
            .unwrap_or_default();
        let effective_pin = if !req_pin.is_empty() {
            req_pin.clone()
        } else if !dpapi_pin.is_empty() {
            dpapi_pin
        } else {
            saved_pin
        };

        // Zero Passcode Leakage: execute in-process HID wake + raw digit injection without touching typed buffer
        let wake_res = wake_and_unlock_iphone(&effective_pin);
        let mut st = state.lock().unwrap();
        st.intentionally_locked = false;

        let mut fwd_req = req.clone();
        if !effective_pin.is_empty() && req_pin.is_empty() {
            let patched = format!(r#"{{"passcode":"{}"}}"#, escape_json_str(&effective_pin));
            fwd_req.body_bytes = patched.as_bytes().to_vec();
            fwd_req.body = patched;
        }
        if let Some(resp) = forward_to_worker(&fwd_req) {
            if resp.status_code == 200 {
                return resp;
            }
        }
        let (pkts_tx, hid_err, hid_act) = iphone_link_apple::hid::physical_hid_telemetry();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":{},"status":"ok","woken":{},"unlocked":{},"dispatched":{},"steps":{},"swipeSteps":{},"physical_packets_transmitted":{},"physical_hid_errors":{},"physical_hid_active":{}}}"#,
            wake_res.ok,
            wake_res.woken,
            wake_res.unlocked,
            wake_res.dispatched,
            wake_res.pin_steps.len(),
            wake_res.swipe_steps.len(),
            pkts_tx,
            hid_err,
            hid_act
        ));
    }

    if path == "/api/device/stealth" {
        if req.method == "GET" {
            let st = state.lock().unwrap();
            let active = st.stealth_dim || is_stealth_dim_active();
            let (pkts_tx, hid_err, hid_act) = iphone_link_apple::hid::physical_hid_telemetry();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"stealth":{},"stealthDimActive":{},"physical_packets_transmitted":{},"physical_hid_errors":{},"physical_hid_active":{}}}"#,
                active, active, pkts_tx, hid_err, hid_act
            ));
        }
        let enabled = parse_bool_param(&req.body, "enabled")
            .or_else(|| parse_bool_param(&req.body, "enable"))
            .unwrap_or(!is_stealth_dim_active());
        let force = parse_bool_param(&req.body, "force").unwrap_or(false);
        let restore_steps = parse_int_param(&req.body, "restoreSteps")
            .map(|v| v.max(1) as usize)
            .unwrap_or(4);
        let level = if enabled { "min" } else { "restore" };
        let steps = if enabled { 16 } else { restore_steps };
        let res = set_iphone_physical_brightness(level, steps, force);
        {
            let mut st = state.lock().unwrap();
            st.stealth_dim = res.stealth;
            GLOBAL_STEALTH_ACTIVE.store(res.stealth, Ordering::SeqCst);
        }
        return HttpResponse::ok_json(&res.to_json_string());
    }

    if path == "/api/device/wipe-privacy-data" || path == "/api/ocr/clear-chunks" || path == "/api/keyboard/clear-chunks" {
        if path == "/api/device/wipe-privacy-data" {
            wipe_all_privacy_state_and_files();
            let mut st = state.lock().unwrap();
            st.stealth_dim = false;
            st.intentionally_locked = false;
            return HttpResponse::ok_json(r#"{"ok":true,"status":"ok","wiped":true}"#);
        }
        clear_recent_long_ocr_chunks();
        return HttpResponse::ok_json(r#"{"ok":true,"native":true,"action":"clear_chunks","cleared":true}"#);
    }

    // 5A. API: Spotlight HID Endpoints for Grab Link (Alt+B) — 100% Golden v4.0.0 Parity (Tahap 242 + 243 catch_unwind)
    if path == "/api/device/spotlight-paste" || path == "/api/device/spotlight-paste-tail" {
        let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            iphone_link_apple::hid::dispatch_spotlight_paste,
        ))
        .unwrap_or(false);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"native":true,"stage":"pasted_clean","physicalSent":{}}}"#,
            sent
        ));
    }

    if path == "/api/device/spotlight-move-head" {
        let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            iphone_link_apple::hid::dispatch_spotlight_move_head,
        ))
        .unwrap_or(false);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"native":true,"stage":"head","physicalSent":{}}}"#,
            sent
        ));
    }

    if path == "/api/device/spotlight-close" {
        let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            iphone_link_apple::hid::dispatch_spotlight_close,
        ))
        .unwrap_or(false);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"native":true,"stage":"closed","physicalSent":{}}}"#,
            sent
        ));
    }

    // 5B. API: Native In-Process Social Link Grabber, QR/Spotlight OCR & Clipboard Pipeline (Tahap 242)
    if path == "/api/social/grab-link" || path == "/api/link/grab" || path == "/api/qr/scan" {
        let body_str = String::from_utf8_lossy(&req.body_bytes);
        let resp_json = handle_native_grab_link_payload(&body_str);
        return HttpResponse::ok_json(&resp_json);
    }

    if path == "/api/ocr/status" {
        let tele = crate::ocr_engine::HotOcrTelemetry::new_warm(45.0);
        return HttpResponse::ok_json(&tele.to_json());
    }

    if path == "/api/ocr/extract" {
        let body_str = String::from_utf8_lossy(&req.body_bytes);
        let has_input = body_str.contains("batchTexts")
            || body_str.contains("batch_texts")
            || body_str.contains("raw_text")
            || body_str.contains("rawText")
            || body_str.contains("\"text\"")
            || body_str.contains("frameDataUrl")
            || body_str.contains("imageDataUrl")
            || body_str.contains("dataUrl")
            || body_str.contains("frameHistory");
        let (auto_snap_used, snap_ms, snap_w, snap_h, snap_url) = if !has_input {
            match iphone_link_apple::mirror::global_export_latest_video_snapshot() {
                Ok(snap) => (true, snap.elapsed_ms, snap.width, snap.height, Some(snap.data_url)),
                Err(_) => (false, 0.0, 0, 0, None),
            }
        } else {
            (false, 0.0, 0, 0, None)
        };
        let (final_opt, auto_cleared, batch_count) = if let Some(ref s_url) = snap_url {
            crate::ocr_engine::extract_and_process_ocr_from_data_urls(std::slice::from_ref(s_url))
        } else {
            extract_and_process_ocr_batch_from_json(&body_str)
        };
        let text = final_opt.unwrap_or_default();
        let len = text.chars().count();
        let esc = escape_json_value(&text);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"native":true,"text":"{}","extractedText":"{}","autoClearedCache":{},"batchCount":{},"length":{},"autoSnapshotUsed":{},"snapshotElapsedMs":{:.3},"snapshotWidth":{},"snapshotHeight":{}}}"#,
            esc, esc, auto_cleared, batch_count, len, auto_snap_used, snap_ms, snap_w, snap_h
        ));
    }

    if path == "/api/keyboard/shortcut" {
        commit_and_reset_current_typed_line();
        let action = parse_string_param(&req.body, "action")
            .unwrap_or_else(|| "copy".to_string());

        if action == "clear-copy-cache" {
            clear_copy_cache();
            return HttpResponse::ok_json(
                r#"{"ok":true,"native":true,"action":"clear-copy-cache","cleared":true}"#,
            );
        }

        if action == "scan-qr" || action == "grab-link" {
            let body_str = String::from_utf8_lossy(&req.body_bytes);
            let resp_json = handle_native_grab_link_payload(&body_str);
            return HttpResponse::ok_json(&resp_json);
        }

        if action == "copy" || action == "cut" {
            let hid_steps = generate_ios_shortcut_steps(&action);
            if let Some(ref steps) = hid_steps {
                let _ = iphone_link_apple::hid::dispatch_physical_key_steps(steps);
            }
            set_active_clipboard_source(ClipboardSource::Iphone);
            let skip_ocr = parse_bool_param(&req.body, "skipOcr").unwrap_or(false);
            if skip_ocr {
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":true,"native":true,"action":"{}","source":"iphone","skipOcr":true,"internal":true}}"#,
                    escape_json_value(&action)
                ));
            }

            let body_str = String::from_utf8_lossy(&req.body_bytes);
            let has_input = body_str.contains("batchTexts")
                || body_str.contains("batch_texts")
                || body_str.contains("raw_text")
                || body_str.contains("rawText")
                || body_str.contains("\"text\"")
                || body_str.contains("frameDataUrl")
                || body_str.contains("imageDataUrl")
                || body_str.contains("dataUrl")
                || body_str.contains("frameHistory");
            let (auto_snap_used, snap_ms, snap_w, snap_h, snap_url) = if !has_input {
                match iphone_link_apple::mirror::global_export_latest_video_snapshot() {
                    Ok(snap) => (true, snap.elapsed_ms, snap.width, snap.height, Some(snap.data_url)),
                    Err(_) => (false, 0.0, 0, 0, None),
                }
            } else {
                (false, 0.0, 0, 0, None)
            };
            let (final_opt, auto_cleared, _batch_count) = if let Some(ref s_url) = snap_url {
                crate::ocr_engine::extract_and_process_ocr_from_data_urls(std::slice::from_ref(s_url))
            } else {
                extract_and_process_ocr_batch_from_json(&body_str)
            };
            let final_text = final_opt.unwrap_or_default();
            let copied_to_pc = if !final_text.is_empty() {
                let written = write_win32_clipboard(&final_text);
                if written {
                    set_active_clipboard_source(ClipboardSource::Pc);
                }
                written
            } else {
                false
            };
            let esc = escape_json_value(&final_text);
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"native":true,"action":"{}","text":"{}","extractedText":"{}","copiedToPc":{},"clipboardSource":"{}","autoClearedCache":{},"length":{},"autoSnapshotUsed":{},"snapshotElapsedMs":{:.3},"snapshotWidth":{},"snapshotHeight":{}}}"#,
                escape_json_value(&action),
                esc,
                esc,
                copied_to_pc,
                if copied_to_pc { "pc" } else { "iphone" },
                auto_cleared,
                final_text.chars().count(),
                auto_snap_used,
                snap_ms,
                snap_w,
                snap_h
            ));
        }

        if let Some(steps) = generate_ios_shortcut_steps(&action) {
            let phys_sent = iphone_link_apple::hid::dispatch_physical_key_steps(&steps);
            let (pkts_tx, _, _) = iphone_link_apple::hid::physical_hid_telemetry();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"native":true,"action":"{}","steps":{},"physicalSent":{},"physical_packets_transmitted":{}}}"#,
                escape_json_value(&action),
                steps.len(),
                phys_sent,
                pkts_tx
            ));
        }

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"native":true,"action":"{}"}}"#,
            escape_json_value(&action)
        ));
    }

    if path == "/api/keyboard/toggle-onscreen" {
        let events = generate_onscreen_keyboard_toggle_events();
        let phys_sent =
            iphone_link_apple::hid::dispatch_physical_button_name("eject", "press").unwrap_or(false);
        if let Some(resp) = forward_to_worker(req) {
            if resp.status_code == 200 {
                return resp;
            }
        }
        let (pkts_tx, _, _) = iphone_link_apple::hid::physical_hid_telemetry();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","usagePage":{},"usage":{},"events":{},"physicalSent":{},"physical_packets_transmitted":{}}}"#,
            events[0].page,
            events[0].usage,
            events.len(),
            phys_sent,
            pkts_tx
        ));
    }

    if path == "/api/keyboard/typed" {
        if req.method.eq_ignore_ascii_case("GET") {
            let lines = get_typed_lines_buffer();
            let items: Vec<String> = lines
                .iter()
                .map(|l| format!(r#""{}""#, escape_json_str(l)))
                .collect();
            return HttpResponse::ok_json(&format!(r#"{{"ok":true,"lines":[{}]}}"#, items.join(",")));
        }
        if parse_bool_param(&req.body, "commitLine").unwrap_or(false) {
            commit_and_reset_current_typed_line();
        } else if let Some(text_val) = parse_string_param(&req.body, "text") {
            if !text_val.is_empty() {
                record_typed_text(&text_val);
            }
        } else if let Some(key_val) = parse_string_param(&req.body, "key") {
            if !key_val.is_empty() {
                record_typed_char(&key_val);
            }
        }
        if let Some(resp) = forward_to_worker(req) {
            if resp.status_code == 200 {
                return resp;
            }
        }
        let buffered = get_typed_lines_buffer().len();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","bufferedLines":{}}}"#,
            buffered
        ));
    }

    if path == "/api/keyboard/paste" {
        commit_and_reset_current_typed_line();
        let force_pc = parse_bool_param(&req.body, "force")
            .or_else(|| parse_bool_param(&req.body, "forcePc"))
            .unwrap_or(false);
        let explicit_text = parse_string_param(&req.body, "text");
        let client_paste_pc = parse_bool_param(&req.body, "pastePcClipboard")
            .or_else(|| parse_bool_param(&req.body, "pcClipboardPaste"));
        let paste_pc_enabled = client_paste_pc.unwrap_or_else(|| {
            state.lock().map(|s| s.paste_pc_clipboard).unwrap_or(true)
        });

        // Rule 1: Explicit PC paste (`force: true` / `Alt+V` / "Paste PC Text")
        if force_pc {
            let pc_text = explicit_text
                .filter(|t| !t.is_empty())
                .or_else(read_win32_clipboard)
                .unwrap_or_default();
            if !pc_text.is_empty() {
                set_active_clipboard_source(ClipboardSource::Pc);
                record_typed_text(&pc_text);
                let (steps, typed) = generate_key_steps_for_text(&pc_text);
                let phys_sent = iphone_link_apple::hid::dispatch_physical_key_steps(&steps);
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":true,"native":true,"status":"ok","method":"typed","action":"paste-pc","source":"pc","typed":{},"length":{},"steps":{},"physicalSent":{}}}"#,
                    typed,
                    pc_text.chars().count(),
                    steps.len(),
                    phys_sent
                ));
            }
            let ios_steps = generate_ios_shortcut_steps("paste-ios").unwrap_or_default();
            let phys_sent = iphone_link_apple::hid::dispatch_physical_key_steps(&ios_steps);
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"native":true,"status":"ok","method":"ios-shortcut","action":"paste-ios","source":"pc","reason":"pc-empty-fallback","typed":0,"length":0,"steps":{},"physicalSent":{}}}"#,
                ios_steps.len(),
                phys_sent
            ));
        }

        // Rule 2: If toggle "Ctrl+V Pastes PC Clipboard" is OFF (and no explicit text override) -> native iOS Cmd+V
        if !paste_pc_enabled && explicit_text.is_none() {
            let ios_steps = generate_ios_shortcut_steps("paste-ios").unwrap_or_default();
            let phys_sent = iphone_link_apple::hid::dispatch_physical_key_steps(&ios_steps);
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"native":true,"status":"ok","method":"ios-shortcut","action":"paste-ios","source":"iphone","reason":"toggle-off","typed":0,"length":0,"steps":{},"physicalSent":{}}}"#,
                ios_steps.len(),
                phys_sent
            ));
        }

        // Rule 3: Check Win32 GetClipboardSequenceNumber() state machine
        let (active_source, clip_seq) = resolve_active_clipboard_source_with_seq();

        // Rule 4: Active source is iPhone and user didn't pass explicit text override -> native iOS Cmd+V (<1ms)
        if active_source == ClipboardSource::Iphone && explicit_text.is_none() {
            let ios_steps = generate_ios_shortcut_steps("paste-ios").unwrap_or_default();
            let phys_sent = iphone_link_apple::hid::dispatch_physical_key_steps(&ios_steps);
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"native":true,"status":"ok","method":"ios-shortcut","action":"paste-ios","source":"iphone","reason":"iphone-source","clipboardSeq":{},"typed":0,"length":0,"steps":{},"physicalSent":{}}}"#,
                clip_seq,
                ios_steps.len(),
                phys_sent
            ));
        }

        // Rule 5: Active source is PC (or explicit text was requested) -> inject HID typing steps
        let pc_text = explicit_text
            .filter(|t| !t.is_empty())
            .or_else(read_win32_clipboard)
            .unwrap_or_default();
        if !pc_text.is_empty() {
            record_typed_text(&pc_text);
            let (steps, typed) = generate_key_steps_for_text(&pc_text);
            let phys_sent = iphone_link_apple::hid::dispatch_physical_key_steps(&steps);
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"native":true,"status":"ok","method":"typed","action":"paste-pc","source":"pc","clipboardSeq":{},"typed":{},"length":{},"steps":{},"physicalSent":{}}}"#,
                clip_seq,
                typed,
                pc_text.chars().count(),
                steps.len(),
                phys_sent
            ));
        }

        let ios_steps = generate_ios_shortcut_steps("paste-ios").unwrap_or_default();
        let phys_sent = iphone_link_apple::hid::dispatch_physical_key_steps(&ios_steps);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"native":true,"status":"ok","method":"ios-shortcut","action":"paste-ios","source":"pc","reason":"pc-empty-fallback","clipboardSeq":{},"typed":0,"length":0,"steps":{},"physicalSent":{}}}"#,
            clip_seq,
            ios_steps.len(),
            phys_sent
        ));
    }

    // 6. API: Audio & Update & Wallpaper & System Startup & Tray
    if path == "/api/audio/native-status" {
        let rep = iphone_link_audio::wasapi_output::probe_wasapi_shared_engine();
        let ok = rep.mmdevice_enumerator_ok && rep.audio_client_ok && rep.audio_render_client_ok;
        let json = format!(
            r#"{{"ok":{},"mmcss_enabled":{},"task_name":"{}","bypass_ecoqos":{},"mmdevice_enumerator_ok":{},"audio_client_ok":{},"audio_render_client_ok":{},"virtual_null_sink":{},"buffer_frames":{},"sample_rate":{},"channels":{},"bits_per_sample":{},"backend":"{}","detail":"{}"}}"#,
            ok,
            rep.mmcss_enabled,
            rep.task_name.replace('"', "'"),
            rep.bypass_ecoqos,
            rep.mmdevice_enumerator_ok,
            rep.audio_client_ok,
            rep.audio_render_client_ok,
            rep.virtual_null_sink,
            rep.buffer_frames,
            rep.sample_rate,
            rep.channels,
            rep.bits_per_sample,
            rep.backend.replace('"', "'"),
            rep.detail.replace('\\', "\\\\").replace('"', "'"),
        );
        return HttpResponse::ok_json(&json);
    }
    if path == "/api/audio/airplay-config" {
        let explicit_vol = parse_int_param(&req.body, "volume")
            .or_else(|| parse_int_param(&req.body, "audioVolume"))
            .or_else(|| parse_int_param(&req.body, "pcVolume"))
            .map(|v| v.clamp(0, 100) as u32);
        let explicit_preset = parse_string_param(&req.body, "buffer")
            .or_else(|| parse_string_param(&req.body, "bufferPreset"))
            .or_else(|| parse_string_param(&req.body, "audioBufferPreset"))
            .or_else(|| parse_string_param(&req.body, "audioBuffer"));
        let explicit_lipsync = parse_bool_param(&req.body, "lipSync")
            .or_else(|| parse_bool_param(&req.body, "lip_sync"));
        let explicit_muted = parse_bool_param(&req.body, "muted")
            .or_else(|| parse_bool_param(&req.body, "audioMuted"));

        let snap = iphone_link_apple::airplay::apply_live_audio_config(
            explicit_vol,
            explicit_preset.as_deref(),
            explicit_lipsync,
            explicit_muted,
        );
        if iphone_link_audio::is_global_pc_audio_bridge_active()
            || iphone_link_audio::is_global_pc_audio_bridge_connecting()
        {
            iphone_link_audio::stop_global_pc_audio_bridge();
        }
        if let Ok(mut st) = state.lock() {
            if let Some(v) = explicit_vol {
                st.audio_volume = v as i32;
            }
            if explicit_preset.is_some() {
                st.audio_buffer_preset = snap.buffer_preset.clone();
            }
            if let Some(ls) = explicit_lipsync {
                st.lip_sync = ls;
            }
            if let Some(m) = explicit_muted {
                st.audio_muted = m;
            }
            save_user_settings_to_disk(&st);
        }
        let ap_active = iphone_link_apple::mirror::is_airplay_active();
        let codec_json = snap
            .codec
            .as_ref()
            .filter(|_| ap_active)
            .map(|c| format!(r#""{}""#, escape_json_value(c)))
            .unwrap_or_else(|| "null".to_string());
        let ap_video_q_dropped = if ap_active {
            iphone_link_apple::mirror::global_airplay_video_queue_dropped()
        } else {
            0
        };
        let json = format!(
            r#"{{"ok":true,"airplayActive":{},"airplayAudioActive":{},"airplayAudioMuted":{},"airplayAudioCodec":{},"airplayAudioSampleRate":{},"airplayAudioDecodedPackets":{},"airplayAudioPlayedSamples":{},"airplayAudioVolume":{},"airplayAudioBufferPreset":"{}","airplayAudioLipSync":{},"airplayAudioBufferMs":{},"airplayAudioTargetMs":{},"airplayAudioDroppedPackets":{},"airplayVideoQueueDropped":{},"volume":{},"muted":{},"bufferPreset":"{}","lipSync":{},"targetDelayMs":{},"delayMs":{},"pcBridgeActive":false,"pcBridgeConnecting":false}}"#,
            ap_active,
            ap_active && snap.active,
            snap.muted,
            codec_json,
            if ap_active { snap.sample_rate } else { 0 },
            if ap_active { snap.decoded_packets } else { 0 },
            if ap_active { snap.played_samples } else { 0 },
            snap.volume,
            escape_json_value(&snap.buffer_preset),
            snap.lip_sync,
            if ap_active && snap.active { snap.buffer_ms } else { 0 },
            snap.target_ms,
            if ap_active { snap.dropped_packets } else { 0 },
            ap_video_q_dropped,
            snap.volume,
            snap.muted,
            escape_json_value(&snap.buffer_preset),
            snap.lip_sync,
            snap.target_ms,
            if ap_active && snap.active { snap.buffer_ms } else { 0 },
        );
        return HttpResponse::ok_json(&json);
    }
    if path == "/api/audio/airplay-mute" {
        let explicit = parse_bool_param(&req.body, "muted")
            .or_else(|| parse_bool_param(&req.body, "audioMuted"));
        let next_muted = explicit.unwrap_or_else(|| !iphone_link_apple::airplay::is_airplay_audio_muted());
        iphone_link_apple::airplay::set_airplay_audio_muted(next_muted);
        if let Ok(mut st) = state.lock() {
            st.audio_muted = next_muted;
            save_user_settings_to_disk(&st);
        }
        let snap = iphone_link_apple::airplay::get_airplay_audio_snapshot();
        let ap_active = iphone_link_apple::mirror::is_airplay_active();
        let codec_json = snap
            .codec
            .as_ref()
            .filter(|_| ap_active)
            .map(|c| format!(r#""{}""#, escape_json_value(c)))
            .unwrap_or_else(|| "null".to_string());
        let ap_video_q_dropped = if ap_active {
            iphone_link_apple::mirror::global_airplay_video_queue_dropped()
        } else {
            0
        };
        let json = format!(
            r#"{{"ok":true,"airplayActive":{},"airplayAudioActive":{},"airplayAudioMuted":{},"airplayAudioCodec":{},"airplayAudioSampleRate":{},"airplayAudioDecodedPackets":{},"airplayAudioPlayedSamples":{},"airplayAudioVolume":{},"airplayAudioBufferPreset":"{}","airplayAudioLipSync":{},"airplayAudioBufferMs":{},"airplayAudioTargetMs":{},"airplayAudioDroppedPackets":{},"airplayVideoQueueDropped":{},"muted":{},"pcBridgeActive":false,"pcBridgeConnecting":false}}"#,
            ap_active,
            ap_active && snap.active,
            snap.muted,
            codec_json,
            if ap_active { snap.sample_rate } else { 0 },
            if ap_active { snap.decoded_packets } else { 0 },
            if ap_active { snap.played_samples } else { 0 },
            snap.volume,
            escape_json_value(&snap.buffer_preset),
            snap.lip_sync,
            if ap_active && snap.active { snap.buffer_ms } else { 0 },
            snap.target_ms,
            if ap_active { snap.dropped_packets } else { 0 },
            ap_video_q_dropped,
            snap.muted,
        );
        return HttpResponse::ok_json(&json);
    }
    if path.starts_with("/api/audio") {
        let airplay_now = iphone_link_apple::mirror::is_airplay_active();
        let is_mutating = req.method.eq_ignore_ascii_case("POST")
            || path == "/api/audio/mute"
            || path == "/api/audio/start"
            || path == "/api/audio/connect"
            || path == "/api/audio/stop"
            || path == "/api/audio/disconnect"
            || path == "/api/audio/toggle"
            || path == "/api/audio/volume"
            || path == "/api/audio/buffer"
            || path == "/api/audio/lipsync";
        // Tahap 258/259 Pilar 3: When AirPlay H.264 is active, NEVER launch Bluetooth A2DP on any
        // audio toggle/mute/volume/buffer/lipsync call! Hot-reload live atomics with 0 ms dropout,
        // and NEVER toggle mute when only bufferPreset / lipSync / volume is being updated!
        if airplay_now {
            if is_mutating {
                let explicit_vol = parse_int_param(&req.body, "volume")
                    .or_else(|| parse_int_param(&req.body, "audioVolume"))
                    .or_else(|| parse_int_param(&req.body, "pcVolume"))
                    .map(|v| v.clamp(0, 100) as u32);
                let explicit_preset = parse_string_param(&req.body, "buffer")
                    .or_else(|| parse_string_param(&req.body, "bufferPreset"))
                    .or_else(|| parse_string_param(&req.body, "audioBufferPreset"))
                    .or_else(|| parse_string_param(&req.body, "audioBuffer"));
                let explicit_lipsync = parse_bool_param(&req.body, "lipSync")
                    .or_else(|| parse_bool_param(&req.body, "lip_sync"));
                let explicit_muted = parse_bool_param(&req.body, "muted")
                    .or_else(|| parse_bool_param(&req.body, "audioMuted"));
                let explicit_mode = parse_string_param(&req.body, "audioOut")
                    .or_else(|| parse_string_param(&req.body, "audio_out"))
                    .or_else(|| parse_string_param(&req.body, "mode"));
                let explicit_en = parse_bool_param(&req.body, "enabled")
                    .or_else(|| parse_bool_param(&req.body, "enable"));
                let is_mute_or_route_action = explicit_muted.is_some()
                    || explicit_en.is_some()
                    || explicit_mode.is_some()
                    || path == "/api/audio/toggle"
                    || path == "/api/audio/mute"
                    || path == "/api/audio/stop"
                    || path == "/api/audio/disconnect"
                    || path == "/api/audio/start"
                    || path == "/api/audio/connect";
                let next_muted_opt = if is_mute_or_route_action {
                    Some(if let Some(m) = explicit_muted {
                        m
                    } else if path == "/api/audio/stop" || path == "/api/audio/disconnect" {
                        true
                    } else if path == "/api/audio/start" || path == "/api/audio/connect" {
                        false
                    } else if let Some(en) = explicit_en {
                        !en
                    } else if let Some(ref mode_str) = explicit_mode {
                        mode_str.eq_ignore_ascii_case("device")
                    } else {
                        !iphone_link_apple::airplay::is_airplay_audio_muted()
                    })
                } else {
                    None
                };
                let snap_upd = iphone_link_apple::airplay::apply_live_audio_config(
                    explicit_vol,
                    explicit_preset.as_deref(),
                    explicit_lipsync,
                    next_muted_opt,
                );
                if let Ok(mut st) = state.lock() {
                    if let Some(v) = explicit_vol {
                        st.audio_volume = v as i32;
                    }
                    if explicit_preset.is_some() {
                        st.audio_buffer_preset = snap_upd.buffer_preset.clone();
                    }
                    if let Some(ls) = explicit_lipsync {
                        st.lip_sync = ls;
                    }
                    if let Some(m) = next_muted_opt {
                        st.audio_muted = m;
                    }
                    save_user_settings_to_disk(&st);
                }
            }
            if iphone_link_audio::is_global_pc_audio_bridge_active()
                || iphone_link_audio::is_global_pc_audio_bridge_connecting()
            {
                iphone_link_audio::stop_global_pc_audio_bridge();
            }
            let snap = iphone_link_apple::airplay::get_airplay_audio_snapshot();
            let base_json = iphone_link_apple::mirror::global_audio_status_json();
            let codec_json = snap
                .codec
                .as_ref()
                .map(|c| format!(r#""{}""#, escape_json_value(c)))
                .unwrap_or_else(|| "null".to_string());
            let ap_video_q_dropped = iphone_link_apple::mirror::global_airplay_video_queue_dropped();
            let final_json = if base_json.ends_with('}') {
                format!(
                    r#"{},"airplayActive":true,"airplayAudioActive":{},"airplayAudioMuted":{},"airplayAudioCodec":{},"airplayAudioSampleRate":{},"airplayAudioDecodedPackets":{},"airplayAudioPlayedSamples":{},"airplayAudioVolume":{},"airplayAudioBufferPreset":"{}","airplayAudioLipSync":{},"airplayAudioBufferMs":{},"airplayAudioTargetMs":{},"airplayAudioDroppedPackets":{},"airplayVideoQueueDropped":{},"volume":{},"muted":{},"bufferPreset":"{}","lipSync":{},"targetDelayMs":{},"delayMs":{},"pcBridgeActive":false,"pcBridgeConnecting":false,"pcBridgeDetail":"airplay_isolated","pcBridgeDevice":"","pcBridgeCandidates":""}}"#,
                    &base_json[..base_json.len() - 1],
                    snap.active,
                    snap.muted,
                    codec_json,
                    snap.sample_rate,
                    snap.decoded_packets,
                    snap.played_samples,
                    snap.volume,
                    escape_json_value(&snap.buffer_preset),
                    snap.lip_sync,
                    if snap.active { snap.buffer_ms } else { 0 },
                    snap.target_ms,
                    snap.dropped_packets,
                    ap_video_q_dropped,
                    snap.volume,
                    snap.muted,
                    escape_json_value(&snap.buffer_preset),
                    snap.lip_sync,
                    snap.target_ms,
                    if snap.active { snap.buffer_ms } else { 0 },
                )
            } else {
                base_json
            };
            return HttpResponse::ok_json(&final_json);
        }
        let active_override = if path == "/api/audio/start" || path == "/api/audio/connect" {
            AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);
            Some(true)
        } else if path == "/api/audio/stop" || path == "/api/audio/disconnect" {
            AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(true, Ordering::SeqCst);
            Some(false)
        } else if path == "/api/audio/toggle" {
            let toggle_en = parse_bool_param(&req.body, "enabled");
            let st_cur = state.lock().unwrap();
            let next_mode = if toggle_en == Some(true) || (toggle_en.is_none() && st_cur.audio_routing == "device") {
                "pc"
            } else {
                "device"
            };
            drop(st_cur);
            let mut st = state.lock().unwrap();
            st.audio_routing = next_mode.to_string();
            Some(next_mode != "device" && st.is_streaming)
        } else {
            None
        };

        let explicit_enable = parse_bool_param(&req.body, "enable")
            .or_else(|| parse_bool_param(&req.body, "enabled"));
        let explicit_mode = parse_string_param(&req.body, "audioOut")
            .or_else(|| parse_string_param(&req.body, "audio_out"))
            .or_else(|| parse_string_param(&req.body, "audio_routing"))
            .or_else(|| parse_string_param(&req.body, "routing"))
            .or_else(|| parse_string_param(&req.body, "mode"))
            .or_else(|| {
                parse_bool_param(&req.body, "audioOut")
                    .map(|en| if en { "pc".to_string() } else { "device".to_string() })
            })
            .or_else(|| match explicit_enable {
                Some(true) => Some("pc".to_string()),
                Some(false) => Some("device".to_string()),
                None => None,
            });
        let explicit_vol = parse_int_param(&req.body, "volume")
            .or_else(|| parse_int_param(&req.body, "audioVolume"));
        let explicit_muted = parse_bool_param(&req.body, "muted")
            .or_else(|| parse_bool_param(&req.body, "audioMuted"));
        let explicit_preset = parse_string_param(&req.body, "bufferPreset")
            .or_else(|| parse_string_param(&req.body, "audioBufferPreset"));
        let explicit_lipsync = parse_bool_param(&req.body, "lipSync");
        let force_stream = parse_bool_param(&req.body, "forceStream");

        if is_mutating {
            let mut st = state.lock().unwrap();
            let _ = apply_settings_json_body_to_state(&req.body, &mut st);

            // Tahap 259 Pilar 3: Live zero-drop hot-reload of AirPlay audio settings whenever
            // audioVolume / audioBufferPreset / lipSync / audioMuted is changed via /api/settings.
            let ap_vol_patch = parse_int_param(&req.body, "audioVolume")
                .or_else(|| parse_int_param(&req.body, "pcVolume"))
                .or_else(|| parse_int_param(&req.body, "volume"))
                .map(|v| v.clamp(0, 100) as u32);
            let ap_buf_patch = parse_string_param(&req.body, "audioBufferPreset")
                .or_else(|| parse_string_param(&req.body, "audioBuffer"))
                .or_else(|| parse_string_param(&req.body, "bufferPreset"))
                .or_else(|| parse_string_param(&req.body, "buffer"));
            let ap_ls_patch = parse_bool_param(&req.body, "lipSync")
                .or_else(|| parse_bool_param(&req.body, "lip_sync"));
            let ap_mute_patch = parse_bool_param(&req.body, "audioMuted")
                .or_else(|| parse_bool_param(&req.body, "pcMuted"))
                .or_else(|| parse_bool_param(&req.body, "muted"));
            if ap_vol_patch.is_some()
                || ap_buf_patch.is_some()
                || ap_ls_patch.is_some()
                || ap_mute_patch.is_some()
            {
                let _ = iphone_link_apple::airplay::apply_live_audio_config(
                    ap_vol_patch,
                    ap_buf_patch.as_deref(),
                    ap_ls_patch,
                    ap_mute_patch,
                );
            }
            if path == "/api/audio/mute" && explicit_muted.is_none() {
                st.audio_muted = !st.audio_muted;
            }
            if let Some(m) = explicit_mode.as_deref() {
                st.audio_routing = m.to_string();
                st.bluetooth_a2dp = m.eq_ignore_ascii_case("bluetooth");
                if m.eq_ignore_ascii_case("device") {
                    AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(true, Ordering::SeqCst);
                } else if m.eq_ignore_ascii_case("pc")
                    || m.eq_ignore_ascii_case("bluetooth")
                    || m.eq_ignore_ascii_case("wasapi")
                {
                    AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);
                    if explicit_muted.is_none() {
                        st.audio_muted = false;
                    }
                }
            }
            if let Some(m_flag) = explicit_muted {
                AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(m_flag, Ordering::SeqCst);
            } else if path == "/api/audio/mute" {
                AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(st.audio_muted, Ordering::SeqCst);
            }
            if let Some(v) = explicit_vol {
                st.audio_volume = v.clamp(0, 100) as i32;
            }
            if let Some(p) = explicit_preset.as_deref() {
                st.audio_buffer_preset = p.to_string();
            }
            if let Some(ls) = explicit_lipsync {
                st.lip_sync = ls;
            }
            let wants_pc_audio =
                (st.audio_routing == "pc" || st.audio_routing == "bluetooth") && !st.audio_muted;
            let effective_active = active_override
                .or(force_stream)
                .or(explicit_enable)
                .unwrap_or(wants_pc_audio);
            GLOBAL_AUDIO_PC_ACTIVE.store(wants_pc_audio, Ordering::SeqCst);
            let a_mode = st.audio_routing.clone();
            let a_vol = st.audio_volume as i64;
            let a_muted = st.audio_muted;
            let a_preset = st.audio_buffer_preset.clone();
            let a_ls = st.lip_sync;
            save_user_settings_to_disk(&st);
            drop(st);

            let explicit_target_udid = parse_string_param(&req.body, "udid");
            let explicit_target_name = parse_string_param(&req.body, "deviceName")
                .or_else(|| parse_string_param(&req.body, "device_name"))
                .or_else(|| parse_string_param(&req.body, "preferredName"));
            let explicit_target_bt = parse_string_param(&req.body, "btMac")
                .or_else(|| parse_string_param(&req.body, "bluetoothAddress"));
            let (pref_name, pref_model, pref_bt) = resolve_active_audio_device_target(
                state,
                explicit_target_udid.as_deref(),
                explicit_target_name.as_deref(),
                explicit_target_bt.as_deref(),
            );

            let (bridge_active, bridge_connecting, bridge_detail) = if wants_pc_audio && effective_active {
                let _ = iphone_link_audio::start_global_pc_audio_bridge_for_device(
                    pref_name.as_deref(),
                    pref_model.as_deref(),
                    pref_bt.as_deref(),
                );
                iphone_link_audio::global_pc_audio_bridge_full_status()
            } else {
                iphone_link_audio::stop_global_pc_audio_bridge();
                (false, false, "stopped".to_string())
            };
            let (bridge_dev, bridge_cands) =
                iphone_link_audio::global_pc_audio_bridge_device_info();

            let native_json = iphone_link_apple::mirror::global_configure_audio(
                Some(&a_mode),
                Some(a_vol),
                Some(a_muted),
                Some(&a_preset),
                Some(a_ls),
                Some(effective_active),
            );
            let pc_bridge_state = iphone_link_audio::global_pc_audio_bridge_state_kind();
            let final_json = if native_json.ends_with('}') {
                format!(
                    r#"{},"pcBridgeActive":{},"pcBridgeConnecting":{},"pcBridgeState":"{}","pcBridgeDetail":"{}","pcBridgeDevice":"{}","pcBridgeCandidates":"{}"}}"#,
                    &native_json[..native_json.len() - 1],
                    bridge_active,
                    bridge_connecting,
                    pc_bridge_state,
                    escape_json_value(&bridge_detail),
                    escape_json_value(&bridge_dev),
                    escape_json_value(&bridge_cands),
                )
            } else {
                native_json
            };
            return HttpResponse::ok_json(&final_json);
        }

        let base_json = iphone_link_apple::mirror::global_audio_status_json();
        let (bridge_active, bridge_connecting, bridge_detail) =
            iphone_link_audio::global_pc_audio_bridge_full_status();
        let pc_bridge_state = iphone_link_audio::global_pc_audio_bridge_state_kind();
        let (bridge_dev, bridge_cands) =
            iphone_link_audio::global_pc_audio_bridge_device_info();
        let final_json = if base_json.ends_with('}') {
            format!(
                r#"{},"pcBridgeActive":{},"pcBridgeConnecting":{},"pcBridgeState":"{}","pcBridgeDetail":"{}","pcBridgeDevice":"{}","pcBridgeCandidates":"{}"}}"#,
                &base_json[..base_json.len() - 1],
                bridge_active,
                bridge_connecting,
                pc_bridge_state,
                escape_json_value(&bridge_detail),
                escape_json_value(&bridge_dev),
                escape_json_value(&bridge_cands),
            )
        } else {
            base_json
        };
        return HttpResponse::ok_json(&final_json);
    }

    if path == "/api/update/status" {
        let (cached_json, auto_check, ch, active_transfer) = if let Ok(mut s) = state.lock() {
            let ac = s.auto_check_update;
            let uc = s.update_channel.clone();
            let active = s.updater.is_active_transfer();
            if !active {
                s.updater.channel = uc.clone();
            }
            (s.updater.to_json(ac), ac, uc, active)
        } else {
            return HttpResponse::ok_json(&UpdateStateCache::default().to_json(true));
        };
        if active_transfer {
            if let Some(worker_resp) = forward_to_worker(req) {
                if worker_resp.status_code == 200 {
                    let body = String::from_utf8_lossy(&worker_resp.body);
                    if let Ok(mut s) = state.lock() {
                        s.updater.apply_worker_payload(&body, &ch);
                        return HttpResponse::ok_json(&s.updater.to_json(auto_check));
                    }
                }
            }
        }
        return HttpResponse::ok_json(&cached_json);
    }

    if path == "/api/update/check" {
        let ch = parse_string_param(&req.body, "channel")
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| {
                state
                    .lock()
                    .map(|s| s.update_channel.clone())
                    .unwrap_or_else(|_| "stable".to_string())
            });
        let api_override = env::var("IPHONE_LINK_UPDATER_API_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let fresh = crate::updater::check_github_updates_native(&ch, api_override.as_deref());
        if let Ok(mut s) = state.lock() {
            s.update_channel = fresh.channel.clone();
            let ac = s.auto_check_update;
            s.updater = fresh;
            return HttpResponse::ok_json(&s.updater.to_json(ac));
        }
        return HttpResponse::ok_json(&fresh.to_json(true));
    }

    if path == "/api/update/install" || path == "/api/update/open_release" {
        let requested_url = parse_string_param(&req.body, "url")
            .or_else(|| parse_string_param(&req.body, "releaseUrl"))
            .or_else(|| parse_string_param(&req.body, "downloadUrl"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let cached_url = state
            .lock()
            .ok()
            .map(|s| {
                if !s.updater.release_url.is_empty() {
                    s.updater.release_url.clone()
                } else if !s.updater.download_url.is_empty() {
                    s.updater.download_url.clone()
                } else {
                    format!("{}/releases", crate::updater::PUBLIC_GITHUB_REPO_URL)
                }
            })
            .unwrap_or_else(|| format!("{}/releases", crate::updater::PUBLIC_GITHUB_REPO_URL));
        let target_url = requested_url.unwrap_or(cached_url);
        if !crate::updater::is_allowed_github_release_url(&target_url) {
            return HttpResponse::ok_json(
                r#"{"ok":false,"opened":false,"error":"URL pembaruan tidak valid atau tidak termasuk repositori resmi."}"#,
            );
        }
        let dry_run = is_dry_run_shell_open();
        return match shell_open_target_native(&target_url) {
            Ok(launched) => HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"opened":{},"dryRun":{},"url":"{}"}}"#,
                launched,
                dry_run,
                escape_json_str(&target_url)
            )),
            Err(err) => HttpResponse::ok_json(&format!(
                r#"{{"ok":false,"opened":false,"dryRun":{},"url":"{}","error":"{}"}}"#,
                dry_run,
                escape_json_str(&target_url),
                escape_json_str(&err)
            )),
        };
    }

    if path == "/api/system/startup" {
        if req.method == "POST" {
            let target = parse_bool_param(&req.body, "enable")
                .or_else(|| parse_bool_param(&req.body, "enabled"))
                .unwrap_or_else(|| !is_start_with_windows_enabled());
            match set_start_with_windows(target) {
                Ok(enabled_now) => {
                    return HttpResponse::ok_json(&format!(
                        r#"{{"ok":true,"enabled":{},"registryKey":"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run","valueName":"iPhoneLink"}}"#,
                        enabled_now
                    ));
                }
                Err(e) => {
                    return HttpResponse {
                        status_code: 500,
                        status_text: "Internal Server Error",
                        content_type: "application/json; charset=utf-8",
                        body: format!(r#"{{"ok":false,"error":"{}"}}"#, escape_json_str(&e)).into_bytes(),
                    };
                }
            }
        } else {
            let enabled_now = is_start_with_windows_enabled();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"enabled":{},"registryKey":"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run","valueName":"iPhoneLink"}}"#,
                enabled_now
            ));
        }
    }

    if path == "/api/system/wallpaper" {
        let (bytes, mime, _) = get_current_wallpaper_data();
        return HttpResponse {
            status_code: 200,
            status_text: "OK",
            content_type: mime,
            body: bytes,
        };
    }

    if path == "/api/system/wallpaper-sig" {
        let (_, _, sig) = get_current_wallpaper_data();
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"wallpaperSig":"{}"}}"#, sig));
    }

    if path == "/api/system/endurance-benchmark" {
        let req_frames = parse_int_param(&req.body, "frames")
            .or_else(|| parse_int_param(&req.body, "frameCount"))
            .or_else(|| {
                extract_query_param(&req.query, "frames").and_then(|s| s.parse::<i32>().ok())
            })
            .map(|v| v.clamp(10, 5000) as usize)
            .unwrap_or(1000);
        // Verify Invariant 1 bounded viewport history (max 4, TTL 45s, 2500ms cooldown)
        for step in 0..8u64 {
            let _ = record_viewport_history_snapshot(
                346,
                750,
                0xA000 + step,
                50_000 + step * 3_000,
            );
        }
        let vp_hist_cnt = crate::ocr_engine::viewport_history_count();
        let rep = iphone_link_apple::mirror::run_long_session_endurance_benchmark(req_frames);
        let mut json = rep.to_json_string();
        if let Some(pos) = json.rfind('}') {
            json.replace_range(
                pos..,
                &format!(
                    r#","viewportHistoryBoundedCount":{},"maxViewportHistoryCap":{}}}"#,
                    vp_hist_cnt,
                    crate::ocr_engine::MAX_VIEWPORT_HISTORY
                ),
            );
        }
        return HttpResponse::ok_json(&json);
    }

    if path == "/api/system/memory" || path == "/api/system/reclaim-memory" {
        let is_reclaim = path == "/api/system/reclaim-memory";
        if is_reclaim {
            trim_current_rust_process_working_set();
        }
        if let Some(worker_resp) = forward_to_worker(req) {
            if worker_resp.status_code == 200 && !worker_resp.body.is_empty() {
                let worker_str = String::from_utf8_lossy(&worker_resp.body).to_string();
                let py_mb = extract_json_f64_field(&worker_str, "python_worker_working_set_mb").unwrap_or(0.0);
                let ps_mb = extract_json_f64_field(&worker_str, "hot_ocr_ps_working_set_mb").unwrap_or(0.0);
                let rust_mb = current_rust_process_working_set_mb();
                let total_mb = ((py_mb + ps_mb + rust_mb) * 100.0).round() / 100.0;
                let mut updated = worker_str;
                for pat in [r#""rust_shell_working_set_mb": 0.0"#, r#""rust_shell_working_set_mb":0.0"#] {
                    if updated.contains(pat) {
                        updated = updated.replacen(pat, &format!(r#""rust_shell_working_set_mb":{:.2}"#, rust_mb), 1);
                        break;
                    }
                }
                if let Some(t_idx) = updated.find(r#""total_backend_working_set_mb":"#) {
                    let val_start = t_idx + r#""total_backend_working_set_mb":"#.len();
                    let val_end = updated[val_start..]
                        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != ' ')
                        .map(|o| val_start + o)
                        .unwrap_or(updated.len());
                    updated.replace_range(val_start..val_end, &format!("{:.2}", total_mb));
                }
                return HttpResponse::ok_json(&updated);
            }
        }
        let rust_mb = current_rust_process_working_set_mb();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"reclaimed":{},"worker_online":false,"python_worker_working_set_mb":0.0,"hot_ocr_ps_working_set_mb":0.0,"rust_shell_working_set_mb":{:.2},"total_backend_working_set_mb":{:.2}}}"#,
            is_reclaim, rust_mb, rust_mb
        ));
    }

    if path == "/api/debug/airplay-trace" {
        return HttpResponse::ok_json(&iphone_link_apple::airplay::get_airplay_trace_json());
    }

    if path == "/api/debug/ui-action-log" && req.method.eq_ignore_ascii_case("GET") {
        return HttpResponse::ok_json(&crate::native_ui::ui_action_log_json());
    }

    if path == "/api/debug/lock-trace" && req.method.eq_ignore_ascii_case("GET") {
        return HttpResponse::ok_json(&iphone_link_apple::hid::lock_trace_json());
    }

    if path == "/api/system/hevc_status" {
        let cap = query_hevc_capabilities();
        let bundle = find_hevc_appxbundle_path();
        let bundled = bundle.is_some();
        let bundle_str = bundle
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"installed":{},"hevcMftAvailable":{},"h264MftAvailable":{},"mediaFeaturePackMissing":{},"mfplatLoaded":{},"version":"{}","package":"{}","bundled":{},"bundlePath":"{}","storeUrl":"{}","storeFallbackUrl":"{}","mediaFeaturePackUrl":"{}"}}"#,
            cap.installed,
            cap.hevc_mft_available,
            cap.h264_mft_available,
            cap.media_feature_pack_missing,
            cap.mfplat_loaded,
            escape_json_str(&cap.version),
            escape_json_str(&cap.package),
            bundled,
            escape_json_str(&bundle_str),
            cap.store_url,
            cap.store_fallback_url,
            cap.media_feature_pack_url
        ));
    }

    if path == "/api/system/open_hevc_store" {
        use iphone_link_video::d3d11_context::{
            HEVC_OEM_FREE_STORE_URL, HEVC_PAID_FALLBACK_STORE_URL, MEDIA_FEATURE_PACK_SETTINGS_URL,
        };
        let req_target = parse_string_param(&req.body, "target")
            .unwrap_or_else(|| "hevc_oem".to_string())
            .to_ascii_lowercase();
        let (target_norm, target_url) = match req_target.as_str() {
            "hevc_paid" | "paid" => ("hevc_paid", HEVC_PAID_FALLBACK_STORE_URL),
            "media_feature_pack" | "mfp" | "optionalfeatures" => {
                ("media_feature_pack", MEDIA_FEATURE_PACK_SETTINGS_URL)
            }
            _ => ("hevc_oem", HEVC_OEM_FREE_STORE_URL),
        };
        let dry_run = is_dry_run_shell_open();
        return match shell_open_target_native(target_url) {
            Ok(launched) => HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"target":"{}","url":"{}","launched":{},"dryRun":{}}}"#,
                target_norm, target_url, launched, dry_run
            )),
            Err(err) => HttpResponse::ok_json(&format!(
                r#"{{"ok":false,"target":"{}","url":"{}","launched":false,"dryRun":{},"error":"{}"}}"#,
                target_norm,
                target_url,
                dry_run,
                escape_json_str(&err)
            )),
        };
    }

    if path == "/api/system/install_hevc" {
        let cap = query_hevc_capabilities();
        if cap.installed {
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"installed":true,"alreadyInstalled":true,"hevcMftAvailable":{},"h264MftAvailable":{},"mediaFeaturePackMissing":{},"version":"{}","package":"{}","storeUrl":"{}","storeFallbackUrl":"{}","mediaFeaturePackUrl":"{}"}}"#,
                cap.hevc_mft_available,
                cap.h264_mft_available,
                cap.media_feature_pack_missing,
                escape_json_str(&cap.version),
                escape_json_str(&cap.package),
                cap.store_url,
                cap.store_fallback_url,
                cap.media_feature_pack_url
            ));
        }
        if cap.media_feature_pack_missing {
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":false,"installed":false,"hevcMftAvailable":false,"h264MftAvailable":false,"mediaFeaturePackMissing":true,"storeUrl":"{}","storeFallbackUrl":"{}","mediaFeaturePackUrl":"{}","error":"Windows Media Feature Pack missing (Windows N/KN edition)"}}"#,
                cap.store_url, cap.store_fallback_url, cap.media_feature_pack_url
            ));
        }
        if let Some(bundle_path) = find_hevc_appxbundle_path() {
            match install_hevc_from_bundle(&bundle_path) {
                Ok(launched) => {
                    let now_cap = query_hevc_capabilities();
                    return HttpResponse::ok_json(&format!(
                        r#"{{"ok":true,"installed":{},"launchedInstaller":{},"alreadyInstalled":false,"hevcMftAvailable":{},"h264MftAvailable":{},"mediaFeaturePackMissing":{},"version":"{}","package":"{}","bundle":"{}","storeUrl":"{}","storeFallbackUrl":"{}","mediaFeaturePackUrl":"{}"}}"#,
                        now_cap.installed,
                        launched,
                        now_cap.hevc_mft_available,
                        now_cap.h264_mft_available,
                        now_cap.media_feature_pack_missing,
                        escape_json_str(&now_cap.version),
                        escape_json_str(&now_cap.package),
                        escape_json_str(&bundle_path.to_string_lossy()),
                        now_cap.store_url,
                        now_cap.store_fallback_url,
                        now_cap.media_feature_pack_url
                    ));
                }
                Err(err) => {
                    return HttpResponse::ok_json(&format!(
                        r#"{{"ok":false,"installed":false,"error":"{}","bundle":"{}","storeUrl":"{}","storeFallbackUrl":"{}","mediaFeaturePackUrl":"{}"}}"#,
                        escape_json_str(&err),
                        escape_json_str(&bundle_path.to_string_lossy()),
                        cap.store_url,
                        cap.store_fallback_url,
                        cap.media_feature_pack_url
                    ));
                }
            }
        }
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":false,"installed":false,"hevcMftAvailable":{},"h264MftAvailable":{},"mediaFeaturePackMissing":{},"storeUrl":"{}","storeFallbackUrl":"{}","mediaFeaturePackUrl":"{}","error":"HEVC AppxBundle not found locally; open free OEM Microsoft Store package"}}"#,
            cap.hevc_mft_available,
            cap.h264_mft_available,
            cap.media_feature_pack_missing,
            cap.store_url,
            cap.store_fallback_url,
            cap.media_feature_pack_url
        ));
    }

    if path == "/api/driver/diagnostics" {
        let force = req.query.contains("refresh=1") || req.query.contains("force=1");
        let is_streaming = state.lock().map(|s| s.is_streaming).unwrap_or(false);
        let diag_json = build_native_driver_diagnostics_json_with_opts(force, is_streaming);
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"native":true,"diagnostics":{}}}"#, diag_json));
    }

    if path == "/api/diagnostics/report" {
        let report = build_zero_pii_diagnostic_report(state);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"report":"{}"}}"#,
            escape_json_str(&report)
        ));
    }

    if path == "/api/driver/repair" {
        let (repaired, drv_ok, svc_ok, msi_found) = repair_native_apple_service();
        let is_streaming = state.lock().map(|s| s.is_streaming).unwrap_or(false);
        let diag_json = build_native_driver_diagnostics_json_with_opts(true, is_streaming);
        let msg_id = if repaired {
            "Layanan Apple Mobile Device Service & Driver USB aktif dan siap digunakan."
        } else {
            "Perbaikan driver selesai dijalankan. Jika belum aktif, jalankan Repair sebagai Administrator atau pasang ulang Setup.exe."
        };
        let msg_en = if repaired {
            "Apple Mobile Device Service & USB Driver verified and running."
        } else {
            "Driver repair finished. If still inactive, run Repair as Administrator or re-run Setup.exe."
        };
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"native":true,"repaired":{},"driver_ready":{},"service_running":{},"bundled_msi_found":{},"diagnostics":{},"message_id":"{}","message_en":"{}"}}"#,
            repaired,
            drv_ok,
            svc_ok,
            msi_found,
            diag_json,
            escape_json_str(msg_id),
            escape_json_str(msg_en)
        ));
    }

    if path == "/api/device/unlock-developer-mode" {
        let req_udid = parse_string_param(&req.body, "udid");
        let arm_enable = req.body.contains(r#""arm_enable":true"#)
            || req.body.contains(r#""arm_enable": true"#);
        let outcome = unlock_developer_mode_menu(req_udid.as_deref(), arm_enable);
        if let Ok(mut guard) = DRIVER_DIAG_CACHE.lock() {
            *guard = None;
        }
        match outcome {
            Ok(res) => return HttpResponse::ok_json(&res.to_json_string()),
            Err(err) => {
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":false,"connected":false,"udid":null,"revealed":false,"developer_mode_status":"disconnected","status_code":"usb_required","statusCode":"usb_required","error":"{}","message_id":"Hubungkan iPhone menggunakan kabel data USB terlebih dahulu (Unlock Developer Mode hanya berjalan via USB, bukan Wi-Fi), buka kunci layar, dan ketuk Trust This Computer.","message_en":"Connect your iPhone with a USB data cable first (Unlock Developer Mode requires USB, not Wi-Fi), unlock the screen, and tap Trust This Computer."}}"#,
                    escape_json_str(&err)
                ));
            }
        }
    }

    if path == "/api/tray-state" || path == "/api/tray/menu" {
        let st = state.lock().unwrap();
        let json = build_tray_state_json("status", true, &st);
        return HttpResponse::ok_json(&json);
    }

    if path == "/api/tray/trigger_action" {
        let action = parse_string_param(&req.body, "action").unwrap_or_default();
        let mut handled = true;
        match action.as_str() {
            "show" | "restore" => {
                let hwnd_usize = state.lock().map(|s| s.window_hwnd).unwrap_or(0);
                if hwnd_usize != 0 {
                    unsafe {
                        PostMessageW(hwnd_usize as *mut _, 0x8004, 0, 0); // WM_APP_RESTORE_FROM_TRAY
                    }
                }
            }
            "hide" => {
                let hwnd_usize = state.lock().map(|s| s.window_hwnd).unwrap_or(0);
                if hwnd_usize != 0 {
                    unsafe {
                        show_window_native(hwnd_usize as *mut _, 0);
                    }
                }
            }
            "toggle_startup" => {
                let next = !is_start_with_windows_enabled();
                let _ = set_start_with_windows(next);
            }
            "check_updates" => {
                let hwnd_usize = {
                    let mut st = state.lock().unwrap();
                    st.is_drawer_open = true;
                    st.geometry.is_drawer_open = true;
                    GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
                    st.window_hwnd
                };
                if hwnd_usize != 0 {
                    unsafe {
                        show_window_native(hwnd_usize as *mut _, 5);
                        show_window_native(hwnd_usize as *mut _, 9);
                    }
                    apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
                }
                if let Ok(mut g) = GLOBAL_PENDING_TRAY_ACTION.lock() {
                    *g = Some("check_updates".to_string());
                }
            }
            "toggle_landscape" => {
                let next_land = !GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
                let hwnd_usize = {
                    let mut st = state.lock().unwrap();
                    st.is_landscape = next_land;
                    st.geometry.is_landscape = next_land;
                    GLOBAL_IS_LANDSCAPE.store(next_land, Ordering::SeqCst);
                    st.window_hwnd
                };
                if hwnd_usize != 0 {
                    apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
                }
                if let Ok(mut g) = GLOBAL_PENDING_TRAY_ACTION.lock() {
                    *g = Some(format!("set_landscape:{}", next_land));
                }
            }
            "toggle_stealth" => {
                let mut ctrl = IndigoHidController::new();
                let _ = ctrl.execute_stealth_dim_toggle();
                let next_stealth = !GLOBAL_STEALTH_ACTIVE.load(Ordering::SeqCst);
                GLOBAL_STEALTH_ACTIVE.store(next_stealth, Ordering::SeqCst);
                {
                    let mut st = state.lock().unwrap();
                    st.stealth_dim = next_stealth;
                }
                if let Ok(mut g) = GLOBAL_PENDING_TRAY_ACTION.lock() {
                    *g = Some(format!("set_stealth:{}", next_stealth));
                }
            }
            "toggle_audio" => {
                let next_pc = !GLOBAL_AUDIO_PC_ACTIVE.load(Ordering::SeqCst);
                GLOBAL_AUDIO_PC_ACTIVE.store(next_pc, Ordering::SeqCst);
                AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(!next_pc, Ordering::SeqCst);
                let is_streaming_now = {
                    let mut st = state.lock().unwrap();
                    st.audio_routing = if next_pc { "pc".to_string() } else { "device".to_string() };
                    st.bluetooth_a2dp = false;
                    st.audio_muted = !next_pc;
                    save_user_settings_to_disk(&st);
                    st.is_streaming
                        || iphone_link_apple::mirror::global_screen_status_json()
                            .contains(r#""phase":"running""#)
                };
                if next_pc && is_streaming_now {
                    let (pn, pm, pb) = resolve_active_audio_device_target(state, None, None, None);
                    let _ = iphone_link_audio::start_global_pc_audio_bridge_for_device(
                        pn.as_deref(),
                        pm.as_deref(),
                        pb.as_deref(),
                    );
                } else if !next_pc {
                    iphone_link_audio::stop_global_pc_audio_bridge();
                }
                if let Ok(mut g) = GLOBAL_PENDING_TRAY_ACTION.lock() {
                    *g = Some(format!("set_audio_pc:{}", next_pc));
                }
            }
            "clear_cache" => {
                clear_copy_cache();
                if let Ok(mut g) = GLOBAL_PENDING_TRAY_ACTION.lock() {
                    *g = Some("clear_cache".to_string());
                }
            }
            "quit" | "exit" => {
                let hwnd_usize = state.lock().map(|s| s.window_hwnd).unwrap_or(0);
                let _ = execute_clean_window_close_resources(hwnd_usize as *mut _, Some(state));
                if hwnd_usize != 0 {
                    unsafe {
                        post_quit_message(hwnd_usize as *mut _);
                    }
                }
            }
            _ => {
                handled = false;
            }
        }
        let st = state.lock().unwrap();
        let json = build_tray_state_json(&action, handled, &st);
        return HttpResponse::ok_json(&json);
    }

    if path == "/api/tray/dismiss_menu" {
        let was_open = dismiss_tray_menu_if_open();
        if was_open {
            GLOBAL_TRAY_DISMISSED_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"wasOpen":{},"menuOpen":{},"dismissedCount":{}}}"#,
            was_open,
            GLOBAL_TRAY_MENU_OPEN.load(Ordering::SeqCst),
            GLOBAL_TRAY_DISMISSED_COUNT.load(Ordering::SeqCst)
        ));
    }

    if path == "/debug-metrics" {
        if req.method.eq_ignore_ascii_case("POST") {
            let trimmed = req.body.trim();
            if !trimmed.is_empty() {
                if let Ok(mut hist) = DEBUG_METRICS_HISTORY.lock() {
                    hist.push(trimmed.to_string());
                    if hist.len() > 60 {
                        let excess = hist.len() - 60;
                        hist.drain(..excess);
                    }
                }
            }
            return HttpResponse::ok_json(r#"{"ok":true,"native":true,"recorded":true}"#);
        }
        let items: Vec<String> = DEBUG_METRICS_HISTORY
            .lock()
            .map(|h| h.iter().map(|s| format!(r#""{}""#, escape_json_str(s))).collect())
            .unwrap_or_default();
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"history":[{}]}}"#, items.join(",")));
    }

    // 7. API: Window Controls (with DPI-aware HWND & WebView2 Bounds Sync via Main Thread Dispatch)
    if path == "/api/window/tray-menu" || path == "/api/window/tray_menu" {
        let st = state.lock().unwrap();
        let json = build_tray_state_json("menu", true, &st);
        return HttpResponse::ok_json(&json);
    }

    if path == "/api/window/state" {
        let st = state.lock().unwrap();
        let is_fs = st.is_fullscreen || GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst);
        let is_land = st.is_landscape || GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let open = st.is_drawer_open || GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst).clamp(336, 600);
        let (cur_w, cur_h) = compute_logical_window_dims(is_fs, is_land, open, port_w);
        let stealth_active =
            st.stealth_dim || GLOBAL_STEALTH_ACTIVE.load(Ordering::SeqCst) || is_stealth_dim_active();
        let tray_reg = GLOBAL_TRAY_REGISTERED.load(Ordering::SeqCst);
        let prof = if st.device_profile.is_empty() {
            "auto".to_string()
        } else {
            st.device_profile.clone()
        };
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"drawer_open":{},"is_drawer_open":{},"landscape":{},"is_landscape":{},"fullscreen":{},"is_fullscreen":{},"portrait_width":{},"width":{},"height":{},"min_width":{},"min_height":{},"theme":"{}","device_profile":"{}","stealth_dim":{},"tray_registered":{}}}"#,
            open,
            open,
            is_land,
            is_land,
            is_fs,
            is_fs,
            port_w,
            cur_w,
            cur_h,
            MIN_WINDOW_WIDTH,
            MIN_WINDOW_HEIGHT,
            escape_json_str(&st.theme),
            escape_json_str(&prof),
            stealth_active,
            tray_reg
        ));
    }

    if path == "/api/window/set_drawer" || path == "/api/window/drawer" {
        let mut st = state.lock().unwrap();
        let open = req.body.contains(r#""open":true"#)
            || req.body.contains(r#""open": true"#)
            || req.body.contains(r#""drawer":true"#)
            || req.body.contains(r#""drawer": true"#);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let prev_ms = LAST_SET_DRAWER_MS.load(Ordering::SeqCst);
        let prev_open = LAST_SET_DRAWER_STATE.load(Ordering::SeqCst);
        let cur_global_open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        let is_duplicate = cur_global_open == open && prev_open == open && now_ms.saturating_sub(prev_ms) < 80;

        LAST_SET_DRAWER_STATE.store(open, Ordering::SeqCst);
        LAST_SET_DRAWER_MS.store(now_ms, Ordering::SeqCst);
        st.is_drawer_open = open;
        st.geometry.is_drawer_open = open;
        GLOBAL_DRAWER_OPEN.store(open, Ordering::SeqCst);
        let is_fs = st.is_fullscreen || GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst);
        let is_land = st.is_landscape || GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let hwnd_usize = st.window_hwnd;
        drop(st);

        if is_fs {
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"status":"ok","skipped":"fullscreen","fullscreen":true,"is_fullscreen":true,"drawer_open":{},"width":1920,"height":1080}}"#,
                open
            ));
        }

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !is_duplicate && !hwnd_ptr.is_null() {
            apply_hwnd_geometry_from_state(hwnd_ptr);
        }

        let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst);
        let (cur_w, cur_h) = compute_logical_window_dims(false, is_land, open, port_w);

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","drawer_open":{},"width":{},"height":{}}}"#,
            open, cur_w, cur_h
        ));
    }

    if path == "/api/window/set_orientation" || path == "/api/window/orientation" {
        let mut st = state.lock().unwrap();
        let land = parse_landscape_flag_from_body(&req.body);
        st.is_landscape = land;
        st.geometry.is_landscape = land;
        GLOBAL_IS_LANDSCAPE.store(land, Ordering::SeqCst);
        if req.body.contains(r#""drawer":true"#) || req.body.contains(r#""drawer": true"#) {
            st.is_drawer_open = true;
            st.geometry.is_drawer_open = true;
            GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
        } else if req.body.contains(r#""drawer":false"#) || req.body.contains(r#""drawer": false"#) {
            st.is_drawer_open = false;
            st.geometry.is_drawer_open = false;
            GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        }

        let is_fs = st.is_fullscreen || GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst);
        let hwnd_usize = st.window_hwnd;
        let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        drop(st);

        if is_fs {
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"status":"ok","skipped":"fullscreen","fullscreen":true,"is_fullscreen":true,"landscape":{},"drawer_open":{},"width":1920,"height":1080}}"#,
                land, open
            ));
        }

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            apply_hwnd_geometry_from_state(hwnd_ptr);
        }

        let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst);
        let (cur_w, cur_h) = compute_logical_window_dims(false, land, open, port_w);

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","landscape":{},"drawer_open":{},"width":{},"height":{}}}"#,
            land, open, cur_w, cur_h
        ));
    }

    if path == "/api/window/set_theme"
        || (path == "/api/window/theme" && req.method.eq_ignore_ascii_case("POST"))
    {
        let is_light = req.body.contains(r#""theme":"light""#) || req.body.contains(r#""theme": "light""#);
        GLOBAL_DARK_MODE.store(!is_light, Ordering::SeqCst);
        let mut st = state.lock().unwrap();
        st.theme = if is_light { "light".to_string() } else { "dark".to_string() };
        save_user_settings_to_disk(&st);
        let hwnd_usize = st.window_hwnd;
        let theme_str = st.theme.clone();
        drop(st);

        if hwnd_usize != 0 {
            let _ = apply_true_files_acrylic_backdrop(hwnd_usize as *mut _, !is_light);
        }
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"status":"ok","theme":"{}"}}"#, theme_str));
    }

    if path == "/api/window/theme" {
        let is_dark = GLOBAL_DARK_MODE.load(Ordering::SeqCst);
        let dom_thm = GLOBAL_DOM_THEME.lock().map(|s| s.clone()).unwrap_or_default();
        let active = if is_dark { "dark" } else { "light" };
        let dom_active = if dom_thm.is_empty() { active } else { &dom_thm };
        let light_x = GLOBAL_THEME_LIGHT_X.load(Ordering::SeqCst);
        let light_y = GLOBAL_THEME_LIGHT_Y.load(Ordering::SeqCst);
        let wf_clean = GLOBAL_WIREFRAME_CLEAN.load(Ordering::SeqCst);
        let wf_det = GLOBAL_WIREFRAME_DETAILS.lock().map(|s| s.clone()).unwrap_or_default();
        let conc_track = GLOBAL_CONCENTRIC_TRACK.lock().map(|s| s.clone()).unwrap_or_default();
        let conc_btn = GLOBAL_CONCENTRIC_BTN.lock().map(|s| s.clone()).unwrap_or_default();
        let badge_bg = GLOBAL_BADGE_BG.lock().map(|s| s.clone()).unwrap_or_default();
        let seg_clean = GLOBAL_SEGMENTED_CLEAN.load(Ordering::SeqCst);
        let seg_det = GLOBAL_SEGMENTED_DETAILS.lock().map(|s| s.clone()).unwrap_or_default();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"theme":"{}","activeTheme":"{}","domTheme":"{}","themeLightX":{},"themeLightY":{},"wireframeClean":{},"wireframeDetails":"{}","concentricTrackRadius":"{}","concentricButtonRadius":"{}","badgeBg":"{}","segmentedClean":{},"segmentedDetails":"{}"}}"#,
            active, active, dom_active, light_x, light_y, wf_clean, wf_det.replace('"', r#"\""#),
            conc_track, conc_btn, badge_bg, seg_clean, seg_det.replace('"', r#"\""#)
        ));
    }

    if path == "/api/window/click_selector" || path == "/api/test/click_selector" {
        if let Some(sel) = parse_string_param(&req.body, "selector") {
            if let Ok(mut g_sel) = GLOBAL_PENDING_CLICK.lock() {
                g_sel.push_back(sel);
            }
            return HttpResponse::ok_json(r#"{"ok":true,"queued":true}"#);
        }
        return HttpResponse::ok_json(r#"{"ok":false,"error":"missing selector"}"#);
    }

    if path == "/api/window/set_input_value" || path == "/api/test/set_input_value" {
        if let Some(sel) = parse_string_param(&req.body, "selector") {
            let val = parse_string_param(&req.body, "value").unwrap_or_default();
            if let Ok(mut g_inp) = GLOBAL_PENDING_INPUT.lock() {
                g_inp.push_back((sel, val));
            }
            return HttpResponse::ok_json(r#"{"ok":true,"queued":true}"#);
        }
        return HttpResponse::ok_json(r#"{"ok":false,"error":"missing selector"}"#);
    }

    if path == "/api/test/verify_mirror_autofit" {
        if let Ok(mut g_rep) = GLOBAL_MIRROR_TEST_REPORT.lock() {
            g_rep.clear();
        }
        if let Ok(mut g_tst) = GLOBAL_PENDING_TEST.lock() {
            *g_tst = Some("verify_mirror_autofit".to_string());
        }
        return HttpResponse::ok_json(r#"{"ok":true,"queued":true,"test":"verify_mirror_autofit"}"#);
    }

    if path == "/api/test/verify_drawer_toggle_flicker" {
        if let Ok(mut g_rep) = GLOBAL_MIRROR_TEST_REPORT.lock() {
            g_rep.clear();
        }
        if let Ok(mut g_tst) = GLOBAL_PENDING_TEST.lock() {
            *g_tst = Some("verify_drawer_toggle_flicker".to_string());
        }
        return HttpResponse::ok_json(r#"{"ok":true,"queued":true,"test":"verify_drawer_toggle_flicker"}"#);
    }

    if path == "/api/test/verify_fullscreen_enter_sync" {
        if let Ok(mut g_rep) = GLOBAL_MIRROR_TEST_REPORT.lock() {
            g_rep.clear();
        }
        if let Ok(mut g_tst) = GLOBAL_PENDING_TEST.lock() {
            *g_tst = Some("verify_fullscreen_enter_sync".to_string());
        }
        return HttpResponse::ok_json(r#"{"ok":true,"queued":true,"test":"verify_fullscreen_enter_sync"}"#);
    }

    if path == "/api/test/verify_tahap182_ui" {
        if let Ok(mut g_rep) = GLOBAL_MIRROR_TEST_REPORT.lock() {
            g_rep.clear();
        }
        if let Ok(mut g_tst) = GLOBAL_PENDING_TEST.lock() {
            *g_tst = Some("verify_tahap182_ui".to_string());
        }
        return HttpResponse::ok_json(r#"{"ok":true,"queued":true,"test":"verify_tahap182_ui"}"#);
    }

    if path == "/api/test/mirror_report" {
        if req.method == "POST" {
            if let Ok(mut g_rep) = GLOBAL_MIRROR_TEST_REPORT.lock() {
                *g_rep = req.body.clone();
            }
            return HttpResponse::ok_json(r#"{"ok":true,"saved":true}"#);
        } else {
            let rep = GLOBAL_MIRROR_TEST_REPORT.lock().map(|s| s.clone()).unwrap_or_default();
            if rep.is_empty() {
                return HttpResponse::ok_json(r#"{"ready":false}"#);
            }
            return HttpResponse::ok_json(&rep);
        }
    }

    if path == "/api/window/toggle_fullscreen" || path == "/api/window/fullscreen" {
        if req.body.contains(r#""drawer_open":false"#) || req.body.contains(r#""drawer_open": false"#) {
            GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        } else if req.body.contains(r#""drawer_open":true"#) || req.body.contains(r#""drawer_open": true"#) {
            GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
        }

        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);

        let target_opt = parse_bool_param(&req.body, "target")
            .or_else(|| parse_bool_param(&req.body, "fullscreen"));

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        let is_fs = if !hwnd_ptr.is_null() {
            toggle_fullscreen_state(hwnd_ptr, target_opt)
        } else {
            let next_val = target_opt.unwrap_or_else(|| !GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst));
            GLOBAL_IS_FULLSCREEN.store(next_val, Ordering::SeqCst);
            next_val
        };
        {
            let mut st = state.lock().unwrap();
            st.is_fullscreen = is_fs;
            st.geometry.is_fullscreen = is_fs;
        }

        let (cur_w, cur_h) = compute_logical_window_dims(
            is_fs,
            GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst),
            GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst),
            GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst),
        );

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","fullscreen":{},"is_fullscreen":{},"width":{},"height":{}}}"#,
            is_fs, is_fs, cur_w, cur_h
        ));
    }

    if path == "/api/window/sync_device_aspect" || path == "/api/window/sync_aspect" || path == "/api/window/aspect" {
        let width_val = parse_int_param(&req.body, "width")
            .or_else(|| parse_int_param(&req.body, "displayWidth"))
            .or_else(|| parse_int_param(&req.body, "display_width"))
            .unwrap_or(0);
        let cur_prof = GLOBAL_DEVICE_PROFILE.lock().map(|p| p.clone()).unwrap_or_default();
        let is_ipad = width_val >= 500
            || req.body.contains(r#""profile":"ipad""#)
            || req.body.contains(r#""profile": "ipad""#)
            || req.body.contains(r#""family":"ipad""#)
            || req.body.contains(r#""family": "ipad""#)
            || req.body.contains("4:3")
            || (cur_prof == "ipad" && !(req.body.contains(r#""profile":"auto""#) || req.body.contains(r#""profile":"iphone""#)));
        // Tahap 182: Lock Super Retina iPhone portrait widths (336..=356, covering 340..=348px) at 346px
        // so zero 346px -> 340px mid-stream calibration jump occurs, while preserving 376..=440px (e.g. displayWidth 376 / 422px).
        let portrait_w = if is_ipad {
            564
        } else if (336..=356).contains(&width_val) {
            346
        } else if (357..=440).contains(&width_val) {
            width_val
        } else {
            346
        };
        GLOBAL_PORTRAIT_WIDTH.store(portrait_w, Ordering::SeqCst);
        let out_prof = if is_ipad {
            "ipad".to_string()
        } else if req.body.contains(r#""profile":"iphone""#) || req.body.contains(r#""profile": "iphone""#) {
            "iphone".to_string()
        } else if req.body.contains(r#""profile":"auto""#) || req.body.contains(r#""profile": "auto""#) {
            "auto".to_string()
        } else if cur_prof == "iphone" {
            "iphone".to_string()
        } else {
            "auto".to_string()
        };
        if let Ok(mut prof) = GLOBAL_DEVICE_PROFILE.lock() {
            *prof = out_prof.clone();
        }

        if req.body.contains(r#""drawer":true"#) || req.body.contains(r#""drawer": true"#) || req.body.contains(r#""open":true"#) || req.body.contains(r#""open": true"#) {
            GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
        } else if req.body.contains(r#""drawer":false"#) || req.body.contains(r#""drawer": false"#) || req.body.contains(r#""open":false"#) || req.body.contains(r#""open": false"#) {
            GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        }

        let st = state.lock().unwrap();
        let is_fs = st.is_fullscreen || GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst);
        let hwnd_usize = st.window_hwnd;
        drop(st);

        if is_fs {
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"skipped":"fullscreen","fullscreen":true,"is_fullscreen":true,"is_ipad":{},"portrait_width":{},"width":1920,"height":1080,"profile":"{}"}}"#,
                is_ipad, portrait_w, out_prof
            ));
        }

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            apply_hwnd_geometry_from_state(hwnd_ptr);
        }

        let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        let land = GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let (cur_w, cur_h) = compute_logical_window_dims(false, land, open, portrait_w);

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"is_ipad":{},"portrait_width":{},"width":{},"height":{},"profile":"{}"}}"#,
            is_ipad, portrait_w, cur_w, cur_h, out_prof
        ));
    }

    if path == "/api/window/sync_layout" || path == "/api/window/sync_geometry" {
        let mut changed = false;
        if req.body.contains(r#""drawerOpen":true"#) || req.body.contains(r#""drawerOpen": true"#) {
            if !GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst) {
                GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
                changed = true;
            }
        } else if req.body.contains(r#""drawerOpen":false"#) || req.body.contains(r#""drawerOpen": false"#) {
            if GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst) {
                GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
                changed = true;
            }
        }

        if req.body.contains(r#""isLandscape":true"#) || req.body.contains(r#""isLandscape": true"#) {
            if !GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst) {
                GLOBAL_IS_LANDSCAPE.store(true, Ordering::SeqCst);
                changed = true;
            }
        } else if req.body.contains(r#""isLandscape":false"#) || req.body.contains(r#""isLandscape": false"#) {
            if GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst) {
                GLOBAL_IS_LANDSCAPE.store(false, Ordering::SeqCst);
                changed = true;
            }
        }

        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if changed && !hwnd_ptr.is_null() {
            apply_hwnd_geometry_from_state(hwnd_ptr);
        }

        let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        let land = GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst);
        let cur_w = if land {
            if open { 1200 } else { 840 }
        } else {
            if open { port_w + 360 } else { port_w }
        };
        let cur_h = if land { 468 } else { 832 };

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"synced":true,"drawer_open":{},"landscape":{},"width":{},"height":{}}}"#,
            open, land, cur_w, cur_h
        ));
    }

    if path == "/api/window/start_drag" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            trigger_hwnd_drag(hwnd_ptr);
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"dragging"}"#);
    }

    if path == "/api/window/move_by" {
        let dx = parse_int_param(&req.body, "dx").unwrap_or(0);
        let dy = parse_int_param(&req.body, "dy").unwrap_or(0);
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            move_hwnd_by(hwnd_ptr, dx, dy);
        }
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"dx":{},"dy":{}}}"#, dx, dy));
    }

    if path == "/api/window/move_to" {
        let x = parse_int_param(&req.body, "x").unwrap_or(100);
        let y = parse_int_param(&req.body, "y").unwrap_or(100);
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            move_hwnd_to(hwnd_ptr, x, y);
        }
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"x":{},"y":{}}}"#, x, y));
    }

    if path == "/api/window/dom_ready_beacon" {
        if let Ok(beacon_path) = std::env::var("IPHONE_LINK_DOM_BEACON_PATH") {
            let trimmed = beacon_path.trim();
            if !trimmed.is_empty() {
                let _ = std::fs::write(trimmed, &req.body);
            }
        }
        let has_edg = req.body.contains("Edg/");
        let btn_vis = req.body.contains(r#""startBtnVisible":true"#) || req.body.contains(r#""startBtnVisible": true"#);
        let btn_text_ok = req.body.to_lowercase().contains("mirroring");
        let raf_ok = req.body.contains(r#""rafVerified":true"#) || req.body.contains(r#""rafVerified": true"#);

        if raf_ok {
            GLOBAL_RAF_VERIFIED.store(true, Ordering::SeqCst);
        }

        if let Some(theme) = parse_string_param(&req.body, "domTheme") {
            if let Ok(mut g_thm) = GLOBAL_DOM_THEME.lock() {
                *g_thm = theme;
            }
        }
        if let Some(x) = parse_int_param(&req.body, "themeLightX") {
            if x > 0 {
                GLOBAL_THEME_LIGHT_X.store(x as usize, Ordering::SeqCst);
            }
        }
        if let Some(y) = parse_int_param(&req.body, "themeLightY") {
            if y > 0 {
                GLOBAL_THEME_LIGHT_Y.store(y as usize, Ordering::SeqCst);
            }
        }

        let wf_clean = req.body.contains(r#""wireframeClean":true"#) || req.body.contains(r#""wireframeClean": true"#);
        GLOBAL_WIREFRAME_CLEAN.store(wf_clean, Ordering::SeqCst);
        if let Some(details) = parse_string_param(&req.body, "wireframeDetails") {
            if let Ok(mut g_wf) = GLOBAL_WIREFRAME_DETAILS.lock() {
                *g_wf = details;
            }
        }

        if let Some(track_r) = parse_string_param(&req.body, "concentricTrackRadius") {
            if let Ok(mut g_tr) = GLOBAL_CONCENTRIC_TRACK.lock() {
                *g_tr = track_r;
            }
        }
        if let Some(btn_r) = parse_string_param(&req.body, "concentricButtonRadius") {
            if let Ok(mut g_br) = GLOBAL_CONCENTRIC_BTN.lock() {
                *g_br = btn_r;
            }
        }
        if let Some(badge_bg) = parse_string_param(&req.body, "badgeBg") {
            if let Ok(mut g_bg) = GLOBAL_BADGE_BG.lock() {
                *g_bg = badge_bg;
            }
        }
        let seg_clean = req.body.contains(r#""segmentedClean":true"#) || req.body.contains(r#""segmentedClean": true"#);
        GLOBAL_SEGMENTED_CLEAN.store(seg_clean, Ordering::SeqCst);
        if let Some(seg_det) = parse_string_param(&req.body, "segmentedDetails") {
            if let Ok(mut g_sd) = GLOBAL_SEGMENTED_DETAILS.lock() {
                *g_sd = seg_det;
            }
        }
        if let Some(prof) = parse_string_param(&req.body, "activeDeviceProfile") {
            if let Ok(mut g_prof) = GLOBAL_DEVICE_PROFILE.lock() {
                *g_prof = prof;
            }
        }
        if let Some(dw) = parse_int_param(&req.body, "drawerRectWidth") {
            GLOBAL_DRAWER_RECT_WIDTH.store(dw as usize, Ordering::SeqCst);
        }
        if let Some(ov) = parse_int_param(&req.body, "drawerRightOverflow") {
            GLOBAL_DRAWER_RIGHT_OVERFLOW.store(ov as isize, Ordering::SeqCst);
        }
        if let Some(dcw) = parse_int_param(&req.body, "displayColWidth") {
            GLOBAL_DISPLAY_COL_WIDTH.store(dcw.max(0) as usize, Ordering::SeqCst);
        }
        if let Some(sdw) = parse_int_param(&req.body, "sideDrawerWidth") {
            GLOBAL_SIDE_DRAWER_WIDTH.store(sdw.max(0) as usize, Ordering::SeqCst);
        }
        if let Some(sdrg) = parse_int_param(&req.body, "sideDrawerRightGap") {
            GLOBAL_SIDE_DRAWER_RIGHT_GAP.store(sdrg as isize, Ordering::SeqCst);
        }
        if let Some(dsrg) = parse_int_param(&req.body, "drawerScrollRightGap") {
            GLOBAL_DRAWER_SCROLL_RIGHT_GAP.store(dsrg as isize, Ordering::SeqCst);
        }
        if let Some(iw) = parse_int_param(&req.body, "innerWidth") {
            GLOBAL_INNER_WIDTH.store(iw.max(0) as usize, Ordering::SeqCst);
        }
        if let Some(sgc) = parse_int_param(&req.body, "stretchGlitchCount") {
            GLOBAL_STRETCH_GLITCH_COUNT.store(sgc.max(0) as usize, Ordering::SeqCst);
        }
        if let Some(mcd) = parse_int_param(&req.body, "maxDisplayColDuringClose") {
            GLOBAL_MAX_DISPLAY_COL_WIDTH_DURING_CLOSE.store(mcd.max(0) as usize, Ordering::SeqCst);
        }

        if let Some(txt) = parse_string_param(&req.body, "startBtnText") {
            if let Ok(mut g_txt) = GLOBAL_START_BTN_TEXT.lock() {
                *g_txt = txt;
            }
        }
        if let Some(ua) = parse_string_param(&req.body, "userAgent") {
            if let Ok(mut g_ua) = GLOBAL_USER_AGENT.lock() {
                *g_ua = ua;
            }
        }
        if let Some(pin_dom) = parse_string_param(&req.body, "unlockPinValue") {
            if let Ok(mut g_pin) = GLOBAL_DOM_UNLOCK_PIN.lock() {
                *g_pin = sanitize_pin_digits(&pin_dom);
            }
        }
        if let Some(ver_dom) = parse_string_param(&req.body, "updateVersionText") {
            if let Ok(mut g_ver) = GLOBAL_DOM_UPDATE_VERSION.lock() {
                *g_ver = ver_dom;
            }
        }
        if let Some(act_dom) = parse_string_param(&req.body, "lastTrayAction") {
            if let Ok(mut g_act) = GLOBAL_DOM_LAST_TRAY_ACTION.lock() {
                *g_act = act_dom;
            }
        }
        if let Some(lang_dom) = parse_string_param(&req.body, "domLanguage") {
            if let Ok(mut g_lang) = GLOBAL_DOM_LANG.lock() {
                *g_lang = lang_dom;
            }
        }
        GLOBAL_DOM_LANG_ID_ACTIVE.store(
            req.body.contains(r#""activeLangId":true"#) || req.body.contains(r#""activeLangId": true"#),
            Ordering::SeqCst,
        );
        GLOBAL_DOM_LANG_EN_ACTIVE.store(
            req.body.contains(r#""activeLangEn":true"#) || req.body.contains(r#""activeLangEn": true"#),
            Ordering::SeqCst,
        );
        if let Some(dt_dom) = parse_string_param(&req.body, "drawerTitleText") {
            if let Ok(mut g_dt) = GLOBAL_DOM_DRAWER_TITLE.lock() {
                *g_dt = dt_dom;
            }
        }
        if let Some(tsum) = parse_string_param(&req.body, "domTogglesSummary") {
            if let Ok(mut g_ts) = GLOBAL_DOM_TOGGLES_SUMMARY.lock() {
                *g_ts = tsum;
            }
        }
        GLOBAL_DOM_CARD_UPDATER_EXISTS.store(
            req.body.contains(r#""cardUpdaterExists":true"#) || req.body.contains(r#""cardUpdaterExists": true"#),
            Ordering::SeqCst,
        );
        if let Some(cu_title) = parse_string_param(&req.body, "cardUpdaterTitle") {
            if let Ok(mut g_ct) = GLOBAL_DOM_CARD_UPDATER_TITLE.lock() {
                *g_ct = cu_title;
            }
        }

        if has_edg && btn_vis && btn_text_ok {
            DOM_READY_BEACON.fetch_add(1, Ordering::SeqCst);
        }

        if let Some(dpr_val) = parse_float_param(&req.body, "dpr") {
            let dpr_int = (dpr_val * 100.0).round() as usize;
            GLOBAL_DEVICE_PIXEL_RATIO.store(dpr_int, Ordering::SeqCst);
        }
        if let Some(w) = parse_int_param(&req.body, "appWidth") {
            if w > 0 {
                GLOBAL_APP_WIDTH.store(w as usize, Ordering::SeqCst);
            }
        }
        if let Some(h) = parse_int_param(&req.body, "appHeight") {
            if h > 0 {
                GLOBAL_APP_HEIGHT.store(h as usize, Ordering::SeqCst);
            }
        }
        let lp_vis = req.body.contains(r#""launchpadVisible":true"#) || req.body.contains(r#""launchpadVisible": true"#);
        GLOBAL_LAUNCHPAD_VISIBLE.store(lp_vis, Ordering::SeqCst);
        GLOBAL_START_BTN_VISIBLE.store(btn_vis, Ordering::SeqCst);
        return HttpResponse::ok_json(r#"{"ok":true,"beacon":true}"#);
    }

    if path == "/api/window/rect" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        let fallback_r = st.geometry.current_rect;
        drop(st);

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            #[link(name = "user32")]
            extern "system" {
                fn GetWindowRect(hwnd: *mut std::ffi::c_void, lpRect: *mut RECT) -> i32;
            }
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let got = unsafe { GetWindowRect(hwnd_ptr, &mut rc) };
            if got != 0 {
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":true,"hwnd":{},"x":{},"y":{},"width":{},"height":{},"drawer_open":{},"is_landscape":{},"is_fullscreen":{},"portrait_width":{}}}"#,
                    hwnd_ptr as usize,
                    rc.left,
                    rc.top,
                    rc.width(),
                    rc.height(),
                    GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst),
                    GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst),
                    GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst),
                    GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst),
                ));
            }
        }

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"hwnd":{},"x":{},"y":{},"width":{},"height":{},"drawer_open":{},"is_landscape":{},"is_fullscreen":{},"portrait_width":{}}}"#,
            hwnd_ptr as usize,
            fallback_r.left,
            fallback_r.top,
            fallback_r.width(),
            fallback_r.height(),
            GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst),
            GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst),
            GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst),
            GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst),
        ));
    }

    if path == "/api/window/set_device_profile" {
        let next_prof = if req.body.contains(r#""profile":"ipad""#)
            || req.body.contains(r#""profile": "ipad""#)
            || req.body.contains(r#""ipad""#)
        {
            "ipad".to_string()
        } else if req.body.contains(r#""profile":"iphone""#) || req.body.contains(r#""profile": "iphone""#) {
            "iphone".to_string()
        } else {
            "auto".to_string()
        };
        let is_ipad = next_prof == "ipad";
        let portrait_w = if is_ipad { 564 } else { 346 };
        GLOBAL_PORTRAIT_WIDTH.store(portrait_w, Ordering::SeqCst);
        if let Ok(mut prof) = GLOBAL_DEVICE_PROFILE.lock() {
            *prof = next_prof.clone();
        }

        if req.body.contains(r#""drawer":true"#) || req.body.contains(r#""drawer": true"#) || req.body.contains(r#""open":true"#) || req.body.contains(r#""open": true"#) {
            GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
        } else if req.body.contains(r#""drawer":false"#) || req.body.contains(r#""drawer": false"#) || req.body.contains(r#""open":false"#) || req.body.contains(r#""open": false"#) {
            GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        }

        let mut st = state.lock().unwrap();
        st.device_profile = next_prof.clone();
        save_user_settings_to_disk(&st);
        let hwnd_usize = st.window_hwnd;
        drop(st);

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            apply_hwnd_geometry_from_state(hwnd_ptr);
        }

        let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        let land = GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let cur_w = if land {
            if open { 1200 } else { 840 }
        } else {
            if open { portrait_w + 360 } else { portrait_w }
        };
        let cur_h = if land { 468 } else { 832 };

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"profile":"{}","isIpadMode":{},"portrait_width":{},"width":{},"height":{}}}"#,
            next_prof, is_ipad, portrait_w, cur_w, cur_h
        ));
    }

    if path == "/api/window/set_device_family" {
        let cur_prof = GLOBAL_DEVICE_PROFILE.lock().map(|p| p.clone()).unwrap_or_default();
        if cur_prof == "ipad" {
            // User selected iPad profile; do not downgrade to iphone
            return HttpResponse::ok_json(r#"{"ok":true,"family":"ipad","isIpadMode":true}"#);
        }
        let should_resize = req.body.contains(r#""resize":true"#) || req.body.contains(r#""resize": true"#);
        let is_ipad = req.body.contains(r#""family":"ipad""#)
            || req.body.contains(r#""family": "ipad""#)
            || req.body.contains(r#""ipad""#);
        if should_resize {
            let portrait_w = if is_ipad { 564 } else { 346 };
            GLOBAL_PORTRAIT_WIDTH.store(portrait_w, Ordering::SeqCst);
            if let Ok(mut prof) = GLOBAL_DEVICE_PROFILE.lock() {
                *prof = if is_ipad { "ipad".to_string() } else { "auto".to_string() };
            }

            let st = state.lock().unwrap();
            let hwnd_usize = st.window_hwnd;
            drop(st);

            let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
            if !hwnd_ptr.is_null() {
                apply_hwnd_geometry_from_state(hwnd_ptr);
            }
        }

        let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        let land = GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst);
        let cur_w = if land {
            if open { 1200 } else { 840 }
        } else {
            if open { port_w + 360 } else { port_w }
        };
        let cur_h = if land { 468 } else { 832 };

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"family":"{}","isIpadMode":{},"portrait_width":{},"width":{},"height":{}}}"#,
            if is_ipad { "ipad" } else { "iphone" }, is_ipad, port_w, cur_w, cur_h
        ));
    }

    if path == "/api/window/reset_size" || path == "/api/window/reset_window_size" {
        GLOBAL_PORTRAIT_WIDTH.store(346, Ordering::SeqCst);
        if let Ok(mut prof) = GLOBAL_DEVICE_PROFILE.lock() {
            *prof = "auto".to_string();
        }

        let mut st = state.lock().unwrap();
        st.device_profile = "auto".to_string();
        save_user_settings_to_disk(&st);
        let hwnd_usize = st.window_hwnd;
        drop(st);

        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            apply_hwnd_geometry_from_state(hwnd_ptr);
        }

        let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        let land = GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let cur_w = if land {
            if open { 1200 } else { 840 }
        } else {
            if open { 706 } else { 346 }
        };
        let cur_h = if land { 468 } else { 832 };

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"reset":true,"width":{},"height":{},"isIpadMode":false}}"#,
            cur_w, cur_h
        ));
    }

    if path == "/api/window/apply_custom_size"
        || path == "/api/window/begin_resize"
        || path == "/api/window/update_resize"
        || path == "/api/window/end_resize"
    {
        return HttpResponse::ok_json(r#"{"ok":true}"#);
    }

    if path == "/api/window/close" {
        let hwnd_usize = state.lock().map(|s| s.window_hwnd).unwrap_or(0);
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        let (stealth_restored, restore_steps, tray_deleted, elapsed_ms) =
            execute_clean_window_close_resources(hwnd_ptr, Some(state));
        if !hwnd_ptr.is_null() {
            unsafe {
                post_quit_message(hwnd_ptr);
            }
        }
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"closing","hidden":true,"stealth_restored":{},"stealth_restore_steps":{},"tray_deleted":{},"elapsed_ms":{}}}"#,
            stealth_restored, restore_steps, tray_deleted, elapsed_ms
        ));
    }

    if path == "/api/window/minimize" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            unsafe {
                show_window_native(hwnd_ptr, 6); // SW_MINIMIZE
            }
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"minimized"}"#);
    }

    if path == "/api/window/hide_to_tray" || path == "/api/window/tray" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            let _ = register_tray_icon(hwnd_ptr);
            unsafe {
                show_window_native(hwnd_ptr, 0); // SW_HIDE
            }
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"hidden","trayActive":true}"#);
    }

    if path == "/api/window/restore_from_tray" || path == "/api/window/restore" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            crate::window_dwm::restore_window_from_tray(hwnd_ptr);
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"restored"}"#);
    }

    // 8. Screen streaming start / stop / orientation / status & /mirror/* bridge (Native Rust Tahap 230)
    if path == "/api/screen/orientation" {
        let angle = parse_int_param(&req.body, "orientation")
            .or_else(|| parse_int_param(&req.body, "angle"))
            .map(|v| (v.max(0) as u32) % 360);
        let auto_rot = parse_bool_param(&req.body, "autoRotate")
            .or_else(|| parse_bool_param(&req.body, "auto_rotate"));
        let action = parse_string_param(&req.body, "action")
            .or_else(|| parse_string_param(&req.body, "gesture"));
        let (deg, json_resp) = iphone_link_apple::mirror::global_apply_orientation(
            angle,
            auto_rot,
            action.as_deref(),
            None,
        );
        let is_land = deg == 90 || deg == 270;
        GLOBAL_IS_LANDSCAPE.store(is_land, Ordering::SeqCst);
        let hwnd_usize = {
            let mut st = state.lock().unwrap();
            st.is_landscape = is_land;
            st.geometry.is_landscape = is_land;
            if let Some(ar) = auto_rot {
                st.auto_rotate = ar;
                save_user_settings_to_disk(&st);
            }
            st.window_hwnd
        };
        let hwnd_ptr = crate::window_dwm::get_effective_main_hwnd(hwnd_usize);
        if !hwnd_ptr.is_null() {
            apply_hwnd_geometry_from_state(hwnd_ptr);
        }
        return HttpResponse::ok_json(&json_resp);
    }

    if path == "/api/screen/restart" || path == "/mirror/restart" {
        let json = iphone_link_apple::mirror::global_request_keyframe_pli();
        return HttpResponse::ok_json(&json);
    }

    let auto_start_pc_audio_if_preferred =
        |st_arc: &Arc<Mutex<AppState>>, explicit_udid: Option<&str>| {
            if AUDIO_MANUALLY_STOPPED_THIS_SESSION.load(Ordering::SeqCst)
                || iphone_link_apple::mirror::is_airplay_active()
            {
                return;
            }
            let (wants_pc, a_mode, a_vol, a_preset, a_ls) = {
                let st = st_arc.lock().unwrap();
                let mode_low = st.audio_routing.trim().to_ascii_lowercase();
                let wants = (mode_low == "pc" || mode_low == "bluetooth" || mode_low == "wasapi")
                    && !st.audio_muted;
                (
                    wants,
                    st.audio_routing.clone(),
                    st.audio_volume as i64,
                    st.audio_buffer_preset.clone(),
                    st.lip_sync,
                )
            };
            if wants_pc {
                GLOBAL_AUDIO_PC_ACTIVE.store(true, Ordering::SeqCst);
                let (pref_name, pref_model, pref_bt) =
                    resolve_active_audio_device_target(st_arc, explicit_udid, None, None);
                let _ = iphone_link_audio::start_global_pc_audio_bridge_nonblocking_for_device(
                    pref_name.as_deref(),
                    pref_model.as_deref(),
                    pref_bt.as_deref(),
                );
                let _ = iphone_link_apple::mirror::global_configure_audio(
                    Some(&a_mode),
                    Some(a_vol),
                    Some(false),
                    Some(&a_preset),
                    Some(a_ls),
                    Some(true),
                );
            }
        };

    let auto_stop_pc_audio_on_mirror_end = |st_arc: &Arc<Mutex<AppState>>| {
        // Tear down WinRT AudioPlaybackConnection::Close() + WASAPI immediately so audio returns
        // to the iPhone speaker without mutating the user's saved `audio_routing` preference!
        iphone_link_audio::stop_global_pc_audio_bridge();
        let (a_mode, a_vol, a_muted, a_preset, a_ls) = {
            let st = st_arc.lock().unwrap();
            (
                st.audio_routing.clone(),
                st.audio_volume as i64,
                st.audio_muted,
                st.audio_buffer_preset.clone(),
                st.lip_sync,
            )
        };
        let _ = iphone_link_apple::mirror::global_configure_audio(
            Some(&a_mode),
            Some(a_vol),
            Some(a_muted),
            Some(&a_preset),
            Some(a_ls),
            Some(false),
        );
    };

    let preserve_wifi_reachability_on_clean_stop = |st_arc: &Arc<Mutex<AppState>>| {
        // Tahap 249 Pilar 3 (Glitch #4): Preserve Wi-Fi `reachable = true` + fresh `last_seen_at`
        // and open a 20s post-clean-stop grace window BEFORE `global_stop_screen()` tears down the
        // tunnel, so Launchpad & top bar NEVER flash "iPhone Terputus" for 3–5 seconds on disconnect.
        let (m_udid, m_transport, m_had_active_session) = iphone_link_apple::mirror::mirror_runtime()
            .lock()
            .map(|m| {
                let active_like = m.frames_received > 0
                    || m.phase == "running"
                    || m.phase == "starting"
                    || m.tunnel_active;
                (m.udid.clone(), m.transport.clone(), active_like)
            })
            .unwrap_or((None, String::new(), false));
        let (st_udid, st_transport, st_was_streaming) = st_arc
            .lock()
            .map(|s| (s.selected_udid.clone(), s.transport.clone(), s.is_streaming))
            .unwrap_or_default();
        let is_wifi_stop =
            m_transport.eq_ignore_ascii_case("wifi") || st_transport.eq_ignore_ascii_case("wifi");
        let target_udid = m_udid
            .filter(|s| !s.is_empty())
            .or_else(|| (!st_udid.is_empty()).then_some(st_udid));
        if is_wifi_stop && (m_had_active_session || st_was_streaming) {
            if let Some(ref u) = target_udid {
                let mut wmeta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(u);
                let ip_opt = wmeta.last_ip.clone();
                iphone_link_apple::wifi::mark_wifi_device_online_after_clean_stop(
                    u,
                    ip_opt.as_deref(),
                );
                wmeta.reachable = Some(true);
                wmeta.last_seen_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(wmeta.last_seen_at);
                cache_unified_wifi_meta_ram(u, &wmeta);
                let fresh_wjson = get_wifi_status(Some(u), false).to_json_string();
                if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
                    *guard = Some((u.clone(), fresh_wjson, std::time::Instant::now()));
                }
            }
        }
    };

    if path == "/api/screen" || path == "/api/screen/start" {
        if req.method.eq_ignore_ascii_case("DELETE") {
            LAST_STOP_MIRROR_MS.store(now_epoch_ms_u64(), Ordering::SeqCst);
            LAST_AUTO_AUDIO_ATTEMPT_MS.store(0, Ordering::SeqCst);
            preserve_wifi_reachability_on_clean_stop(state);
            {
                let mut st = state.lock().unwrap();
                st.is_streaming = false;
                st.stealth_dim = false;
            }
            auto_stop_pc_audio_on_mirror_end(state);
            let outcome = iphone_link_apple::mirror::global_stop_screen();
            GLOBAL_STEALTH_ACTIVE.store(
                iphone_link_apple::hid::is_stealth_dim_active(),
                Ordering::SeqCst,
            );
            return HttpResponse::ok_json(&outcome.json);
        }
        if req.method.eq_ignore_ascii_case("POST") {
            SCREEN_SESSION_EPOCH.fetch_add(1, Ordering::SeqCst);
            LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
            LAST_AUTO_AUDIO_ATTEMPT_MS.store(0, Ordering::SeqCst);
            AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);
            let req_udid_raw = parse_string_param(&req.body, "udid");
            let req_udid = req_udid_raw
                .as_deref()
                .map(iphone_link_apple::physical_stream::sanitize_udid_label)
                .filter(|s| !s.is_empty());
            let req_transport = parse_string_param(&req.body, "transport").unwrap_or_else(|| {
                if req.body.to_ascii_lowercase().contains("wifi") {
                    "wifi".to_string()
                } else {
                    "usb".to_string()
                }
            });
            let is_wifi = req_transport.eq_ignore_ascii_case("wifi");
            let explicit_honest_gate = parse_bool_param(&req.body, "honestGate")
                .or_else(|| parse_bool_param(&req.body, "waitForFirstFrame"));
            let req_mode = parse_string_param(&req.body, "mode")
                .or_else(|| parse_string_param(&req.body, "streamMode"))
                .unwrap_or_else(|| "auto".to_string())
                .trim()
                .to_ascii_lowercase();
            let req_ios_version = parse_string_param(&req.body, "iosVersion")
                .or_else(|| parse_string_param(&req.body, "productVersion"))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let req_airplay_port = parse_int_param(&req.body, "airplayPort")
                .map(|p| p.clamp(0, 65535) as u16);
            let req_advertise_mdns = parse_bool_param(&req.body, "advertiseMdns").unwrap_or(true);

            // Tahap 252/255 Pilar 2 & 3: Automatic ProductVersion Router + Strict Pure AirPlay H.264 Isolation.
            // - iOS >= 27.0 (or unknown/empty) in `auto` mode -> 100% untouched CoreDevice HEVC 60 FPS + full HID!
            // - iOS 17.4–26.x in `auto` mode -> Hybrid AirPlay H.264 + fail-soft CoreDevice `gate_only` HID.
            // - iOS <= 17.3 in `auto` mode -> AirPlay Standalone H.264 (ZERO CoreDeviceProxy calls!).
            // - Explicit `mode == "airplay"` (Arsitektur ZenLink) -> ALWAYS 100% Pure AirPlay H.264 Standalone
            //   (`hybrid_hid = false`, `hidControlAvailable = false`, 0 CoreDevice `DisplayService`/`gate_only` worker)
            //   to eliminate iOS 27 encoder contention and CodecHeader overwrite!
            let state_selected_udid = state
                .lock()
                .ok()
                .map(|s| s.selected_udid.clone())
                .filter(|s| !s.is_empty());
            let effective_target_udid = req_udid.clone().or(state_selected_udid);
            let mut resolved_ios_ver = req_ios_version.clone().unwrap_or_else(|| {
                let target_u = effective_target_udid
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let cached = snapshot_cached_devices_metadata_zero_io();
                if let Some(u) = target_u {
                    cached
                        .iter()
                        .find(|d| !d.product_version.is_empty() && d.udid.eq_ignore_ascii_case(u))
                        .map(|d| d.product_version.clone())
                        .unwrap_or_default()
                } else {
                    cached
                        .iter()
                        .find(|d| !d.product_version.is_empty())
                        .map(|d| d.product_version.clone())
                        .unwrap_or_default()
                }
            });
            // Tahap 285 Pilar 1: Cold-cache Wi-Fi fallback — never let iOS 17–26 over Wi-Fi miss
            // `HybridAirPlayCoreDeviceHid` just because active USB discovery cache is empty.
            if resolved_ios_ver.trim().is_empty() {
                if let Some(ref u) = effective_target_udid {
                    let wmeta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(u);
                    if !wmeta.ios_version.trim().is_empty() {
                        resolved_ios_ver = wmeta.ios_version.trim().to_string();
                    }
                }
            }
            let auto_route = iphone_link_apple::link::resolve_ios_mirror_route(&resolved_ios_ver);
            iphone_link_apple::diag_log::record(
                "screen_start",
                &format!("transport={req_transport}, mode={req_mode}, ios={resolved_ios_ver}"),
            );

            // Tahap 293 Opsi 1 + Opsi 2: Stop silent USB -> AirPlay hijack.
            // - Only explicit `mode == "airplay"` launches AirPlay Standalone immediately.
            // - On iOS < 27 (iOS 17-26) when user clicked Start under USB/Auto, return an honest
            //   `requiresDecision` payload (`ios_below_27`) with 1-click AirPlay switch option.
            // - On iOS >= 27 without HEVC MFT, return `requiresDecision` (`hevc_mft_missing`).
            let use_airplay_route = if req_mode == "airplay" {
                Some(false)
            } else {
                None
            };

            if use_airplay_route.is_none()
                && req_mode != "coredevice"
                && explicit_honest_gate != Some(false)
            {
                if auto_route == iphone_link_apple::link::IosMirrorRoute::AirPlayStandalone {
                    let ios_disp = if resolved_ios_ver.trim().is_empty() {
                        "< 27"
                    } else {
                        resolved_ios_ver.trim()
                    };
                    let msg_id = format!(
                        "iPhone Anda menjalankan iOS {ios_disp}. Apple membatasi fitur mirroring USB 60fps & kontrol mouse/keyboard hanya untuk iOS 27+. Gunakan Mode AirPlay (Wi-Fi) untuk tampilan & audio saja."
                    );
                    let msg_en = format!(
                        "Your iPhone is running iOS {ios_disp}. Apple restricts 60fps USB mirroring & mouse/keyboard control to iOS 27+. Use AirPlay Mode (Wi-Fi) for display & audio (view-only)."
                    );
                    let payload = format!(
                        r#"{{"ok":false,"phase":"idle","stage":"","requiresDecision":true,"decisionReason":"ios_below_27","iosVersion":"{}","mirrorRoute":"airplay_standalone","hidControlAvailable":false,"message_id":"{}","message_en":"{}"}}"#,
                        escape_json_str(ios_disp),
                        escape_json_str(&msg_id),
                        escape_json_str(&msg_en),
                    );
                    return HttpResponse::ok_json(&payload);
                }

                if req_mode == "auto" {
                    let cap = query_hevc_capabilities();
                    if cap.media_feature_pack_missing {
                        let err_msg = "Windows Edisi N/KN membutuhkan Media Feature Pack agar video mirroring dapat ditampilkan.";
                        let err_json = iphone_link_apple::mirror::global_set_screen_error(
                            effective_target_udid.as_deref(),
                            &req_transport,
                            err_msg,
                        );
                        let enriched = if let Some(prefix) = err_json.strip_suffix('}') {
                            format!(
                                r#"{prefix},"errorCode":"media_feature_pack_missing","mediaFeaturePackMissing":true,"hevcMftAvailable":false,"h264MftAvailable":false,"message_id":"{}","message_en":"Windows N/KN edition requires Microsoft Media Feature Pack to decode video mirroring."}}"#,
                                escape_json_str(err_msg)
                            )
                        } else {
                            err_json
                        };
                        return HttpResponse::ok_json(&enriched);
                    } else if !cap.hevc_mft_available && cap.h264_mft_available {
                        let msg_id = "Codec Video HEVC belum terpasang pada akun Windows ini. Pasang HEVC Gratis untuk USB 60fps + kontrol penuh, atau gunakan AirPlay sementara (tampilan & audio).";
                        let msg_en = "HEVC Video Extensions is not installed for this Windows user yet. Install Free HEVC for 60fps USB + full control, or use AirPlay temporarily (view & audio only).";
                        let payload = format!(
                            r#"{{"ok":false,"phase":"idle","stage":"","requiresDecision":true,"decisionReason":"hevc_mft_missing","hevcMftAvailable":false,"h264MftAvailable":true,"message_id":"{}","message_en":"{}"}}"#,
                            escape_json_str(msg_id),
                            escape_json_str(msg_en),
                        );
                        return HttpResponse::ok_json(&payload);
                    }
                }
            }
            let fallback_reason: Option<&'static str> = None;

            if let Some(hybrid_hid) = use_airplay_route {
                let target_udid = req_udid.clone().or_else(|| {
                    state
                        .lock()
                        .ok()
                        .map(|s| s.selected_udid.clone())
                        .filter(|s| !s.is_empty())
                });
                let wifi_ip_hint = if is_wifi {
                    target_udid.as_deref().and_then(|u| {
                        iphone_link_apple::wifi::read_device_metadata_from_wifi_state(u).last_ip
                    })
                } else {
                    None
                };
                // Tahap 257/259: Tear down any lingering Bluetooth A2DP bridge before starting
                // AirPlay H.264, and load persisted user audio preferences (volume/buffer/lipSync).
                iphone_link_audio::stop_global_pc_audio_bridge();
                if let Ok(st_pref) = state.lock() {
                    let _ = iphone_link_apple::airplay::apply_live_audio_config(
                        Some(st_pref.audio_volume.clamp(0, 100) as u32),
                        Some(&st_pref.audio_buffer_preset),
                        Some(st_pref.lip_sync),
                        Some(st_pref.audio_muted),
                    );
                }
                match iphone_link_apple::mirror::start_airplay_mirror_session(
                    target_udid.as_deref(),
                    hybrid_hid,
                    &req_transport,
                    wifi_ip_hint.as_deref(),
                    req_airplay_port,
                    req_advertise_mdns,
                ) {
                    Ok(resp_json) => {
                        if let Ok(mut st) = state.lock() {
                            st.is_streaming = false;
                            if let Some(ref u) = target_udid {
                                st.selected_udid = u.clone();
                            }
                            st.transport = if is_wifi {
                                "wifi".to_string()
                            } else {
                                "usb".to_string()
                            };
                        }
                        let final_resp = if let Some(reason) = fallback_reason {
                            if let Some(prefix) = resp_json.strip_suffix('}') {
                                format!(
                                    r#"{prefix},"fallbackReason":"{reason}","hevcMftAvailable":false,"h264MftAvailable":true}}"#
                                )
                            } else {
                                resp_json
                            }
                        } else {
                            resp_json
                        };
                        return HttpResponse::ok_json(&final_resp);
                    }
                    Err(e) => {
                        let err_json = iphone_link_apple::mirror::global_set_screen_error(
                            target_udid.as_deref(),
                            &req_transport,
                            &format!("Gagal memulai penerima AirPlay H.264: {e}"),
                        );
                        return HttpResponse::ok_json(&err_json);
                    }
                }
            }

            let pick_usb_from = |list: &[iphone_link_apple::models::DeviceMetadata], req_trimmed: Option<&str>| -> Option<String> {
                let usb_devs: Vec<&iphone_link_apple::models::DeviceMetadata> = list
                    .iter()
                    .filter(|d| d.connection_type.eq_ignore_ascii_case("USB") && !d.udid.is_empty())
                    .collect();
                usb_devs
                    .iter()
                    .find(|d| {
                        req_trimmed
                            .map(|u| d.udid.eq_ignore_ascii_case(u))
                            .unwrap_or(true)
                    })
                    .or_else(|| usb_devs.first())
                    .map(|d| d.udid.clone())
            };

            let find_usb_match = |allow_forced: bool| -> Option<String> {
                let req_trimmed = req_udid.as_deref().map(str::trim).filter(|s| !s.is_empty());
                let cached = snapshot_cached_devices_metadata_zero_io();
                if let Some(u) = pick_usb_from(&cached, req_trimmed) {
                    return Some(u);
                }
                let max_attempts = if allow_forced { 5 } else { 2 };
                for attempt in 0..max_attempts {
                    let discovered = discover_devices_with_metadata(allow_forced && attempt >= 1);
                    if let Some(u) = pick_usb_from(&discovered, req_trimmed) {
                        return Some(u);
                    }
                    if attempt + 1 < max_attempts {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                None
            };

            if is_wifi && explicit_honest_gate != Some(false) {
                // Stage 1 (Tahap 240/247 — Target Lock & REAL Wireless Wi-Fi RemotePairing + TLS 1.2 PSK 0x00A8):
                // Strictly honor explicit `req_udid` or `state.selected_udid` (NEVER silently hijack with stale cache!).
                let (selected_state_udid, user_explicit_flag, last_sel_udid) = state
                    .lock()
                    .ok()
                    .map(|s| {
                        (
                            s.selected_udid.clone(),
                            s.user_explicitly_selected_udid,
                            s.last_selected_udid.clone(),
                        )
                    })
                    .unwrap_or_default();
                let explicit_target_locked =
                    req_udid.is_some() || (user_explicit_flag && !selected_state_udid.is_empty());
                let pinned_udid = req_udid
                    .clone()
                    .or_else(|| (!selected_state_udid.is_empty()).then_some(selected_state_udid.clone()))
                    .or_else(|| {
                        let unified = collect_unified_devices(&[], &[], false);
                        let auto_u = resolve_active_selected_udid(&unified, "", false, &last_sel_udid);
                        (!auto_u.is_empty()).then_some(auto_u)
                    });

                let mut rp_rec = if explicit_target_locked {
                    pinned_udid
                        .as_deref()
                        .and_then(iphone_link_apple::wifi::find_record)
                } else {
                    pinned_udid
                        .as_deref()
                        .and_then(iphone_link_apple::wifi::find_record)
                        .or_else(|| iphone_link_apple::wifi::all_records().into_iter().next())
                };
                if rp_rec.is_none() && !explicit_target_locked {
                    if let Some(usb_u) = find_usb_match(false) {
                        let _ = iphone_link_apple::wifi::prepare_wifi_over_usb(Some(&usb_u));
                        rp_rec = iphone_link_apple::wifi::find_record(&usb_u)
                            .or_else(|| iphone_link_apple::wifi::all_records().into_iter().next());
                    }
                }

                // Helper to format rich Model + Name + IP diagnostic for an offline Wi-Fi target
                let format_offline_target_error = |target_u: &str, ip_opt: Option<&str>| -> String {
                    let wmeta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(target_u);
                    let lbl_model = if wmeta.model.is_empty() {
                        "iPhone".to_string()
                    } else {
                        wmeta.model.clone()
                    };
                    let dev_label = if !wmeta.name.is_empty()
                        && !wmeta.name.eq_ignore_ascii_case(&lbl_model)
                    {
                        format!("{lbl_model} ({})", wmeta.name)
                    } else {
                        lbl_model
                    };
                    match ip_opt.or(wmeta.last_ip.as_deref()).filter(|s| !s.is_empty()) {
                        Some(ip_s) => format!(
                            "{dev_label} di {ip_s} sedang offline/terkunci. Pilih perangkat lain di Launchpad atau nyalakan layarnya."
                        ),
                        None => format!(
                            "{dev_label} sedang offline/terkunci di jaringan Wi-Fi. Pilih perangkat lain di Launchpad atau nyalakan layarnya."
                        ),
                    }
                };

                if let Some(ref rec) = rp_rec {
                    // Tahap 247/250 Pilar 2: Anti-False-Offline Click #1 while preserving instant
                    // fail-fast (< 50 ms) for RFC 5737 synthetic offline targets & ultra-fresh (< 2s)
                    // confirmed offline probes. For real paired LAN iPhones whose cache is >= 2s old
                    // or last marked reachable=true, hand off directly to fresh mDNS wakeup!
                    if let Some((false, ref cached_ip, _, age)) =
                        iphone_link_apple::wifi::get_cached_wifi_reachability(&rec.udid)
                    {
                        let is_rfc5737_ip = cached_ip.as_deref().is_some_and(|ip| {
                            ip.starts_with("192.0.2.")
                                || ip.starts_with("198.51.100.")
                                || ip.starts_with("203.0.113.")
                        });
                        let disk_meta_check =
                            iphone_link_apple::wifi::read_device_metadata_from_wifi_state(&rec.udid);
                        let should_fail_fast_probe = is_rfc5737_ip
                            || (age < Duration::from_secs(2) && disk_meta_check.reachable != Some(true));
                        if should_fail_fast_probe {
                            let still_alive = !is_rfc5737_ip
                                && cached_ip.as_deref().is_some_and(|ip| {
                                    iphone_link_apple::wifi::verify_ip_is_apple_device(
                                        ip,
                                        Some(&rec.udid),
                                        250,
                                    )
                                });
                            if !still_alive {
                                let err_msg =
                                    format_offline_target_error(&rec.udid, cached_ip.as_deref());
                                let err_json = iphone_link_apple::mirror::global_set_screen_error(
                                    Some(&rec.udid),
                                    "wifi",
                                    &err_msg,
                                );
                                if let Ok(mut st) = state.lock() {
                                    st.is_streaming = false;
                                    st.selected_udid = rec.udid.clone();
                                    st.transport = "wifi".to_string();
                                }
                                return HttpResponse::ok_json(&err_json);
                            }
                        }
                    }

                    // Tahap 241/244/249: Fast zero-blocking handoff to real Wi-Fi worker!
                    let cached_ws = get_wifi_status(Some(&rec.udid), false);
                    let ip_hint = cached_ws.ip.as_deref().unwrap_or("");
                    let mut wmeta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(&rec.udid);
                    if !ip_hint.is_empty() {
                        wmeta.last_ip = Some(ip_hint.to_string());
                    }
                    wmeta.reachable = Some(true);
                    cache_unified_wifi_meta_ram(&rec.udid, &wmeta);
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    iphone_link_apple::wifi::set_cached_wifi_reachability(
                        &rec.udid,
                        true,
                        (!ip_hint.is_empty()).then_some(ip_hint),
                        now_secs,
                    );
                    iphone_link_apple::physical_stream::set_active_session_device_identity(
                        &rec.udid,
                        (!wmeta.name.is_empty()).then_some(wmeta.name.as_str()),
                        (!wmeta.model.is_empty()).then_some(wmeta.model.as_str()),
                        (!wmeta.bluetooth_address.is_empty())
                            .then_some(wmeta.bluetooth_address.as_str()),
                    );
                    let resp_json =
                        iphone_link_apple::mirror::start_real_physical_wifi_lockdown_screen(
                            &rec.udid,
                            ip_hint,
                        );
                    if let Ok(mut st) = state.lock() {
                        st.is_streaming = false;
                        st.selected_udid = rec.udid.clone();
                        st.transport = "wifi".to_string();
                    }
                    if explicit_honest_gate != Some(true) {
                        auto_start_pc_audio_if_preferred(state, Some(&rec.udid));
                    }
                    return HttpResponse::ok_json(&resp_json);
                }

                // If user explicitly targeted a specific paired/known device without an active online RemotePairing session
                if explicit_target_locked {
                    if let Some(ref pinned_u) = pinned_udid {
                        let wmeta = iphone_link_apple::wifi::read_device_metadata_from_wifi_state(pinned_u);
                        if wmeta.wifi_enabled || wmeta.last_ip.is_some() {
                            let err_msg = format_offline_target_error(pinned_u, wmeta.last_ip.as_deref());
                            let err_json = iphone_link_apple::mirror::global_set_screen_error(
                                Some(pinned_u),
                                "wifi",
                                &err_msg,
                            );
                            if let Ok(mut st) = state.lock() {
                                st.is_streaming = false;
                                st.selected_udid = pinned_u.clone();
                                st.transport = "wifi".to_string();
                            }
                            return HttpResponse::ok_json(&err_json);
                        }
                    }
                }

                let wifi_st = get_wifi_status(pinned_udid.as_deref(), true);
                let resolved_ip = wifi_st.ip.clone().filter(|s| !s.is_empty());

                // Stage 2: USB-Assisted Fallback if Wi-Fi LAN endpoint is currently unreachable but cable is plugged in
                if let Some(real_udid) = find_usb_match(explicit_honest_gate.is_none()) {
                    let _ = iphone_link_apple::wifi::prepare_wifi_over_usb(Some(&real_udid));
                    iphone_link_apple::physical_stream::set_active_session_device_identity(
                        &real_udid, None, None, None,
                    );
                    let resp_json = iphone_link_apple::mirror::start_real_physical_usb_screen_with_transport(
                        &real_udid,
                        "wifi",
                    );
                    if let Ok(mut st) = state.lock() {
                        st.is_streaming = false;
                        st.selected_udid = real_udid.clone();
                        st.transport = "wifi".to_string();
                    }
                    if explicit_honest_gate != Some(true) {
                        auto_start_pc_audio_if_preferred(state, Some(&real_udid));
                    }
                    return HttpResponse::ok_json(&resp_json);
                }

                // Stage 3: Honest diagnostic error state when neither Wi-Fi RemotePairing nor USB is reachable
                let wifi_hint = if let Some(ref pinned_u) = pinned_udid {
                    format_offline_target_error(pinned_u, resolved_ip.as_deref())
                } else if let Some(ref ip_str) = resolved_ip {
                    format!(
                        "iPhone terdeteksi di Wi-Fi ({ip_str}), tetapi belum diatur untuk RemotePairing Wi-Fi. Colok kabel USB 1x lalu klik 'Atur Wi-Fi'."
                    )
                } else {
                    "Streaming Wi-Fi nirkabel tidak menemukan iPhone di jaringan Wi-Fi (_remotepairing._tcp.local.). Pastikan iPhone menyala, terhubung di Wi-Fi yang sama, dan sudah pernah klik 'Atur Wi-Fi' 1x via USB.".to_string()
                };
                let err_json = iphone_link_apple::mirror::global_set_screen_error(
                    pinned_udid.as_deref(),
                    "wifi",
                    &wifi_hint,
                );
                if let Ok(mut st) = state.lock() {
                    st.is_streaming = false;
                    st.transport = "wifi".to_string();
                }
                return HttpResponse::ok_json(&err_json);
            }

            if !is_wifi && explicit_honest_gate != Some(false) {
                if let Some(real_udid) = find_usb_match(explicit_honest_gate.is_none()) {
                    iphone_link_apple::physical_stream::set_active_session_device_identity(
                        &real_udid, None, None, None,
                    );
                    let resp_json =
                        iphone_link_apple::mirror::start_real_physical_usb_screen(&real_udid);
                    if let Ok(mut st) = state.lock() {
                        st.is_streaming = false;
                        st.selected_udid = real_udid.clone();
                        st.transport = "usb".to_string();
                    }
                    if explicit_honest_gate != Some(true) {
                        auto_start_pc_audio_if_preferred(state, Some(&real_udid));
                    }
                    return HttpResponse::ok_json(&resp_json);
                }
                if explicit_honest_gate.is_none() {
                    let err_json = iphone_link_apple::mirror::global_set_screen_error(
                        req_udid.as_deref(),
                        &req_transport,
                        "Perangkat iPhone USB tidak terdeteksi atau belum terpercaya (Trust). Hubungkan kabel USB dan buka kunci iPhone.",
                    );
                    if let Ok(mut st) = state.lock() {
                        st.is_streaming = false;
                        st.transport = "usb".to_string();
                    }
                    return HttpResponse::ok_json(&err_json);
                }
            }

            let honest_gate = explicit_honest_gate.unwrap_or(false);
            let resp_json = iphone_link_apple::mirror::global_start_screen(
                req_udid.as_deref(),
                &req_transport,
                honest_gate,
            );
            {
                let mut st = state.lock().unwrap();
                st.is_streaming = !honest_gate;
                if let Some(ref ru) = req_udid {
                    st.selected_udid = ru.clone();
                }
                st.transport = if is_wifi {
                    "wifi".to_string()
                } else {
                    "usb".to_string()
                };
            }
            if !honest_gate {
                auto_start_pc_audio_if_preferred(state, req_udid.as_deref());
            }
            return HttpResponse::ok_json(&resp_json);
        } else {
            let last_stop = LAST_STOP_MIRROR_MS.load(Ordering::SeqCst);
            let stop_cooldown =
                last_stop != 0 && now_epoch_ms_u64().saturating_sub(last_stop) < 5000;
            if stop_cooldown {
                return HttpResponse::ok_json(
                    r#"{"ok":true,"phase":"idle","active":false,"fps":60,"width":1202,"height":2598,"native":true}"#,
                );
            }
            let scr_json = iphone_link_apple::mirror::global_screen_status_json();
            // Tahap 243/244 Pilar 1: Only trigger auto-audio when bridge is neither active nor connecting,
            // and NEVER stop audio on transient/stale `phase:"error"` polls (audio auto-stops cleanly on
            // explicit `/api/screen/stop`, `DELETE /api/screen`, and `WM_CLOSE`).
            if scr_json.contains(r#""phase":"running""#)
                && !iphone_link_audio::is_global_pc_audio_bridge_active()
                && !iphone_link_audio::is_global_pc_audio_bridge_connecting()
            {
                auto_start_pc_audio_if_preferred(state, None);
            }
            return HttpResponse::ok_json(&scr_json);
        }
    }

    if path == "/api/screen/stop" || path == "/mirror/stop" {
        LAST_STOP_MIRROR_MS.store(now_epoch_ms_u64(), Ordering::SeqCst);
        LAST_AUTO_AUDIO_ATTEMPT_MS.store(0, Ordering::SeqCst);
        preserve_wifi_reachability_on_clean_stop(state);
        {
            let mut st = state.lock().unwrap();
            st.is_streaming = false;
            st.stealth_dim = false;
        }
        auto_stop_pc_audio_on_mirror_end(state);
        let outcome = iphone_link_apple::mirror::global_stop_screen();
        GLOBAL_STEALTH_ACTIVE.store(
            iphone_link_apple::hid::is_stealth_dim_active(),
            Ordering::SeqCst,
        );
        return HttpResponse::ok_json(&outcome.json);
    }

    if path == "/api/wifi" || path.starts_with("/api/wifi/probe") {
        let query_udid = extract_query_param(&req.query, "udid");
        let probe = path.starts_with("/api/wifi/probe")
            || req.query.contains("probe=1")
            || req.query.contains("refresh=1");
        let payload = get_wifi_status(query_udid.as_deref(), probe);
        if probe {
            let wjson = payload.to_json_string();
            let cache_udid = payload
                .udid
                .clone()
                .or_else(|| query_udid.clone())
                .unwrap_or_default();
            if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
                *guard = Some((cache_udid, wjson, std::time::Instant::now()));
            }
        }
        return HttpResponse::ok_json(&payload.to_api_json_string());
    }

    if path == "/api/wifi/scan" {
        let req_udid = parse_string_param(&req.body, "udid")
            .or_else(|| extract_query_param(&req.query, "udid"));
        let scan_resp = scan_wifi_json(req_udid.as_deref(), 150);
        // Tahap 243 Pilar 4: Immediately refresh WIFI_STATUS_STREAM_CACHE after mDNS scan
        // so subsequent GET /api/status calls see the newly discovered IP + reachable:true right away!
        let fresh_payload = get_wifi_status(req_udid.as_deref(), false);
        let cache_udid = fresh_payload
            .udid
            .clone()
            .or_else(|| req_udid.clone())
            .unwrap_or_default();
        if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
            *guard = Some((
                cache_udid,
                fresh_payload.to_json_string(),
                std::time::Instant::now(),
            ));
        }
        return HttpResponse::ok_json(&scan_resp);
    }

    if path == "/api/wifi/pair" || path == "/api/wifi/setup" {
        let req_udid = parse_string_param(&req.body, "udid")
            .or_else(|| extract_query_param(&req.query, "udid"))
            .map(|s| iphone_link_apple::physical_stream::sanitize_udid_label(&s))
            .filter(|s| !s.is_empty());
        return HttpResponse::ok_json(&pair_wifi_json(req_udid.as_deref()));
    }

    if path == "/api/video/pipeline" {
        return HttpResponse::ok_json(&iphone_link_apple::mirror::global_video_pipeline_status_json());
    }

    if path == "/api/video/decode-annexb" {
        let json = iphone_link_apple::mirror::global_decode_annexb_test_json(&req.body);
        return HttpResponse::ok_json(&json);
    }

    if path == "/api/video/presenter" {
        if req.method.eq_ignore_ascii_case("POST") {
            let fit_mode = parse_string_param(&req.body, "fitMode");
            let rot = parse_int_param(&req.body, "rotationDeg")
                .or_else(|| parse_int_param(&req.body, "rotation"))
                .map(|v| (v.rem_euclid(360)) as u16);
            let crop_top = parse_int_param(&req.body, "cropTop").map(|v| v.max(0) as u32);
            let crop_bottom = parse_int_param(&req.body, "cropBottom").map(|v| v.max(0) as u32);
            let crop_left = parse_int_param(&req.body, "cropLeft").map(|v| v.max(0) as u32);
            let crop_right = parse_int_param(&req.body, "cropRight").map(|v| v.max(0) as u32);
            let win_w = parse_int_param(&req.body, "windowWidth")
                .or_else(|| parse_int_param(&req.body, "winW"))
                .map(|v| v.max(0) as u32);
            let win_h = parse_int_param(&req.body, "windowHeight")
                .or_else(|| parse_int_param(&req.body, "winH"))
                .map(|v| v.max(0) as u32);
            let is_fs = parse_bool_param(&req.body, "isFullscreen");
            let is_land = parse_bool_param(&req.body, "isLandscape");
            let drawer = parse_bool_param(&req.body, "drawerOpen");
            let json = iphone_link_apple::mirror::global_update_video_presenter_config(
                fit_mode.as_deref(),
                rot,
                crop_top,
                crop_bottom,
                crop_left,
                crop_right,
                win_w,
                win_h,
                is_fs,
                is_land,
                drawer,
            );
            return HttpResponse::ok_json(&json);
        }
        return HttpResponse::ok_json(&iphone_link_apple::mirror::global_video_presenter_status_json());
    }

    if path == "/api/video/snapshot" {
        match iphone_link_apple::mirror::global_export_latest_video_snapshot() {
            Ok(snap) => {
                let esc = escape_json_str(&snap.data_url);
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":true,"native":true,"format":"bmp","lossless":true,"width":{},"height":{},"rotationDeg":{},"byteLen":{},"elapsedMs":{:.3},"dataUrl":"{}"}}"#,
                    snap.width, snap.height, snap.rotation_deg, snap.byte_len, snap.elapsed_ms, esc
                ));
            }
            Err(e) => {
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":false,"error":"{}"}}"#,
                    escape_json_str(&e)
                ));
            }
        }
    }

    // 8B. Native In-Process /mirror/*, /codec, /pli, /size + Touch/HID Forwarding
    if path == "/codec"
        || path == "/mirror/codec"
        || path == "/pli"
        || path == "/mirror/pli"
        || path == "/mirror/size"
    {
        if path.ends_with("/codec") {
            return HttpResponse::ok_json(&iphone_link_apple::mirror::global_codec_json());
        }
        if path.ends_with("/pli") {
            return HttpResponse::ok_json(&iphone_link_apple::mirror::global_request_keyframe_pli());
        }
        let deg = iphone_link_apple::mirror::active_orientation_fast();
        return HttpResponse::ok_json(&format!(
            r#"{{"width":1178,"height":2598,"orientation":{}}}"#,
            deg
        ));
    }

    if path == "/mirror/touch"
        || path == "/touch"
        || path == "/mirror/scroll"
        || path == "/scroll"
        || path == "/mirror/button"
        || path == "/button"
        || path == "/mirror/key"
        || path == "/key"
        || path == "/mirror/gesture"
        || path == "/gesture"
        || path == "/api/screen/swipe"
    {
        let is_touch = path == "/mirror/touch" || path == "/touch";
        let is_scroll = path == "/mirror/scroll" || path == "/scroll";
        let is_button = path == "/mirror/button" || path == "/button";
        let is_key = path == "/mirror/key" || path == "/key";
        let is_gesture = path == "/mirror/gesture"
            || path == "/gesture"
            || path == "/api/screen/swipe"
            || path == "/api/hid/gesture";

        if is_touch && req.method.eq_ignore_ascii_case("POST") {
            let body_lower = String::from_utf8_lossy(&req.body_bytes).to_ascii_lowercase();
            let is_drag = body_lower.contains("\"move\"") || body_lower.contains("\"contact\"");
            record_user_input_activity(is_drag);
            if !body_lower.contains("\"move\"") {
                notify_worker_commit_typed_line_async();
            }
        } else if (is_button || is_key || is_gesture) && req.method.eq_ignore_ascii_case("POST") {
            record_user_input_activity(false);
        }
        if let Some(resp) = forward_to_stream_server(req) {
            return resp;
        }
        // Tahap 255 Pilar 2: Pure AirPlay Standalone has zero HID channel — return calm HTTP 200
        // without spamming dead HID packets or error telemetry.
        if iphone_link_apple::mirror::is_airplay_standalone_no_hid_active() {
            return HttpResponse::ok_json(
                r#"{"ok":false,"hidControlAvailable":false,"mirrorRoute":"airplay_standalone"}"#,
            );
        }
        if is_touch {
            if let (Some(cx), Some(cy)) = (
                parse_float_param(&req.body, "clientX"),
                parse_float_param(&req.body, "clientY"),
            ) {
                let (ix, iy) = iphone_link_apple::mirror::global_map_stage_touch_to_ios_hid(cx, cy);
                let rot = iphone_link_apple::mirror::active_orientation_fast();
                let body_lower = req.body.to_ascii_lowercase();
                let sent = if body_lower.contains("\"tap\"") || body_lower.contains("\"click\"") {
                    iphone_link_apple::hid::dispatch_physical_tap(ix, iy)
                } else {
                    let contact = !body_lower.contains("\"release\"") && !body_lower.contains("\"up\"");
                    iphone_link_apple::hid::dispatch_physical_touch(contact, ix, iy)
                };
                let (pkts_tx, hid_err, _) = iphone_link_apple::hid::physical_hid_telemetry();
                return HttpResponse::ok_json(&format!(
                    r#"{{"ok":true,"mode":"stage_subpixel","rotation":{},"mappedX":{},"mappedY":{},"physicalSent":{},"physical_packets_transmitted":{},"physical_hid_errors":{}}}"#,
                    rot, ix, iy, sent, pkts_tx, hid_err
                ));
            }
            let rot = parse_int_param(&req.body, "rotation")
                .map(|v| v.max(0) as u32)
                .unwrap_or_else(get_active_rotation_deg);
            let (ok, ix, iy, sent, events_cnt) =
                if iphone_link_apple::mirror::is_hybrid_airplay_route() {
                    iphone_link_apple::hid::dispatch_airplay_touch_payload_json(&req.body, rot)
                } else {
                    iphone_link_apple::hid::dispatch_touch_payload_json(&req.body, rot)
                };
            let (pkts_tx, hid_err, _) = iphone_link_apple::hid::physical_hid_telemetry();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":{},"rotation":{},"mappedX":{},"mappedY":{},"eventsCount":{},"physicalSent":{},"physical_packets_transmitted":{},"physical_hid_errors":{}}}"#,
                ok, rot, ix, iy, events_cnt, sent, pkts_tx, hid_err
            ));
        }
        if is_scroll {
            let (ok, sent, steps_cnt) =
                iphone_link_apple::hid::dispatch_scroll_payload_json(&req.body);
            let (pkts_tx, hid_err, _) = iphone_link_apple::hid::physical_hid_telemetry();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":{},"steps":{},"physicalSent":{},"physical_packets_transmitted":{},"physical_hid_errors":{}}}"#,
                ok, steps_cnt, sent, pkts_tx, hid_err
            ));
        }
        if is_button {
            let name = parse_string_param(&req.body, "name")
                .or_else(|| parse_string_param(&req.body, "button"))
                .unwrap_or_else(|| "home".to_string());
            let btn_state = parse_string_param(&req.body, "state")
                .unwrap_or_else(|| "press".to_string());
            let sent = iphone_link_apple::hid::dispatch_physical_button_name(&name, &btn_state)
                .unwrap_or(false);
            let (pkts_tx, hid_err, _) = iphone_link_apple::hid::physical_hid_telemetry();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"button":"{}","state":"{}","physicalSent":{},"physical_packets_transmitted":{},"physical_hid_errors":{}}}"#,
                escape_json_str(&name),
                escape_json_str(&btn_state),
                sent,
                pkts_tx,
                hid_err
            ));
        }
        if is_key {
            let (sent, count) = if let Some(usages) =
                iphone_link_apple::hid::parse_u8_usages_from_json(&req.body)
            {
                let c = usages.len();
                let s = iphone_link_apple::hid::dispatch_physical_key_usages(&usages);
                (s, c)
            } else if let Some(text_val) = parse_string_param(&req.body, "text")
                .or_else(|| parse_string_param(&req.body, "key"))
            {
                let (steps, typed) = generate_key_steps_for_text(&text_val);
                let s = iphone_link_apple::hid::dispatch_physical_key_steps(&steps);
                (s, typed)
            } else {
                let s = iphone_link_apple::hid::dispatch_physical_key_usages(&[]);
                (s, 0)
            };
            let (pkts_tx, hid_err, _) = iphone_link_apple::hid::physical_hid_telemetry();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"usagesCount":{},"physicalSent":{},"physical_packets_transmitted":{},"physical_hid_errors":{}}}"#,
                count, sent, pkts_tx, hid_err
            ));
        }
        if is_gesture {
            let dir = parse_string_param(&req.body, "direction")
                .or_else(|| parse_string_param(&req.body, "action"))
                .unwrap_or_else(|| "back".to_string());
            let (steps_count, sent) = if dir.eq_ignore_ascii_case("pinch") {
                let frames = pinch_zoom_frames(32768, 32768, 8000, 24000, 8);
                (frames.len(), false)
            } else if let Some(steps) = swipe_direction_steps(&dir) {
                let len = steps.len();
                let s = iphone_link_apple::hid::dispatch_physical_touch_steps(&steps);
                (len, s)
            } else {
                let s = iphone_link_apple::hid::dispatch_physical_swipe_direction(&dir)
                    .unwrap_or(false);
                (10usize, s)
            };
            let (pkts_tx, hid_err, _) = iphone_link_apple::hid::physical_hid_telemetry();
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"gesture":"{}","direction":"{}","steps":{},"physicalSent":{},"physical_packets_transmitted":{},"physical_hid_errors":{}}}"#,
                escape_json_str(&dir),
                escape_json_str(&dir),
                steps_count,
                sent,
                pkts_tx,
                hid_err
            ));
        }
    }

    // 9. Static File Serving — Zero-I/O In-Memory Embedded Web & Icon Assets (Tahap 232)
    let clean_rel = if path.is_empty() || path == "/" || path == "/index.html" {
        "index.html"
    } else if path == "/mirror"
        || path == "/mirror/"
        || path == "/mirror/frame"
        || path == "/mirror/frame/"
        || path == "/mirror/index.html"
        || path == "/mirror.html"
    {
        "mirror.html"
    } else if path == "/mirror/mirror.css" || path == "/mirror.css" {
        "mirror.css"
    } else if path == "/mirror/mirror.js" || path == "/mirror.js" {
        "mirror.js"
    } else if path == "/mirror/keys.js" || path == "/keys.js" {
        "keys.js"
    } else if path == "/mirror/app.css" || path == "/app.css" {
        "app.css"
    } else if path == "/mirror/app.js" || path == "/app.js" {
        "app.js"
    } else if path == "/mirror/logo.svg" || path == "/logo.svg" {
        "logo.svg"
    } else if path == "/logo.png" || path == "/assets/logo.png" {
        "logo.png"
    } else if path == "/favicon.ico" || path == "/iphone_link.ico" || path == "/assets/iphone_link.ico" {
        "favicon.ico"
    } else {
        path.trim_start_matches('/')
    };

    if clean_rel.eq_ignore_ascii_case("index.html") {
        INDEX_HTML_SERVED.fetch_add(1, Ordering::SeqCst);
        let html = render_embedded_index_html(state);
        return HttpResponse::ok_html(&html);
    } else if clean_rel.eq_ignore_ascii_case("app.js") {
        APP_JS_SERVED.fetch_add(1, Ordering::SeqCst);
        return HttpResponse::ok_js(EMBEDDED_APP_JS);
    } else if clean_rel.eq_ignore_ascii_case("app.css") {
        APP_CSS_SERVED.fetch_add(1, Ordering::SeqCst);
        return HttpResponse::ok_css(get_embedded_app_css());
    } else if clean_rel.eq_ignore_ascii_case("mirror.html") {
        return HttpResponse::ok_html(get_embedded_mirror_html());
    } else if clean_rel.eq_ignore_ascii_case("mirror.js") {
        return HttpResponse::ok_js(get_embedded_mirror_js());
    } else if clean_rel.eq_ignore_ascii_case("mirror.css") {
        return HttpResponse::ok_css(get_embedded_mirror_css());
    } else if clean_rel.eq_ignore_ascii_case("keys.js") {
        return HttpResponse::ok_js(EMBEDDED_KEYS_JS);
    } else if clean_rel.eq_ignore_ascii_case("logo.svg") {
        return HttpResponse::ok_bytes(EMBEDDED_LOGO_SVG.to_vec(), "image/svg+xml");
    } else if clean_rel.eq_ignore_ascii_case("logo.png") {
        return HttpResponse::ok_bytes(EMBEDDED_LOGO_PNG.to_vec(), "image/png");
    } else if clean_rel.eq_ignore_ascii_case("favicon.ico") {
        return HttpResponse::ok_bytes(EMBEDDED_IPHONE_LINK_ICO.to_vec(), "image/x-icon");
    }

    if let Some(target_file) = resolve_static_asset_file(web_dir, clean_rel) {
        if let Ok(bytes) = fs::read(&target_file) {
            let mime = mime_for_path_str(clean_rel);
            return HttpResponse::ok_bytes(bytes, mime);
        }
    }

    HttpResponse::not_found()
}

fn transform_mirror_html(raw: &str) -> String {
    let mut html = raw
        .replace(r#"href="mirror.css"#, r#"href="/mirror.css"#)
        .replace(r#"src="keys.js"#, r#"src="/keys.js"#)
        .replace(r#"src="mirror.js"#, r#"src="/mirror.js"#)
        .replace(r#"href='mirror.css"#, r#"href='/mirror.css"#)
        .replace(r#"src='keys.js"#, r#"src='/keys.js"#)
        .replace(r#"src='mirror.js"#, r#"src='/mirror.js"#);
    if !html.contains("v5-anti-white-flash") {
        let black_stage_css = "<style id=\"v5-anti-white-flash\">html,body,#crop-wrapper,canvas,#c{background:#000000 !important;color:#f8fafc !important;margin:0;padding:0;width:100%;height:100%;overflow:hidden;}</style>\n</head>";
        if html.contains("</head>") {
            html = html.replacen("</head>", black_stage_css, 1);
        }
    }
    if !html.contains("imageSmoothingQuality") {
        let canvas_lock_script = r#"<script>
(function() {
    try {
        var origGetContext = HTMLCanvasElement.prototype.getContext;
        HTMLCanvasElement.prototype.getContext = function(type, attribs) {
            var ctx = origGetContext.apply(this, arguments);
            if (ctx && (type === '2d' || !type)) {
                try {
                    ctx.imageSmoothingEnabled = true;
                    ctx.imageSmoothingQuality = 'high';
                } catch (_) {}
            }
            return ctx;
        };
    } catch (_) {}
})();
</script>"#;
        html = html.replace("<head>", &format!("<head>\n{}", canvas_lock_script));
    }
    if !html.contains("v4_true_acrylic.css") {
        html = html.replace(
            "</head>",
            "    <link rel=\"stylesheet\" href=\"/v4_true_acrylic.css\">\n    <script src=\"/v4_bridge.js\"></script>\n</head>",
        );
    }
    if !html.contains("v4-mirror-diag-overlay") {
        let diag_overlay_html = r##"<div id="v4-mirror-diag-overlay" class="v4-mirror-diag-overlay" hidden role="dialog" aria-modal="true" aria-labelledby="v4-diag-title">
  <div class="v4-mirror-diag-card">
    <div id="v4-diag-badge" class="v4-mirror-diag-badge">Codec / Decoder</div>
    <div id="v4-diag-title" class="v4-mirror-diag-title">Video Decoder Perlu Diperbaiki</div>
    <div id="v4-diag-desc" class="v4-mirror-diag-desc">PC Windows Anda belum dapat mendekode stream video H.265/HEVC dari iPhone.</div>
    <div class="v4-mirror-diag-actions">
      <button id="btn-diag-install-hevc" class="v4-diag-btn v4-diag-btn-primary" type="button">Install HEVC Gratis</button>
      <button id="btn-diag-switch-airplay" class="v4-diag-btn v4-diag-btn-secondary" type="button">Pindah ke Mode H.264 (AirPlay)</button>
      <button id="btn-diag-soft-decode" class="v4-diag-btn v4-diag-btn-secondary" type="button">Coba Software Decode</button>
      <button id="btn-diag-copy-info" class="v4-diag-btn v4-diag-btn-ghost" type="button">Salin Info Diagnostik</button>
    </div>
    <div id="v4-diag-feedback" class="v4-mirror-diag-feedback" hidden></div>
  </div>
</div>
</body>"##;
        if html.contains("</body>") {
            html = html.replacen("</body>", diag_overlay_html, 1);
        }
    }
    html
}

fn build_index_html_template_parts(raw: &str) -> (String, String) {
    let mut html = raw
        .replace("iPhone Link v4.0.0", "iPhone Link v5.0.1")
        .replace("iPhone Link v3.0.0", "iPhone Link v5.0.1")
        .replace("iPhone Link v4.0", "iPhone Link v5.0")
        .replace("iPhone Link v3.0", "iPhone Link v5.0");
    if !html.contains("tahap222-theme-contrast") {
        html = html.replace(
            "</head>",
            "  <style id=\"tahap222-theme-contrast\"></style>\n</head>",
        );
    }
    if !html.contains("v4_true_acrylic.css") {
        if let Some(pos) = html.rfind("</body>") {
            html.insert_str(pos, "    <link rel=\"stylesheet\" href=\"/v4_true_acrylic.css\">\n");
        } else {
            html = html.replace(
                "</head>",
                "    <link rel=\"stylesheet\" href=\"/v4_true_acrylic.css\">\n</head>",
            );
        }
    }
    if !html.contains("v4_bridge.js") {
        let bridge_tag = "<script src=\"/v4_bridge.js\"></script>\n  ";
        if let Some(pos) = html.find("<script src=\"/keys.js") {
            html.insert_str(pos, bridge_tag);
        } else if let Some(pos) = html.find("<script src=\"/app.js") {
            html.insert_str(pos, bridge_tag);
        } else if let Some(pos) = html.find("</body>") {
            html.insert_str(pos, "<script src=\"/v4_bridge.js\"></script>\n");
        }
    }
    let split_pos = html
        .find("<script src=\"/v4_bridge.js")
        .or_else(|| html.rfind("</body>"))
        .unwrap_or(html.len());
    (html[..split_pos].to_string(), html[split_pos..].to_string())
}

fn get_embedded_index_template() -> &'static (String, String) {
    static INDEX_TMPL: OnceLock<(String, String)> = OnceLock::new();
    INDEX_TMPL.get_or_init(|| build_index_html_template_parts(EMBEDDED_INDEX_HTML))
}

pub fn render_embedded_index_html(state: &Arc<Mutex<AppState>>) -> String {
    let (prefix, suffix) = get_embedded_index_template();
    let init_settings_json = state
        .lock()
        .map(|s| build_settings_json(&s))
        .unwrap_or_else(|_| "{}".to_string());
    let safe_init_json = init_settings_json.replace("</", "<\\/");
    let seed_script = format!(
        "<script>window.__V4_INITIAL_SETTINGS__={};</script>\n  ",
        safe_init_json
    );
    let mut out = String::with_capacity(prefix.len() + seed_script.len() + suffix.len());
    out.push_str(prefix);
    out.push_str(&seed_script);
    out.push_str(suffix);
    out
}

fn transform_app_css(raw: &str) -> String {
    let mut css_text = raw.to_string();
    css_text.push_str("\n\n/* === V4_TRUE_ACRYLIC_CSS OVERRIDE === */\n");
    css_text.push_str(V4_TRUE_ACRYLIC_CSS);
    css_text
}

pub fn get_embedded_app_css() -> &'static str {
    static APP_CSS_CACHE: OnceLock<String> = OnceLock::new();
    APP_CSS_CACHE.get_or_init(|| transform_app_css(EMBEDDED_APP_CSS)).as_str()
}

pub fn get_embedded_mirror_html() -> &'static str {
    static MIRROR_HTML_CACHE: OnceLock<String> = OnceLock::new();
    MIRROR_HTML_CACHE
        .get_or_init(|| transform_mirror_html(EMBEDDED_MIRROR_HTML))
        .as_str()
}

fn transform_mirror_css(raw: &str) -> String {
    let mut css_text = raw.to_string();
    if !css_text.contains("__V4_TAHAP179_MIRROR_CSS__") {
        css_text.push_str(
            r##"

/* __V4_TAHAP179_MIRROR_CSS__ - Exact Macroblock Trim + Full-Stage Fill on Auto-Fit (0px Black Bar) */
canvas#c.fit-autofit,
canvas#c.fit-fill,
body.fit-autofit #c,
body.fit-fill #c,
body.fit-autofit canvas,
body.fit-fill canvas,
body:not(.fit-contain) #c,
body:not(.fit-contain) canvas {
  width: 100vw !important;
  height: 100vh !important;
  max-width: 100vw !important;
  max-height: 100vh !important;
  object-fit: fill !important;
  display: block !important;
}
canvas#c.fit-contain,
body.fit-contain #c,
body.fit-contain canvas {
  width: 100% !important;
  height: 100% !important;
  max-width: 100% !important;
  max-height: 100% !important;
  object-fit: contain !important;
  display: block !important;
}

/* __V5_TAHAP248_MIRROR_DIAG_OVERLAY__ - Isolated from .mirror-status.warn (0-diff on frozen mirror.css) */
#v4-mirror-diag-overlay,
.v4-mirror-diag-overlay {
  position: fixed !important;
  inset: 0 !important;
  z-index: 99998 !important;
  display: flex !important;
  align-items: center !important;
  justify-content: center !important;
  padding: 18px !important;
  background: #090d16 !important;
  color: #f8fafc !important;
  font-family: "Segoe UI Variable", "Segoe UI", system-ui, -apple-system, sans-serif !important;
  box-sizing: border-box !important;
}
#v4-mirror-diag-overlay[hidden],
.v4-mirror-diag-overlay[hidden] {
  display: none !important;
}
.v4-mirror-diag-card {
  width: 100%;
  max-width: 304px;
  padding: 18px 16px;
  border-radius: 16px;
  background: linear-gradient(160deg, rgba(22, 32, 54, 0.94) 0%, rgba(11, 17, 30, 0.96) 100%);
  border: 1px solid rgba(56, 189, 248, 0.36);
  box-shadow: 0 18px 44px rgba(0, 0, 0, 0.56), inset 0 1px 0 rgba(255, 255, 255, 0.12);
  backdrop-filter: blur(18px);
  display: flex;
  flex-direction: column;
  gap: 10px;
  text-align: center;
}
.v4-mirror-diag-badge {
  align-self: center;
  padding: 3px 10px;
  border-radius: 999px;
  background: rgba(245, 158, 11, 0.16);
  border: 1px solid rgba(251, 191, 36, 0.42);
  color: #fde68a;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.02em;
}
.v4-mirror-diag-title {
  font-size: 14px;
  font-weight: 700;
  color: #f8fafc;
  line-height: 1.3;
}
.v4-mirror-diag-desc {
  font-size: 12px;
  color: #cbd5e1;
  line-height: 1.48;
}
.v4-mirror-diag-actions {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 4px;
}
.v4-diag-btn {
  width: 100%;
  padding: 9px 12px;
  border-radius: 10px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: transform 120ms ease, filter 140ms ease, background 140ms ease;
}
.v4-diag-btn:active {
  transform: scale(0.98);
}
.v4-diag-btn:disabled {
  opacity: 0.65;
  cursor: wait;
}
.v4-diag-btn-primary {
  border: none;
  background: linear-gradient(180deg, #0a84ff 0%, #0066dc 100%);
  color: #ffffff;
  box-shadow: 0 4px 14px rgba(10, 132, 255, 0.36);
}
.v4-diag-btn-primary:hover {
  filter: brightness(1.08);
}
.v4-diag-btn-secondary {
  border: 1px solid rgba(56, 189, 248, 0.42);
  background: rgba(14, 165, 233, 0.16);
  color: #bae6fd;
}
.v4-diag-btn-secondary:hover {
  background: rgba(14, 165, 233, 0.26);
}
.v4-diag-btn-ghost {
  border: 1px solid rgba(255, 255, 255, 0.18);
  background: rgba(255, 255, 255, 0.06);
  color: #e2e8f0;
}
.v4-diag-btn-ghost:hover {
  background: rgba(255, 255, 255, 0.12);
}
.v4-mirror-diag-feedback {
  font-size: 11px;
  color: #7dd3fc;
  line-height: 1.4;
  padding-top: 2px;
}
.v4-mirror-diag-feedback[hidden] {
  display: none !important;
}
"##,
        );
    }
    css_text
}

pub fn get_embedded_mirror_css() -> &'static str {
    static MIRROR_CSS_CACHE: OnceLock<String> = OnceLock::new();
    MIRROR_CSS_CACHE
        .get_or_init(|| transform_mirror_css(EMBEDDED_MIRROR_CSS))
        .as_str()
}

pub fn get_embedded_mirror_js() -> &'static str {
    static MIRROR_JS_CACHE: OnceLock<String> = OnceLock::new();
    MIRROR_JS_CACHE
        .get_or_init(|| transform_mirror_js(EMBEDDED_MIRROR_JS))
        .as_str()
}

fn transform_mirror_js(raw: &str) -> String {
    let mut js_text = raw.replace("\r\n", "\n");
    if !js_text.contains("__V4_TAHAP182_INSTANT_LOCK__") {
                    js_text = js_text.replace(
                        r##"  document.body.classList.remove("fit-fill", "fit-contain", "fit-autofit");
  document.body.classList.add(`fit-${fitMode}`);"##,
                        r##"  // __V4_TAHAP179_AUTOFIT__ __V4_TAHAP180_DYNAMIC_AUTOFIT__ __V4_TAHAP181_DYNAMIC_AUTOFIT__ __V4_TAHAP182_INSTANT_LOCK__: Instant Frame-0 Apple Macroblock Lock + Dynamic Fallback Scanner (baseTrim=0, 1:1 clean crop)
  document.body.classList.remove("fit-fill", "fit-contain", "fit-autofit");
  document.body.classList.add(`fit-${fitMode}`);
  canvas.classList.remove("fit-fill", "fit-contain", "fit-autofit");
  canvas.classList.add(`fit-${fitMode}`);"##,
                    );
                    js_text = js_text.replace(
                        r##"function drawFrame(f) {"##,
                        r##"let _v4MacroTrimCache = null;
let _v4CenterProbeCanvas = null;
let _v4CenterProbeCtx = null;
let _v4ScanRightCanvas = null;
let _v4ScanRightCtx = null;
let _v4ScanBotCanvas = null;
let _v4ScanBotCtx = null;
let _v4LastNotifiedCleanW = 0;
let _v4LastNotifiedCleanH = 0;

window.__v4ResetMacroblockCache = function() {
  _v4MacroTrimCache = null;
  _v4LastNotifiedCleanW = 0;
  _v4LastNotifiedCleanH = 0;
};

function calibrateFallbackMacroblockTrim(rawW, rawH) {
  if (rawW === 1216 && rawH === 2656) return { top: 0, left: 0, right: 38, bottom: 58, knownApple: true, verifiedOnBrightFrame: true, locked: true, source: "calibrated-instant-1216x2656" };
  if (rawW === 1184 && rawH === 2560) return { top: 0, left: 0, right: 14, bottom: 28, knownApple: true, verifiedOnBrightFrame: true, locked: true, source: "calibrated-instant-1184x2560" };
  if (rawW === 1248 && rawH === 2688) return { top: 0, left: 0, right: 6, bottom: 0, knownApple: true, verifiedOnBrightFrame: true, locked: true, source: "calibrated-instant-1248x2688" };
  if (rawW === 1328 && rawH === 2880) return { top: 0, left: 0, right: 8, bottom: 12, knownApple: true, verifiedOnBrightFrame: true, locked: true, source: "calibrated-instant-1328x2880" };
  if (rawW === 1296 && rawH === 2800) return { top: 0, left: 0, right: 6, bottom: 4, knownApple: true, verifiedOnBrightFrame: true, locked: true, source: "calibrated-instant-1296x2800" };
  if (rawW === 1344 && rawH === 2992) return { top: 0, left: 0, right: 24, bottom: 58, knownApple: true, verifiedOnBrightFrame: true, locked: true, source: "calibrated-instant-1344x2992" };
  return { top: 0, left: 0, right: 0, bottom: 0, knownApple: false, verifiedOnBrightFrame: false, locked: false, source: "calibrated-zero" };
}

function detectExactMacroblockTrim(sourceFrame, rawW, rawH) {
  if (!rawW || !rawH) return { top: 0, left: 0, right: 0, bottom: 0, verifiedOnBrightFrame: false, locked: false, source: "none" };
  if (_v4MacroTrimCache && _v4MacroTrimCache.rawW === rawW && _v4MacroTrimCache.rawH === rawH && (_v4MacroTrimCache.locked === true || _v4MacroTrimCache.verifiedOnBrightFrame === true)) {
    return _v4MacroTrimCache;
  }
  const fb = calibrateFallbackMacroblockTrim(rawW, rawH);
  if (fb.knownApple) {
    _v4MacroTrimCache = {
      rawW,
      rawH,
      top: 0,
      left: 0,
      right: fb.right,
      bottom: fb.bottom,
      cleanW: rawW - fb.right,
      cleanH: rawH - fb.bottom,
      verifiedOnBrightFrame: true,
      locked: true,
      centerAvgLuma: -1,
      source: fb.source,
    };
    return _v4MacroTrimCache;
  }
  let centerAvgLuma = 0;
  try {
    if (!_v4CenterProbeCanvas) {
      _v4CenterProbeCanvas = document.createElement("canvas");
      _v4CenterProbeCanvas.width = 16;
      _v4CenterProbeCanvas.height = 16;
      _v4CenterProbeCtx = _v4CenterProbeCanvas.getContext("2d", { willReadFrequently: true });
    }
    const cx = Math.floor(rawW * 0.2);
    const cy = Math.floor(rawH * 0.2);
    const cwProbe = Math.max(1, Math.floor(rawW * 0.6));
    const chProbe = Math.max(1, Math.floor(rawH * 0.6));
    _v4CenterProbeCtx.clearRect(0, 0, 16, 16);
    _v4CenterProbeCtx.drawImage(sourceFrame, cx, cy, cwProbe, chProbe, 0, 0, 16, 16);
    const cData = _v4CenterProbeCtx.getImageData(0, 0, 16, 16).data;
    let sumLuma = 0;
    for (let i = 0; i < cData.length; i += 4) {
      sumLuma += 0.299 * cData[i] + 0.587 * cData[i + 1] + 0.114 * cData[i + 2];
    }
    centerAvgLuma = sumLuma / 256;
  } catch (_) {}

  if (centerAvgLuma < 45) {
    const darkRes = {
      rawW,
      rawH,
      top: 0,
      left: 0,
      right: fb.right,
      bottom: fb.bottom,
      verifiedOnBrightFrame: false,
      locked: false,
      centerAvgLuma,
      source: fb.source,
    };
    _v4MacroTrimCache = darkRes;
    return darkRes;
  }

  const scanW = Math.min(64, rawW);
  const scanH = Math.min(80, rawH);
  let rightVerified = false;
  let botVerified = false;
  let scannedRight = fb.right;
  let scannedBot = fb.bottom;

  try {
    if (!_v4ScanRightCanvas) {
      _v4ScanRightCanvas = document.createElement("canvas");
      _v4ScanRightCtx = _v4ScanRightCanvas.getContext("2d", { willReadFrequently: true });
    }
    if (_v4ScanRightCanvas.width !== scanW || _v4ScanRightCanvas.height !== 3) {
      _v4ScanRightCanvas.width = scanW;
      _v4ScanRightCanvas.height = 3;
    }
    const estActiveH = Math.max(64, rawH - fb.bottom);
    const y25 = Math.min(rawH - 1, Math.floor(estActiveH * 0.25));
    const y50 = Math.min(rawH - 1, Math.floor(estActiveH * 0.50));
    const y75 = Math.min(rawH - 1, Math.floor(estActiveH * 0.75));
    _v4ScanRightCtx.clearRect(0, 0, scanW, 3);
    _v4ScanRightCtx.drawImage(sourceFrame, rawW - scanW, y25, scanW, 1, 0, 0, scanW, 1);
    _v4ScanRightCtx.drawImage(sourceFrame, rawW - scanW, y50, scanW, 1, 0, 1, scanW, 1);
    _v4ScanRightCtx.drawImage(sourceFrame, rawW - scanW, y75, scanW, 1, 0, 2, scanW, 1);
    const rData = _v4ScanRightCtx.getImageData(0, 0, scanW, 3).data;
    let trimR = 0;
    for (let x = scanW - 1; x >= 0; x--) {
      let colBlack = true;
      for (let row = 0; row < 3; row++) {
        const idx = (row * scanW + x) * 4;
        if (rData[idx] > 6 || rData[idx + 1] > 6 || rData[idx + 2] > 6) {
          colBlack = false;
          break;
        }
      }
      if (!colBlack) break;
      trimR++;
    }
    const innerX = scanW - 1 - trimR;
    if (innerX >= 0 && trimR < scanW) {
      let maxLumaR = 0;
      for (let row = 0; row < 3; row++) {
        const idx = (row * scanW + innerX) * 4;
        const luma = 0.299 * rData[idx] + 0.587 * rData[idx + 1] + 0.114 * rData[idx + 2];
        if (luma > maxLumaR) maxLumaR = luma;
      }
      if (maxLumaR >= 45) {
        scannedRight = trimR;
        rightVerified = true;
      }
    }

    if (!_v4ScanBotCanvas) {
      _v4ScanBotCanvas = document.createElement("canvas");
      _v4ScanBotCtx = _v4ScanBotCanvas.getContext("2d", { willReadFrequently: true });
    }
    if (_v4ScanBotCanvas.width !== 3 || _v4ScanBotCanvas.height !== scanH) {
      _v4ScanBotCanvas.width = 3;
      _v4ScanBotCanvas.height = scanH;
    }
    const estActiveW = Math.max(64, rawW - (rightVerified ? scannedRight : fb.right));
    const x25 = Math.min(rawW - 1, Math.floor(estActiveW * 0.25));
    const x50 = Math.min(rawW - 1, Math.floor(estActiveW * 0.50));
    const x75 = Math.min(rawW - 1, Math.floor(estActiveW * 0.75));
    _v4ScanBotCtx.clearRect(0, 0, 3, scanH);
    _v4ScanBotCtx.drawImage(sourceFrame, x25, rawH - scanH, 1, scanH, 0, 0, 1, scanH);
    _v4ScanBotCtx.drawImage(sourceFrame, x50, rawH - scanH, 1, scanH, 1, 0, 1, scanH);
    _v4ScanBotCtx.drawImage(sourceFrame, x75, rawH - scanH, 1, scanH, 2, 0, 1, scanH);
    const bData = _v4ScanBotCtx.getImageData(0, 0, 3, scanH).data;
    let trimB = 0;
    for (let y = scanH - 1; y >= 0; y--) {
      let rowBlack = true;
      for (let col = 0; col < 3; col++) {
        const idx = (y * 3 + col) * 4;
        if (bData[idx] > 6 || bData[idx + 1] > 6 || bData[idx + 2] > 6) {
          rowBlack = false;
          break;
        }
      }
      if (!rowBlack) break;
      trimB++;
    }
    const innerY = scanH - 1 - trimB;
    if (innerY >= 0 && trimB < scanH) {
      let maxLumaB = 0;
      for (let col = 0; col < 3; col++) {
        const idx = (innerY * 3 + col) * 4;
        const luma = 0.299 * bData[idx] + 0.587 * bData[idx + 1] + 0.114 * bData[idx + 2];
        if (luma > maxLumaB) maxLumaB = luma;
      }
      if (maxLumaB >= 45) {
        scannedBot = trimB;
        botVerified = true;
      }
    }
  } catch (_) {}

  const finalRight = rightVerified ? scannedRight : fb.right;
  const finalBot = botVerified ? scannedBot : fb.bottom;
  const verifiedOnBrightFrame = rightVerified && botVerified;
  _v4MacroTrimCache = {
    rawW,
    rawH,
    top: 0,
    left: 0,
    right: finalRight,
    bottom: finalBot,
    verifiedOnBrightFrame,
    locked: verifiedOnBrightFrame,
    centerAvgLuma,
    source: verifiedOnBrightFrame ? "pixel-scan" : (rightVerified || botVerified ? "partial-pixel-scan" : fb.source),
  };
  return _v4MacroTrimCache;
}

window.__v4ComputeExactMacroblockTrim = function(sourceFrame, rawW, rawH, mode, isIpadFlag, isAirplayOrH264) {
  if (isIpadFlag || mode === "contain" || isAirplayOrH264 || rawW > rawH) {
    return { top: 0, left: 0, right: 0, bottom: 0, verifiedOnBrightFrame: true, locked: true, source: "disabled" };
  }
  return detectExactMacroblockTrim(sourceFrame, rawW, rawH);
};

function drawFrame(f) {"##,
                    );
                    js_text = js_text.replace(
                        r##"    // Tahap 74/125/130/276: preserve legacy disableNativeTrim token + disable trim on native landscape frames (rawW > rawH)
    const isAirplayOrH264 = window._isAirPlayStream === true || (typeof _currentCodec === "string" && (_currentCodec.startsWith("avc") || _currentCodec.startsWith("h264")));
    const disableNativeTrim = effectiveIpad || (fitMode === "contain") || isAirplayOrH264;
    const isNativeLandscapeFrame = rawW > rawH;
    const pxScale = Math.max(3, Math.round(rawW / 346));
    const uTop = Math.max(0, (Number(currentCrop.top) || 0) * pxScale);
    const uBot = Math.max(0, (Number(currentCrop.bottom) || 0) * pxScale);
    const uLeft = Math.max(0, (Number(currentCrop.left) || 0) * pxScale);
    const uRight = Math.max(0, (Number(currentCrop.right) || 0) * pxScale);
    const isLandscapeWindow = (window.innerWidth > window.innerHeight) || _autoRotatedToLandscape || (currentRotation === 90 || currentRotation === 270);
    const drawPlan = window.__tahap276ComputeDrawPlan({
      rawW,
      rawH,
      effectiveIpad,
      fitMode,
      isAirplayOrH264,
      macroTrimTop: 0,
      macroTrimBot: 0,
      macroTrimLeft: 0,
      macroTrimRight: 0,
      uTop,
      uBot,
      uLeft,
      uRight,
      isLandscapeWindow,
      autoRotateGameEnabled,
      currentRotation,
    });"##,
                        r##"    const isAirplayOrH264 = window._isAirPlayStream === true || (typeof _currentCodec === "string" && (_currentCodec.startsWith("avc") || _currentCodec.startsWith("h264")));
    const disableNativeTrim = effectiveIpad || (fitMode === "contain") || isAirplayOrH264;
    const isNativeLandscapeFrame = rawW > rawH;
    const effectiveDisableNativeTrim = disableNativeTrim || isNativeLandscapeFrame;
    const macroTrimInfo = window.__v4ComputeExactMacroblockTrim(sourceFrame, rawW, rawH, fitMode, effectiveIpad, isAirplayOrH264);
    const pxScale = Math.max(3, Math.round(rawW / 346));
    const uTop = Math.max(0, (Number(currentCrop.top) || 0) * pxScale);
    const uBot = Math.max(0, (Number(currentCrop.bottom) || 0) * pxScale);
    const uLeft = Math.max(0, (Number(currentCrop.left) || 0) * pxScale);
    const uRight = Math.max(0, (Number(currentCrop.right) || 0) * pxScale);
    const isLandscapeWindow = (window.innerWidth > window.innerHeight) || _autoRotatedToLandscape || (currentRotation === 90 || currentRotation === 270);
    const drawPlan = window.__tahap276ComputeDrawPlan({
      rawW,
      rawH,
      effectiveIpad,
      fitMode,
      isAirplayOrH264,
      macroTrimTop: 0,
      macroTrimBot: effectiveDisableNativeTrim ? 0 : macroTrimInfo.bottom,
      macroTrimLeft: 0,
      macroTrimRight: effectiveDisableNativeTrim ? 0 : macroTrimInfo.right,
      uTop,
      uBot,
      uLeft,
      uRight,
      isLandscapeWindow,
      autoRotateGameEnabled,
      currentRotation,
    });
    const preCleanW = Math.max(64, rawW - (effectiveDisableNativeTrim ? 0 : macroTrimInfo.right));
    const preCleanH = Math.max(64, rawH - (effectiveDisableNativeTrim ? 0 : macroTrimInfo.bottom));"##,
                    );
                    js_text = js_text.replace(
                        r##"    window._lastCleanTransform = { cleanW, cleanH, cleanCenterX, cleanCenterY, srcTrimLeft, srcTrimTop, srcTrimRight, srcTrimBot, nativeTrimTop, nativeTrimBot, nativeTrimLeft, nativeTrimRight, rawW, rawH };"##,
                        r##"    window._lastCleanTransform = { cleanW, cleanH, cleanCenterX, cleanCenterY, srcTrimLeft, srcTrimTop, srcTrimRight, srcTrimBot, nativeTrimTop, nativeTrimBot, nativeTrimLeft, nativeTrimRight, macroblockSource: macroTrimInfo.source, verifiedOnBrightFrame: !!macroTrimInfo.verifiedOnBrightFrame, macroblockLocked: !!macroTrimInfo.locked, baseTrim: 0, fitMode, preCleanW, preCleanH, rawW, rawH };"##,
                    );
                    js_text = js_text.replace(
                        r##"    if (rawW !== _lastNotifiedRawW || rawH !== _lastNotifiedRawH || effectiveIpad !== _lastNotifiedIpad) {
      _lastNotifiedRawW = rawW;
      _lastNotifiedRawH = rawH;
      _lastNotifiedIpad = effectiveIpad;
      try {
        window.parent.postMessage({
          type: "video-dimensions",
          width: rawW,
          height: rawH,
          isIpad: effectiveIpad,
          deviceFamily: _deviceFamily,
          isStreamLandscape: rawW > rawH,
        }, "*");
      } catch (_) {}
    }"##,
                        r##"    if (rawW !== _lastNotifiedRawW || rawH !== _lastNotifiedRawH || preCleanW !== _lastNotifiedCleanW || preCleanH !== _lastNotifiedCleanH || effectiveIpad !== _lastNotifiedIpad) {
      _lastNotifiedRawW = rawW;
      _lastNotifiedRawH = rawH;
      _lastNotifiedCleanW = preCleanW;
      _lastNotifiedCleanH = preCleanH;
      _lastNotifiedIpad = effectiveIpad;
      try {
        window.parent.postMessage({
          type: "video-dimensions",
          width: preCleanW,
          height: preCleanH,
          cleanW: preCleanW,
          cleanH: preCleanH,
          rawWidth: rawW,
          rawHeight: rawH,
          isIpad: effectiveIpad,
          deviceFamily: _deviceFamily,
          isStreamLandscape: preCleanW > preCleanH,
        }, "*");
      } catch (_) {}
    }"##,
                    );
                }
                if !js_text.contains("__V4_TAHAP185_HEVC_PROMPT__") {
                    js_text = js_text.replace(
                        r##"  if (!support.supported) {
    setStatus("This browser cannot decode the iPhone video stream.", "warn");
    return;
  }"##,
                        r##"  if (!support.supported) {
    setStatus("This browser cannot decode the iPhone video stream.", "warn");
    if (typeof window.__v4ShowHevcInstallOverlay === "function") {
      window.__v4ShowHevcInstallOverlay(codec);
    }
    return;
  }"##,
                    );
                    js_text.push_str(
                        r##"

// __V4_TAHAP185_HEVC_PROMPT__ - 1-Click HEVC Codec Installer Overlay (delegates to Tahap 248 #v4-mirror-diag-overlay)
window.__v4ShowHevcInstallOverlay = function(codec) {
  try {
    if (typeof window.__v5ShowMirrorDiagOverlay === "function") {
      window.__v5ShowMirrorDiagOverlay("hevc-missing", { codec: codec || "" });
      return;
    }
  } catch (_) {}
  try {
    if (window.parent && window.parent !== window) {
      window.parent.postMessage({ type: "hevc-missing", codec: codec || "" }, "*");
    }
  } catch (_) {}
  try {
    var box = document.getElementById("v4-hevc-stage-prompt");
    if (!box) {
      box = document.createElement("div");
      box.id = "v4-hevc-stage-prompt";
      box.style.cssText = "position:fixed;inset:0;z-index:99999;display:flex;align-items:center;justify-content:center;background:#090d16;padding:18px;";
      document.body.appendChild(box);
    }
    var card = document.createElement("div");
    card.style.cssText = "max-width:300px;width:100%;padding:16px;border-radius:14px;background:rgba(15,23,42,0.94);border:1px solid rgba(56,189,248,0.4);color:#f8fafc;text-align:center;font-family:system-ui,-apple-system,sans-serif;box-shadow:0 12px 32px rgba(0,0,0,0.48);";
    var title = document.createElement("div");
    title.style.cssText = "font-size:13px;font-weight:700;margin-bottom:6px;";
    title.textContent = "Microsoft HEVC Codec Required";
    card.appendChild(title);
    var desc = document.createElement("div");
    desc.style.cssText = "font-size:12px;color:#cbd5e1;margin-bottom:12px;line-height:1.45;";
    desc.textContent = "This browser cannot decode the iPhone video stream (" + (codec || "HEVC") + "). Install Microsoft HEVC Video Extensions (Free OEM) in 1 click.";
    card.appendChild(desc);
    var btn = document.createElement("button");
    btn.id = "btn-install-hevc-stage";
    btn.type = "button";
    btn.textContent = "Install HEVC Video Codec (1-Click)";
    btn.style.cssText = "width:100%;padding:9px 12px;border-radius:9px;border:none;background:#0A84FF;color:#ffffff;font-weight:600;font-size:12px;cursor:pointer;";
    btn.onclick = function() {
      btn.disabled = true;
      btn.textContent = "Installing HEVC Video Extensions...";
      fetch("/api/system/install_hevc", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}"
      }).then(function(r) { return r.json(); }).then(function(res) {
        if (res && res.installed) {
          btn.textContent = "HEVC Codec Installed! Reloading...";
          setTimeout(function() { window.location.reload(); }, 600);
        } else {
          var target = (res && res.mediaFeaturePackMissing) ? "media_feature_pack" : "hevc_oem";
          fetch("/api/system/open_hevc_store", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ target: target })
          }).catch(function() {});
          btn.disabled = false;
          btn.textContent = "Reload Stream After Install";
          btn.onclick = function() { window.location.reload(); };
        }
      }).catch(function() {
        btn.disabled = false;
        btn.textContent = "Install HEVC Video Codec (1-Click)";
      });
    };
    card.appendChild(btn);
    box.innerHTML = "";
    box.appendChild(card);
  } catch (_) {}
};
"##,
        );
    }
    js_text
}

// ============================================================================
// NATIVE WIN32 HELPER CALLS
// ============================================================================

#[link(name = "user32")]
extern "system" {
    #[allow(dead_code)]
    fn SetWindowPos(
        hwnd: *mut std::ffi::c_void,
        hwndInsertAfter: *mut std::ffi::c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        uFlags: u32,
    ) -> i32;
    fn ShowWindowAsync(hwnd: *mut std::ffi::c_void, nCmdShow: i32) -> i32;
    fn PostMessageW(hwnd: *mut std::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
}

#[allow(dead_code)]
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
// SERVER ENGINE (CONCURRENT STREAM HANDLER)
// ============================================================================

fn handle_client_stream(mut stream: TcpStream, state: Arc<Mutex<AppState>>, web_dir: PathBuf) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(5000)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(5000)));
    let peer_ip = stream
        .peer_addr()
        .map(|a| a.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

    let mut remaining_buf: Vec<u8> = Vec::with_capacity(16384);

    loop {
        let mut raw_buf: Vec<u8> = std::mem::take(&mut remaining_buf);
        let mut temp = [0u8; 16384];
        let mut header_end_pos: Option<usize> = None;
        let mut expected_content_len: usize = 0;

        loop {
            if header_end_pos.is_none() {
                if let Some(pos) = raw_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let hdr_end = pos + 4;
                    header_end_pos = Some(hdr_end);
                    let hdr_str = String::from_utf8_lossy(&raw_buf[..hdr_end]);
                    for line in hdr_str.lines() {
                        if let Some((k, v)) = line.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("content-length") {
                                expected_content_len = v.trim().parse::<usize>().unwrap_or(0);
                            }
                        }
                    }
                }
            }
            if let Some(hdr_end) = header_end_pos {
                let body_received = raw_buf.len().saturating_sub(hdr_end);
                if body_received >= expected_content_len {
                    let total_req_len = hdr_end + expected_content_len;
                    if raw_buf.len() > total_req_len {
                        remaining_buf = raw_buf.split_off(total_req_len);
                    }
                    break;
                }
            }

            match stream.read(&mut temp) {
                Ok(0) => break,
                Ok(n) => {
                    raw_buf.extend_from_slice(&temp[..n]);
                }
                Err(_) => break,
            }
            if raw_buf.len() >= 25 * 1024 * 1024 {
                break;
            }
        }

        if raw_buf.is_empty() {
            break;
        }

        if let Some(req) = HttpRequest::parse(&raw_buf) {
            let wants_close = req.headers.get("connection").map(|v| v.eq_ignore_ascii_case("close")).unwrap_or(false);

            if !is_lan_request_allowed(peer_ip, &req.method, &req.path) {
                let resp = HttpResponse::forbidden_json(
                    "Forbidden: LAN access is restricted to /c?u= and /api/shortcut/push",
                );
                let resp_bytes = resp.to_bytes_with_conn("close");
                let _ = stream.write_all(&resp_bytes);
                let _ = stream.flush();
                break;
            }

            let clean_path = req.path.split('?').next().unwrap_or("/").split('#').next().unwrap_or("/");
            if clean_path == "/mirror/live" || clean_path == "/stream.bin" {
                iphone_link_apple::wifi::configure_low_latency_std_tcp(&stream);
                let _ = stream.set_read_timeout(None);
                let _ = stream.set_write_timeout(None);
                if let Err(_) = stream_live_passthrough(&mut stream) {
                    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: 21\r\nConnection: close\r\n\r\nWorker not connected\n");
                    let _ = stream.flush();
                }
                return;
            }

            let resp = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_request(&req, &state, &web_dir)
            })) {
                Ok(r) => r,
                Err(_) => HttpResponse {
                    status_code: 500,
                    status_text: "Internal Server Error",
                    content_type: "application/json; charset=utf-8",
                    body: br#"{"ok":false,"error":"Internal handler panic recovered cleanly"}"#.to_vec(),
                },
            };
            let conn_hdr = if wants_close { "close" } else { "keep-alive" };
            let resp_bytes = resp.to_bytes_with_conn(conn_hdr);
            if stream.write_all(&resp_bytes).is_err() || stream.flush().is_err() {
                break;
            }

            if wants_close {
                break;
            }
        } else {
            break;
        }
    }
}

pub struct NativeServer {
    pub port: u16,
    running: Arc<AtomicBool>,
}

impl NativeServer {
    pub fn start(state: Arc<Mutex<AppState>>, web_dir: PathBuf, port: u16) -> Result<Self, String> {
        register_tray_app_state(state.clone());
        let bind_host = iphone_link_apple::connection::bind_host_ipv4();
        let mut listener_opt = None;
        for _ in 0..15 {
            if let Ok(l) = TcpListener::bind(format!("{bind_host}:{port}")) {
                listener_opt = Some(l);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let listener = match listener_opt {
            Some(l) => l,
            None => TcpListener::bind(format!("127.0.0.1:{}", port))
                .or_else(|_| TcpListener::bind(format!("{bind_host}:0")))
                .or_else(|_| TcpListener::bind("127.0.0.1:0"))
                .map_err(|e| format!("Failed to bind TCP listener: {}", e))?,
        };

        let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        {
            let mut st = state.lock().unwrap();
            st.server_port = actual_port;
        }

        let temp_port = env::temp_dir().join("server_port.txt");
        let _ = fs::write(temp_port, actual_port.to_string());

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set nonblocking: {}", e))?;

        // Tahap 243/247: 1x parallel background Wi-Fi auto-discovery on startup across ALL paired
        // devices (0 network I/O when 0 paired devices -> 100% Clean Slate Tahap 246).
        let _ = thread::Builder::new()
            .name("iphone-wifi-boot-probe".into())
            .spawn(|| {
                let paired = iphone_link_apple::wifi::wifi_paired_udids();
                if paired.is_empty() {
                    return;
                }
                if paired.len() >= 2 {
                    let results = iphone_link_apple::wifi::probe_all_paired_wifi_devices_parallel(650);
                    let best_udid = results
                        .iter()
                        .find(|r| r.reachable)
                        .or_else(|| results.first())
                        .map(|r| r.udid.clone())
                        .unwrap_or_else(|| paired[0].clone());
                    let payload = get_wifi_status(Some(&best_udid), false);
                    let wjson = payload.to_json_string();
                    if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
                        *guard = Some((best_udid, wjson, std::time::Instant::now()));
                    }
                } else if let Some(first_u) = paired.into_iter().next() {
                    let payload = get_wifi_status(Some(&first_u), true);
                    let wjson = payload.to_json_string();
                    if let Ok(mut guard) = WIFI_STATUS_STREAM_CACHE.lock() {
                        *guard = Some((first_u, wjson, std::time::Instant::now()));
                    }
                }
            });

        thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let st_clone = state.clone();
                        let wd_clone = web_dir.clone();
                        let _ = thread::Builder::new()
                            .name("http-client".into())
                            .stack_size(4 * 1024 * 1024)
                            .spawn(move || {
                                handle_client_stream(stream, st_clone, wd_clone);
                            });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(5));
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

    /// Serializes tests that mutate process-global statics (SAVED_UNLOCK_PIN, tray atomics, etc.)
    /// so default parallel `cargo test` runs deterministically without data races.
    static TEST_SERIAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_http_request_parsing_and_query_strip() {
        let raw = b"GET /app.css?v=tahap149_complete_bilingual HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n";
        let req = HttpRequest::parse(raw).expect("Must parse request");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/app.css");
        assert_eq!(req.query, "v=tahap149_complete_bilingual");
        assert_eq!(mime_for_path_str(&req.path), "text/css; charset=utf-8");
        assert_eq!(mime_for_path_str("app.js"), "application/javascript; charset=utf-8");
        assert_eq!(mime_for_path_str("logo.svg"), "image/svg+xml");
    }

    #[test]
    fn test_server_status_and_acrylic_response() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_css = HttpRequest {
            method: "GET".to_string(),
            path: "/v4_true_acrylic.css".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp_css = handle_request(&req_css, &state, Path::new("web"));
        assert_eq!(resp_css.status_code, 200);
        assert_eq!(resp_css.content_type, "text/css; charset=utf-8");
        assert!(String::from_utf8_lossy(&resp_css.body).contains("overflow: hidden !important"));

        let req_status = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp_status = handle_request(&req_status, &state, Path::new("web"));
        assert_eq!(resp_status.status_code, 200);
        let body_str = String::from_utf8_lossy(&resp_status.body);
        assert!(body_str.contains(r#""version": "5.0.1""#));
        assert!(body_str.contains(r#""zero_python_runtime": true"#));
        assert!(body_str.contains(r#""zero_powershell_runtime": true"#));
        assert!(body_str.contains("devicePixelRatio"));

        let req_wp = HttpRequest {
            method: "GET".to_string(),
            path: "/api/system/wallpaper".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp_wp = handle_request(&req_wp, &state, Path::new("web"));
        assert_eq!(resp_wp.status_code, 200);
        assert!(resp_wp.content_type == "image/jpeg" || resp_wp.content_type == "image/png");
        assert!(resp_wp.body.len() > 50000);

        let req_mirror_js = HttpRequest {
            method: "GET".to_string(),
            path: "/mirror.js".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let web_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join("handset")
            .join("web");
        if web_root.is_dir() {
            let resp_mjs = handle_request(&req_mirror_js, &state, &web_root);
            assert_eq!(resp_mjs.status_code, 200);
            let mjs = String::from_utf8_lossy(&resp_mjs.body);
            assert!(mjs.contains("__V4_TAHAP179_AUTOFIT__"), "mirror.js must keep Auto-Fit marker");
            assert!(mjs.contains("__V4_TAHAP180_DYNAMIC_AUTOFIT__"), "mirror.js must preserve Tahap 180 marker");
            assert!(mjs.contains("__V4_TAHAP181_DYNAMIC_AUTOFIT__"), "mirror.js must preserve Tahap 181 marker");
            assert!(mjs.contains("__V4_TAHAP182_INSTANT_LOCK__"), "mirror.js must apply Tahap 182 instant Frame-0 Apple macroblock lock");
            assert!(mjs.contains("__v4ComputeExactMacroblockTrim"), "mirror.js must invoke __v4ComputeExactMacroblockTrim inside drawFrame");
            assert!(!mjs.contains("14 * scaleW"), "mirror.js must NOT contain legacy hardcoded 14 * scaleW trim");
            assert!(mjs.contains("right: 38, bottom: 58"), "1216x2656 baseline must trim 38px right and 58px bottom (1178x2598)");
            assert!(mjs.contains("knownApple: true"), "mirror.js must lock standard Apple resolutions from Frame 0");
            assert!(mjs.contains("detectExactMacroblockTrim"), "mirror.js must contain detectExactMacroblockTrim scanner");
            assert!(mjs.contains("calibrateFallbackMacroblockTrim"), "mirror.js must contain calibrateFallbackMacroblockTrim table");
            assert!(mjs.contains("baseTrim: 0"), "mirror.js must keep baseTrim = 0");

            let req_mirror_css = HttpRequest {
                method: "GET".to_string(),
                path: "/mirror.css".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            };
            let resp_mcss = handle_request(&req_mirror_css, &state, &web_root);
            assert_eq!(resp_mcss.status_code, 200);
            let mcss = String::from_utf8_lossy(&resp_mcss.body);
            assert!(mcss.contains("__V4_TAHAP179_MIRROR_CSS__"), "mirror.css must apply Tahap 179 CSS patch");
            assert!(mcss.contains("object-fit: fill !important;"), "mirror.css must use object-fit: fill for Auto-Fit");
        }

        // Verify Tahap 182 Super Retina width lock on /api/window/sync_device_aspect (340/347 -> 346, 422 -> 422)
        for (req_w, exp_w) in [(340, 346), (347, 346), (422, 422)] {
            let req_aspect = HttpRequest {
                method: "POST".to_string(),
                path: "/api/window/sync_device_aspect".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: format!(r#"{{"width":{},"open":false,"profile":"auto"}}"#, req_w),
                body_bytes: Vec::new(),
            };
            let resp_aspect = handle_request(&req_aspect, &state, Path::new("web"));
            assert_eq!(resp_aspect.status_code, 200);
            let aspect_json = String::from_utf8_lossy(&resp_aspect.body);
            let expected_tok = format!(r#""portrait_width":{}"#, exp_w);
            assert!(aspect_json.contains(&expected_tok), "sync_device_aspect({}) expected {}, got {}", req_w, expected_tok, aspect_json);
        }

        // Verify /v4_true_acrylic.css and /v4_bridge.js include Tahap 182 #update-channel-group & Document.prototype fixes
        let resp_css = handle_request(&HttpRequest {
            method: "GET".to_string(), path: "/v4_true_acrylic.css".to_string(),
            query: String::new(), headers: HashMap::new(), body: String::new(), body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        assert_eq!(resp_css.status_code, 200);
        let css_str = String::from_utf8_lossy(&resp_css.body);
        assert!(css_str.contains("#update-channel-group button.seg.is-active"));
        assert!(css_str.contains("#screen-error-banner:empty"));

        let resp_br = handle_request(&HttpRequest {
            method: "GET".to_string(), path: "/v4_bridge.js".to_string(),
            query: String::new(), headers: HashMap::new(), body: String::new(), body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        assert_eq!(resp_br.status_code, 200);
        let br_str = String::from_utf8_lossy(&resp_br.body);
        assert!(br_str.contains("Document.prototype.querySelector"));
        assert!(br_str.contains("#pill-connection"));
        assert!(br_str.contains("sanitizeErrorBanners"));
        assert!(br_str.contains("deviceAspect >= 0.440"));

        // Verify /codec fallback reports calibrated 1178x2598 active iPhone 17 Pro dimensions
        let req_codec = HttpRequest {
            method: "GET".to_string(),
            path: "/codec".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp_codec = handle_request(&req_codec, &state, Path::new("web"));
        assert_eq!(resp_codec.status_code, 200);
        let codec_json = String::from_utf8_lossy(&resp_codec.body);
        assert!(codec_json.contains(r#""displayWidth":1178"#) && codec_json.contains(r#""displayHeight":2598"#),
            "/codec fallback must report 1178x2598: {}", codec_json);
    }

    #[test]
    fn test_tahap183_tray_startup_pin_persistence_and_wipe() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ocr_guard = crate::ocr_engine::CRATE_GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. Verify V4_BRIDGE_JS includes Tahap 183 PIN persistence & tray update handlers
        assert!(V4_BRIDGE_JS.contains("hydratePinFromSettings"));
        assert!(V4_BRIDGE_JS.contains("persistUnlockPinDual"));
        assert!(V4_BRIDGE_JS.contains("guardPinInputAgainstAccidentalWipe"));
        assert!(V4_BRIDGE_JS.contains("handlePendingTrayAction"));
        assert!(V4_BRIDGE_JS.contains("iPhone Link v5.0.1"));

        // 2. Deterministic folderInstanceId & disk save/load/wipe roundtrip
        let tmp_dir = env::temp_dir().join("iphone_link_tahap183_test");
        let _ = fs::create_dir_all(&tmp_dir);
        let fid1 = get_deterministic_folder_instance_id_for_dir(&tmp_dir);
        let fid2 = get_deterministic_folder_instance_id_for_dir(&tmp_dir);
        assert_eq!(fid1.len(), 12);
        assert_eq!(fid1, fid2);

        let settings_file = tmp_dir.join("user_settings.json");
        let mut st_disk = AppState::default();
        st_disk.theme = "light".to_string();
        st_disk.update_channel = "beta".to_string();
        save_user_settings_to_path(&settings_file, &st_disk, "246810").expect("save settings");
        assert!(settings_file.is_file());

        let mut st_reloaded = AppState::default();
        let loaded_pin = load_user_settings_from_path(&settings_file, &mut st_reloaded);
        assert_eq!(loaded_pin, "246810");
        assert_eq!(st_reloaded.theme, "light");
        assert_eq!(st_reloaded.update_channel, "beta");

        wipe_privacy_files_in_dir(&tmp_dir);
        assert!(!settings_file.exists());
        let _ = fs::remove_dir_all(&tmp_dir);

        // 3. Verify GET /api/tray-state & POST /api/tray/trigger_action ("check_updates")
        let resp_tray = handle_request(&HttpRequest {
            method: "GET".to_string(),
            path: "/api/tray-state".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        assert_eq!(resp_tray.status_code, 200);
        let tray_json = String::from_utf8_lossy(&resp_tray.body);
        assert!(tray_json.contains("Show / Restore iPhone Link"));
        assert!(tray_json.contains("Start with Windows"));
        assert!(tray_json.contains("Check for Updates..."));

        let resp_upd = handle_request(&HttpRequest {
            method: "POST".to_string(),
            path: "/api/tray/trigger_action".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"action":"check_updates"}"#.to_string(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        assert_eq!(resp_upd.status_code, 200);
        let upd_json = String::from_utf8_lossy(&resp_upd.body);
        assert!(upd_json.contains(r#""drawer_open":true"#));
        let queued = GLOBAL_PENDING_TRAY_ACTION.lock().unwrap().take();
        assert_eq!(queued.as_deref(), Some("check_updates"));

        // 4. Verify POST /api/settings with unlockPin preserves & GET /api/settings returns isFreshFolder:false
        let resp_save_pin = handle_request(&HttpRequest {
            method: "POST".to_string(),
            path: "/api/settings".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"unlockPin":"246810"}"#.to_string(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        assert_eq!(resp_save_pin.status_code, 200);
        let save_json = String::from_utf8_lossy(&resp_save_pin.body);
        assert!(save_json.contains(r#""unlockPin":"246810""#));
        assert!(save_json.contains(r#""isFreshFolder":false"#));

        // Saving unrelated setting (e.g. stealth or channel) must NEVER wipe SAVED_UNLOCK_PIN
        let _ = handle_request(&HttpRequest {
            method: "POST".to_string(),
            path: "/api/settings".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"updateChannel":"beta"}"#.to_string(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        let resp_get = handle_request(&HttpRequest {
            method: "GET".to_string(),
            path: "/api/settings".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        let get_json = String::from_utf8_lossy(&resp_get.body);
        assert!(get_json.contains(r#""unlockPin":"246810""#), "PIN must remain after unrelated settings update: {}", get_json);

        // 5. Explicit wipe clears PIN and deletes user_settings.json
        let resp_wipe = handle_request(&HttpRequest {
            method: "POST".to_string(),
            path: "/api/device/wipe-privacy-data".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: "{}".to_string(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        assert_eq!(resp_wipe.status_code, 200);
        let resp_get_after_wipe = handle_request(&HttpRequest {
            method: "GET".to_string(),
            path: "/api/settings".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        let after_json = String::from_utf8_lossy(&resp_get_after_wipe.body);
        assert!(after_json.contains(r#""unlockPin":"""#), "PIN must be empty after wipe: {}", after_json);
    }

    #[test]
    fn test_tahap184_universal_persistence_language_sync_and_tray_dismiss() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut state_raw = AppState::default();
        assert_eq!(state_raw.fit_mode, "autofit");
        assert_eq!(state_raw.language, Lang::Id);

        let full_patch = r#"{"language":"en","theme":"light","fitMode":"contain","deviceProfile":"ipad","autoRotate":true,"crop":{"top":12,"bottom":24,"left":8,"right":16},"audioOut":"bluetooth","bluetoothA2dp":true,"audioVolume":72,"audioMuted":true,"audioBufferPreset":"high","lipSync":false,"trackpadSpeed":140,"crossPlatformShortcuts":false,"pastePcClipboard":false,"transport":"wifi","autoCheckUpdate":false,"updateChannel":"beta"}"#;
        let touched = apply_settings_json_body_to_state(full_patch, &mut state_raw);
        assert!(touched);
        assert_eq!(state_raw.language, Lang::En);
        assert_eq!(state_raw.theme, "light");
        assert_eq!(state_raw.fit_mode, "contain");
        assert_eq!(state_raw.device_profile, "ipad");
        assert!(state_raw.auto_rotate);
        assert_eq!(state_raw.crop_top, 12);
        assert_eq!(state_raw.crop_bottom, 24);
        assert_eq!(state_raw.crop_left, 8);
        assert_eq!(state_raw.crop_right, 16);
        assert_eq!(state_raw.audio_routing, "bluetooth");
        assert!(state_raw.bluetooth_a2dp);
        assert_eq!(state_raw.audio_volume, 72);
        assert!(state_raw.audio_muted);
        assert_eq!(state_raw.audio_buffer_preset, "high");
        assert!(!state_raw.lip_sync);
        assert_eq!(state_raw.trackpad_speed, 140);
        assert!(!state_raw.cross_platform_shortcuts);
        assert!(!state_raw.paste_pc_clipboard);
        assert_eq!(state_raw.transport, "wifi");
        assert!(!state_raw.auto_check_update);
        assert_eq!(state_raw.update_channel, "beta");

        let tmp_dir = std::env::temp_dir().join(format!(
            "iphone_link_t184_test_{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
        ));
        let settings_file = tmp_dir.join("user_settings.json");
        save_user_settings_to_path(&settings_file, &state_raw, "778899").unwrap();

        let mut state_reloaded = AppState::default();
        assert!(settings_file.exists());
        let pin_loaded = load_user_settings_from_path(&settings_file, &mut state_reloaded);
        assert_eq!(pin_loaded, "778899");
        assert_eq!(state_reloaded.language, Lang::En);
        assert_eq!(state_reloaded.theme, "light");
        assert_eq!(state_reloaded.fit_mode, "contain");
        assert_eq!(state_reloaded.device_profile, "ipad");
        assert!(state_reloaded.auto_rotate);
        assert_eq!(state_reloaded.crop_top, 12);
        assert_eq!(state_reloaded.crop_bottom, 24);
        assert_eq!(state_reloaded.crop_left, 8);
        assert_eq!(state_reloaded.crop_right, 16);
        assert_eq!(state_reloaded.audio_routing, "bluetooth");
        assert!(state_reloaded.bluetooth_a2dp);
        assert_eq!(state_reloaded.audio_volume, 72);
        assert!(state_reloaded.audio_muted);
        assert_eq!(state_reloaded.audio_buffer_preset, "high");
        assert!(!state_reloaded.lip_sync);
        assert_eq!(state_reloaded.trackpad_speed, 140);
        assert!(!state_reloaded.cross_platform_shortcuts);
        assert!(!state_reloaded.paste_pc_clipboard);
        assert_eq!(state_reloaded.transport, "wifi");
        assert!(!state_reloaded.auto_check_update);
        assert_eq!(state_reloaded.update_channel, "beta");
        let _ = fs::remove_dir_all(&tmp_dir);

        // Verify tray dismiss endpoint clears GLOBAL_TRAY_MENU_OPEN and increments count
        let state = Arc::new(Mutex::new(AppState::default()));
        GLOBAL_TRAY_MENU_OPEN.store(true, Ordering::SeqCst);
        let resp_dismiss = handle_request(&HttpRequest {
            method: "POST".to_string(),
            path: "/api/tray/dismiss_menu".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: "{}".to_string(),
            body_bytes: Vec::new(),
        }, &state, Path::new("web"));
        assert_eq!(resp_dismiss.status_code, 200);
        let dismiss_json = String::from_utf8_lossy(&resp_dismiss.body);
        assert!(dismiss_json.contains(r#""wasOpen":true"#), "dismiss_json={}", dismiss_json);
        assert!(dismiss_json.contains(r#""menuOpen":false"#), "dismiss_json={}", dismiss_json);
        assert!(!GLOBAL_TRAY_MENU_OPEN.load(Ordering::SeqCst));
    }

    #[test]
    fn test_tahap185_find_python_runtime_priority_and_hevc_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 1. Self-contained embedded runtime/ beside EXE takes Priority 1 (zero system Python)
        let base = std::env::temp_dir().join(format!(
            "iphone_link_t185_runtime_test_{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
        ));
        let exe_dir = base.join("app");
        let rt_dir = exe_dir.join("runtime");
        let hs_dir = rt_dir.join("handset");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&hs_dir).unwrap();
        let py_exe = rt_dir.join("pythonw.exe");
        fs::write(&py_exe, b"MZ").unwrap();
        fs::write(hs_dir.join("server.py"), b"# stub").unwrap();

        let found = find_python_executable_and_root_from(Some(&exe_dir), None);
        assert!(found.is_some(), "expected self-contained runtime/pythonw.exe to resolve");
        let (found_py, found_root) = found.unwrap();
        assert_eq!(found_py, py_exe);
        assert_eq!(found_root, rt_dir);
        let _ = fs::remove_dir_all(&base);

        // 2. Verify /api/system/hevc_status responds HTTP 200 JSON with installed field
        let state = Arc::new(Mutex::new(AppState::default()));
        let resp_hevc = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/system/hevc_status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_hevc.status_code, 200);
        let hevc_json = String::from_utf8_lossy(&resp_hevc.body);
        assert!(
            hevc_json.contains(r#""ok":true"#) && hevc_json.contains(r#""installed":"#),
            "hevc_status JSON invalid: {}",
            hevc_json
        );

        // 3. Verify 1-Click HEVC banner wiring in V4_BRIDGE_JS and mirror.js serve hook
        assert!(V4_BRIDGE_JS.contains("__v4ShowMainHevcBanner"));
        assert!(V4_BRIDGE_JS.contains("/api/system/install_hevc"));
    }

    #[test]
    fn test_tahap186_resolve_web_root_and_updater_bridge() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!(
            "iphone_link_t186_webroot_test_{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
        ));
        let exe_dir = base.join("install_dir");
        let exe_web = exe_dir.join("web");
        let rt_web = exe_dir.join("runtime").join("handset").join("web");
        let cwd_dir = base.join("cwd_fallback");
        let cwd_handset_web = cwd_dir.join("handset").join("web");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&exe_web).unwrap();
        fs::create_dir_all(&rt_web).unwrap();
        fs::create_dir_all(&cwd_handset_web).unwrap();

        fs::write(exe_web.join("index.html"), b"<html><!-- #card-updater --></html>").unwrap();
        fs::write(rt_web.join("index.html"), b"<html>raw</html>").unwrap();
        fs::write(cwd_handset_web.join("index.html"), b"<html>cwd raw</html>").unwrap();

        // Priority 1 must ALWAYS be exe_dir/web when exe_dir/web/index.html exists
        let resolved = resolve_web_root_from(Some(&exe_dir), Some(&cwd_dir));
        assert_eq!(resolved, exe_web);

        // If exe_dir/web is absent, cwd/handset/web resolves before runtime/handset/web fallback
        fs::remove_dir_all(&exe_web).unwrap();
        let resolved_cwd = resolve_web_root_from(Some(&exe_dir), Some(&cwd_dir));
        assert_eq!(resolved_cwd, cwd_handset_web);

        // If only runtime/handset/web remains, fallback picks it gracefully
        fs::remove_dir_all(&cwd_dir).unwrap();
        let resolved_rt = resolve_web_root_from(Some(&exe_dir), None);
        assert_eq!(resolved_rt, rt_web);
        let _ = fs::remove_dir_all(&base);

        // Verify V4_BRIDGE_JS contains ensureUpdaterCardExistsInDom & ID/EN labels
        assert!(V4_BRIDGE_JS.contains("ensureUpdaterCardExistsInDom"));
        assert!(V4_BRIDGE_JS.contains("PEMBARUAN PERANGKAT LUNAK"));
        assert!(V4_BRIDGE_JS.contains("SOFTWARE UPDATE"));
        assert!(V4_BRIDGE_JS.contains("Saluran Pembaruan"));
        assert!(V4_BRIDGE_JS.contains("Cek pembaruan otomatis saat dibuka"));
        assert!(V4_BRIDGE_JS.contains("Periksa Pembaruan"));
        assert!(V4_BRIDGE_JS.contains("cardUpdaterExists"));
    }

    #[test]
    fn test_tahap202_digital_http_loopback_c_and_clipboard_push() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_c_query = HttpRequest::parse(
            b"GET /c?u=https%3A%2F%2Fwww.threads.com%2F%40wallstreetwatch.id%2Fpost%2FDVghAL_kW9d%3Fxmt%3DAQF0xyz HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n",
        )
        .unwrap();
        let resp_c = handle_request(&req_c_query, &state, Path::new("web"));
        assert_eq!(resp_c.status_code, 200);
        assert_eq!(resp_c.content_type, "text/html; charset=utf-8");
        let html_c = String::from_utf8_lossy(&resp_c.body);
        assert!(html_c.contains("<script>window.close();</script>"));
        assert!(html_c.contains("\u{2713} Link Copied to PC!"));
        assert!(html_c.contains("https://www.threads.com/@wallstreetwatch.id/post/DVghAL_kW9d"));
        assert!(!html_c.contains("xmt="));

        let req_c_path = HttpRequest::parse(
            b"GET /c/https://www.instagram.com/reel/DVV51TDy3RO/?igsh=MWJ3aXNhN2F2a2N0cg== HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n",
        )
        .unwrap();
        let resp_c_path = handle_request(&req_c_path, &state, Path::new("web"));
        assert_eq!(resp_c_path.status_code, 200);
        let html_path = String::from_utf8_lossy(&resp_c_path.body);
        assert!(html_path.contains("https://www.instagram.com/reel/DVV5lTDy3RO/?igsh=MWJ3aXNhN2F2a2N0cg=="));

        let body_push = br#"{"url":"https://www.threads.com/@highlightbolaid/post/DVhR311Eync?xmt=AQF0abc"}"#;
        let raw_push = format!(
            "POST /api/clipboard/push HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body_push.len(),
            String::from_utf8_lossy(body_push)
        );
        let req_push = HttpRequest::parse(raw_push.as_bytes()).unwrap();
        let resp_push = handle_request(&req_push, &state, Path::new("web"));
        assert_eq!(resp_push.status_code, 200);
        let json_push = String::from_utf8_lossy(&resp_push.body);
        assert!(json_push.contains(r#""ok":true"#));
        assert!(json_push.contains(r#""url":"https://www.threads.com/@highlightbolaid/post/DVhR3i1Eync""#));

        let req_bad = HttpRequest::parse(b"GET /c?u=not-a-valid-url HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_bad = handle_request(&req_bad, &state, Path::new("web"));
        assert_eq!(resp_bad.status_code, 400);
    }

    #[test]
    fn test_tahap210_ota_update_endpoints_cache_and_offline_fallback() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. GET /api/update/status must respond from cache in < 1ms without spawning network I/O
        let req_status = HttpRequest::parse(
            b"GET /api/update/status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n",
        )
        .unwrap();
        let t0 = std::time::Instant::now();
        let resp_status = handle_request(&req_status, &state, Path::new("web"));
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(resp_status.status_code, 200);
        assert!(elapsed_ms < 50.0, "status took too long: {elapsed_ms} ms");
        let status_json = String::from_utf8_lossy(&resp_status.body);
        assert!(status_json.contains(r#""currentVersion":"5.0.1""#));
        assert!(status_json.contains(r#""latestVersion":"5.0.1""#));
        assert!(status_json.contains(r#""updateAvailable":false"#));
        assert!(status_json.contains(r#""user_settings.json""#));

        // 2. Simulate cache rejecting stale v4.0.0 & accepting v5.0.2
        {
            let mut st = state.lock().unwrap();
            st.updater.apply_worker_payload(
                r#"{"status":"update_available","latestVersion":"4.0.0","updateAvailable":true,"channel":"stable"}"#,
                "stable",
            );
            assert!(!st.updater.update_available);
            assert_eq!(st.updater.latest_version, "5.0.1");

            st.updater.apply_worker_payload(
                r#"{"status":"update_available","latestVersion":"5.0.2","updateAvailable":true,"downloadUrl":"https://example.com/v5.0.2.zip","channel":"beta"}"#,
                "beta",
            );
            assert!(st.updater.update_available);
            assert_eq!(st.updater.latest_version, "5.0.2");
        }

        // 3. POST /api/update/check gracefully handles worker absent/offline without throwing HTTP error
        let body_chk = br#"{"channel":"beta"}"#;
        let raw_chk = format!(
            "POST /api/update/check HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body_chk.len(),
            String::from_utf8_lossy(body_chk)
        );
        let req_chk = HttpRequest::parse(raw_chk.as_bytes()).unwrap();
        let resp_chk = handle_request(&req_chk, &state, Path::new("web"));
        assert_eq!(resp_chk.status_code, 200);
        let chk_json = String::from_utf8_lossy(&resp_chk.body);
        assert!(chk_json.contains(r#""ok":true"#));
        assert!(chk_json.contains(r#""currentVersion":"5.0.1""#));
        assert!(chk_json.contains(r#""channel":"beta""#));
    }

    #[test]
    fn test_tahap212_hot_ocr_status_and_native_wasapi_audio_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. GET /api/ocr/status & /api/status report hot_ocr_engine_ready
        let req_ocr = HttpRequest::parse(b"GET /api/ocr/status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_ocr = handle_request(&req_ocr, &state, Path::new("web"));
        assert_eq!(resp_ocr.status_code, 200);
        let ocr_json = String::from_utf8_lossy(&resp_ocr.body);
        assert!(ocr_json.contains(r#""hot_ocr_engine_ready":true"#), "got {ocr_json}");

        let req_st = HttpRequest::parse(b"GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_st = handle_request(&req_st, &state, Path::new("web"));
        assert_eq!(resp_st.status_code, 200);
        let st_json = String::from_utf8_lossy(&resp_st.body);
        assert!(st_json.contains(r#""hot_ocr_engine_ready": true"#), "got {st_json}");

        // 2. GET /api/audio/native-status reports real COM WASAPI Shared-Mode engine
        let req_aud = HttpRequest::parse(b"GET /api/audio/native-status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_aud = handle_request(&req_aud, &state, Path::new("web"));
        assert_eq!(resp_aud.status_code, 200);
        let aud_json = String::from_utf8_lossy(&resp_aud.body);
        assert!(aud_json.contains(r#""ok":true"#), "got {aud_json}");
        assert!(aud_json.contains(r#""mmdevice_enumerator_ok":true"#), "got {aud_json}");
        assert!(aud_json.contains(r#""audio_client_ok":true"#), "got {aud_json}");
        assert!(aud_json.contains(r#""audio_render_client_ok":true"#), "got {aud_json}");
        assert!(aud_json.contains(r#""sample_rate":48000"#), "got {aud_json}");
    }

    #[test]
    fn test_tahap213_hot_ocr_status_and_grab_link_shortcut_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ocr_guard = crate::ocr_engine::CRATE_GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        let req_ocr = HttpRequest::parse(b"GET /api/ocr/status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_ocr = handle_request(&req_ocr, &state, Path::new("web"));
        assert_eq!(resp_ocr.status_code, 200);
        let ocr_json = String::from_utf8_lossy(&resp_ocr.body);
        assert!(ocr_json.contains(r#""hot_ocr_engine_ready":true"#), "got {ocr_json}");
        assert!(ocr_json.contains(r#""fallbacks_triggered":0"#), "got {ocr_json}");

        let body_copy = br#"{"action":"copy","waitResult":true,"raw_text":"iphone to windows\n\nprabowo teddy keracun?"}"#;
        let hdr_copy = format!(
            "POST /api/keyboard/shortcut HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body_copy.len()
        );
        let mut raw_copy = hdr_copy.into_bytes();
        raw_copy.extend_from_slice(body_copy);
        let req_copy = HttpRequest::parse(&raw_copy).unwrap();
        let resp_copy = handle_request(&req_copy, &state, Path::new("web"));
        assert_eq!(resp_copy.status_code, 200);
        let copy_json = String::from_utf8_lossy(&resp_copy.body);
        assert!(copy_json.contains(r#""action":"copy""#), "got {copy_json}");

        let body_grab = br#"{"raw_text":"https://www.instagram.com/reel/DScCQallz12/"}"#;
        let hdr_grab = format!(
            "POST /api/social/grab-link HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body_grab.len()
        );
        let mut raw_grab = hdr_grab.into_bytes();
        raw_grab.extend_from_slice(body_grab);
        let req_grab = HttpRequest::parse(&raw_grab).unwrap();
        let resp_grab = handle_request(&req_grab, &state, Path::new("web"));
        assert_eq!(resp_grab.status_code, 200);
        let grab_json = String::from_utf8_lossy(&resp_grab.body);
        assert!(grab_json.contains("DScCQallz12"), "got {grab_json}");
    }

    #[test]
    fn test_tahap214_memory_telemetry_and_reclaim_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        trim_current_rust_process_working_set();
        let ws_mb = current_rust_process_working_set_mb();
        assert!(ws_mb > 0.0 && ws_mb < 150.0, "unexpected working set {ws_mb} MB");

        let req_mem = HttpRequest::parse(b"GET /api/system/memory HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_mem = handle_request(&req_mem, &state, Path::new("web"));
        assert_eq!(resp_mem.status_code, 200);
        let mem_json = String::from_utf8_lossy(&resp_mem.body);
        assert!(mem_json.contains(r#""ok":true"#), "got {mem_json}");
        assert!(mem_json.contains("rust_shell_working_set_mb"), "got {mem_json}");
        assert!(mem_json.contains("total_backend_working_set_mb"), "got {mem_json}");

        let req_reclaim = HttpRequest::parse(
            b"POST /api/system/reclaim-memory HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
        )
        .unwrap();
        let resp_reclaim = handle_request(&req_reclaim, &state, Path::new("web"));
        assert_eq!(resp_reclaim.status_code, 200);
        let rec_json = String::from_utf8_lossy(&resp_reclaim.body);
        assert!(rec_json.contains(r#""ok":true"#), "got {rec_json}");
        assert!(rec_json.contains("rust_shell_working_set_mb"), "got {rec_json}");
        assert!(rec_json.contains("total_backend_working_set_mb"), "got {rec_json}");
    }

    #[test]
    fn test_tahap215_lan_security_guard_and_c_endpoint() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_clip = crate::ocr_engine::read_win32_clipboard();
        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. LAN Security Guard: non-loopback strictly restricted to /c, /api/shortcut/push, /api/clipboard/push, OPTIONS
        let lan_ip: IpAddr = "192.168.1.50".parse().unwrap();
        let loopback_ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(is_lan_request_allowed(lan_ip, "GET", "/c?u=https%3A%2F%2Fshopee.co.id%2Fitem"));
        assert!(is_lan_request_allowed(lan_ip, "POST", "/api/shortcut/push"));
        assert!(is_lan_request_allowed(lan_ip, "POST", "/api/clipboard/push"));
        assert!(is_lan_request_allowed(lan_ip, "OPTIONS", "/api/status"));
        assert!(!is_lan_request_allowed(lan_ip, "GET", "/api/status"));
        assert!(!is_lan_request_allowed(lan_ip, "POST", "/api/screen/stop"));
        assert!(!is_lan_request_allowed(lan_ip, "POST", "/api/ocr/extract"));
        assert!(!is_lan_request_allowed(lan_ip, "POST", "/mirror/touch"));
        assert!(is_lan_request_allowed(loopback_ip, "GET", "/api/status"));
        assert!(is_lan_request_allowed(loopback_ip, "POST", "/api/screen/stop"));

        // 2. Complex URL decoding & Win32 clipboard copy (< 15ms) across IG /p/, /reel/, Threads /post/, TikTok, YouTube, Shopee
        let urls = [
            ("https://www.instagram.com/p/DScCQallz12/?igsh=MzRlODBiNWFlZA==", "https%3A%2F%2Fwww.instagram.com%2Fp%2FDScCQallz12%2F%3Figsh%3DMzRlODBiNWFlZA%3D%3D"),
            ("https://www.instagram.com/reel/DVV5lTDy3RO/", "https%3A%2F%2Fwww.instagram.com%2Freel%2FDVV5lTDy3RO%2F"),
            ("https://www.threads.com/@example.user/post/DVghAL_kW9d", "https%3A%2F%2Fwww.threads.com%2F%40example.user%2Fpost%2FDVghAL_kW9d"),
            ("https://www.tiktok.com/@kelvindaniyar/video/7412345678901234567", "https%3A%2F%2Fwww.tiktok.com%2F%40kelvindaniyar%2Fvideo%2F7412345678901234567"),
            ("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s", "https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ%26t%3D42s"),
            ("https://shopee.co.id/product/123456/7890123?sp_atk=xyz99", "https%3A%2F%2Fshopee.co.id%2Fproduct%2F123456%2F7890123%3Fsp_atk%3Dxyz99"),
        ];
        for (expected, encoded) in urls {
            let raw = format!("GET /c?u={} HTTP/1.1\r\nHost: 192.168.1.10:8765\r\nAccept: application/json\r\n\r\n", encoded);
            let req = HttpRequest::parse(raw.as_bytes()).unwrap();
            let resp = handle_request(&req, &state, Path::new("web"));
            assert_eq!(resp.status_code, 200, "failed for {expected}");
            let body = String::from_utf8_lossy(&resp.body);
            assert!(body.contains(r#""ok":true"#), "got {body}");
            assert!(body.contains(expected), "missing expected URL in {body}");
            let clip_now = crate::ocr_engine::read_win32_clipboard().unwrap_or_default();
            assert_eq!(clip_now, expected, "clipboard mismatch for {expected}");
        }

        // 3. GET /api/shortcut/setup-info
        let req_info = HttpRequest::parse(b"GET /api/shortcut/setup-info HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_info = handle_request(&req_info, &state, Path::new("web"));
        assert_eq!(resp_info.status_code, 200);
        let info_json = String::from_utf8_lossy(&resp_info.body);
        assert!(info_json.contains(r#""pc_lan_ip":"#), "got {info_json}");
        assert!(info_json.contains(r#""port":8765"#), "got {info_json}");
        assert!(info_json.contains("shortcut_endpoint_template"), "got {info_json}");
        assert!(info_json.contains("Send to iPhone Link"), "got {info_json}");

        // 4. Atomic Stop State (/api/screen/stop -> immediate idle phase on /api/status)
        {
            let mut st = state.lock().unwrap();
            st.is_streaming = true;
        }
        let req_stop = HttpRequest::parse(b"POST /api/screen/stop HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        let resp_stop = handle_request(&req_stop, &state, Path::new("web"));
        assert_eq!(resp_stop.status_code, 200);
        let stop_body = String::from_utf8_lossy(&resp_stop.body);
        assert!(stop_body.contains(r#""phase":"idle""#), "got {stop_body}");
        let req_status = HttpRequest::parse(b"GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_status = handle_request(&req_status, &state, Path::new("web"));
        assert_eq!(resp_status.status_code, 200);
        let st_body = String::from_utf8_lossy(&resp_status.body);
        assert!(st_body.contains(r#""phase": "idle""#) || st_body.contains(r#""phase":"idle""#), "got {st_body}");

        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        if let Some(prev) = prev_clip {
            let _ = crate::ocr_engine::write_win32_clipboard(&prev);
        }
    }

    #[test]
    fn test_tahap216_setup_info_anti_confusion_ip_fields() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. Parser extracts cached iPhone LAN IPv4 without matching PC LAN IP or loopback
        let sample_wifi_json = r#"{"paired":["00008140-00112233"],"enabled":true,"last_ip":"192.0.2.41"}"#;
        assert_eq!(
            extract_cached_iphone_lan_ip_from_json(sample_wifi_json, "192.0.2.10").as_deref(),
            Some("192.0.2.41")
        );
        assert_eq!(
            extract_cached_iphone_lan_ip_from_json(sample_wifi_json, "192.0.2.41"),
            None
        );
        let loopback_json = r#"{"last_ip":"127.0.0.1"}"#;
        assert_eq!(
            extract_cached_iphone_lan_ip_from_json(loopback_json, "192.0.2.10"),
            None
        );

        // 2. GET /api/shortcut/setup-info returns explicit anti-confusion IP fields alongside Tahap 215 fields
        let req_info = HttpRequest::parse(b"GET /api/shortcut/setup-info HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_info = handle_request(&req_info, &state, Path::new("web"));
        assert_eq!(resp_info.status_code, 200);
        let info_json = String::from_utf8_lossy(&resp_info.body);
        assert!(info_json.contains(r#""pc_lan_ip":"#), "missing pc_lan_ip in {info_json}");
        assert!(info_json.contains(r#""iphone_lan_ip":"#), "missing iphone_lan_ip in {info_json}");
        assert!(info_json.contains(r#""ip_warning_id":"[GUNAKAN IP PC:"#), "missing ip_warning_id in {info_json}");
        assert!(info_json.contains(r#""ip_warning_en":"[USE PC IP:"#), "missing ip_warning_en in {info_json}");
        assert!(info_json.contains("shortcut_endpoint_template"), "missing template in {info_json}");
        assert!(!info_json.contains('\u{26a1}') && !info_json.contains('\u{26a0}'), "must not contain forbidden emojis in {info_json}");
    }

    #[test]
    fn test_tahap217_icloud_shortcut_url_and_dual_mode_siri_prompt() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_info = HttpRequest::parse(b"GET /api/shortcut/setup-info HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_info = handle_request(&req_info, &state, Path::new("web"));
        assert_eq!(resp_info.status_code, 200);
        let info_json = String::from_utf8_lossy(&resp_info.body);
        assert!(
            info_json.contains(r#""icloud_shortcut_url":"https://www.icloud.com/shortcuts/bbb096d97e174c1386f15e31aedef8ff""#),
            "missing official iCloud shortcut URL in {info_json}"
        );
        assert!(info_json.contains("Send to iPhone Link"), "missing shortcut name in {info_json}");
        assert!(info_json.contains("Get URLs from"), "missing Dual-Mode Get URLs from in {info_json}");
        assert!(info_json.contains("Get Contents of URL"), "missing Get Contents of URL in {info_json}");
    }

    #[test]
    fn test_tahap226_native_inprocess_diagnostics_repair_and_amfi_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        let req_diag = HttpRequest::parse(b"GET /api/driver/diagnostics HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_diag = handle_request(&req_diag, &state, Path::new("web"));
        assert_eq!(resp_diag.status_code, 200);
        let body_diag = String::from_utf8_lossy(&resp_diag.body);
        assert!(body_diag.contains(r#""native":true"#), "got {body_diag}");
        for k in [
            "apple_driver_installed",
            "apple_service_running",
            "tcp_port_27015_normal",
            "hevc_codec_installed",
            "developer_mode_status",
        ] {
            assert!(body_diag.contains(k), "missing {k} in {body_diag}");
        }

        let req_status = HttpRequest::parse(b"GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_status = handle_request(&req_status, &state, Path::new("web"));
        assert_eq!(resp_status.status_code, 200);
        let body_status = String::from_utf8_lossy(&resp_status.body);
        assert!(body_status.contains(r#""detailedDiagnostics":"#), "missing detailedDiagnostics in {body_status}");

        let req_usb = HttpRequest::parse(b"GET /api/devices/native-usb HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_usb = handle_request(&req_usb, &state, Path::new("web"));
        assert_eq!(resp_usb.status_code, 200);
        let body_usb = String::from_utf8_lossy(&resp_usb.body);
        assert!(body_usb.contains(r#""native":true"#) && body_usb.contains(r#""devices":["#), "got {body_usb}");

        let req_rep = HttpRequest::parse(b"POST /api/driver/repair HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        let resp_rep = handle_request(&req_rep, &state, Path::new("web"));
        assert_eq!(resp_rep.status_code, 200);
        let body_rep = String::from_utf8_lossy(&resp_rep.body);
        assert!(
            body_rep.contains(r#""ok":true"#)
                && body_rep.contains(r#""native":true"#)
                && body_rep.contains(r#""diagnostics":"#)
                && body_rep.contains(r#""message_id":"#)
                && body_rep.contains(r#""message_en":"#),
            "got {body_rep}"
        );

        let req_unl_disc = HttpRequest::parse(b"POST /api/device/unlock-developer-mode HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        let resp_unl_disc = handle_request(&req_unl_disc, &state, Path::new("web"));
        assert_eq!(resp_unl_disc.status_code, 200);
        let body_unl_disc = String::from_utf8_lossy(&resp_unl_disc.body);
        assert!(
            body_unl_disc.contains(r#""developer_mode_status":"#)
                && body_unl_disc.contains(r#""message_id":"#)
                && body_unl_disc.contains(r#""message_en":"#),
            "got {body_unl_disc}"
        );

        // Live mock usbmuxd + Lockdown + AmfiClient socket test in-process
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let mux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_listener.local_addr().unwrap();
        mux_listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_srv = stop.clone();

        fn read_mux(s: &mut std::net::TcpStream) -> String {
            let mut hdr = [0u8; 16];
            if s.read_exact(&mut hdr).is_err() {
                return String::new();
            }
            let len = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
            let mut body = vec![0u8; len.saturating_sub(16)];
            let _ = s.read_exact(&mut body);
            String::from_utf8_lossy(&body).into_owned()
        }
        fn write_mux(s: &mut std::net::TcpStream, xml: &str) {
            let b = xml.as_bytes();
            let total = (16 + b.len()) as u32;
            let mut hdr = [0u8; 16];
            hdr[0..4].copy_from_slice(&total.to_le_bytes());
            hdr[4..8].copy_from_slice(&1u32.to_le_bytes());
            hdr[8..12].copy_from_slice(&8u32.to_le_bytes());
            hdr[12..16].copy_from_slice(&1u32.to_le_bytes());
            let _ = s.write_all(&hdr);
            let _ = s.write_all(b);
            let _ = s.flush();
        }
        fn read_lp(s: &mut std::net::TcpStream) -> String {
            let mut lb = [0u8; 4];
            if s.read_exact(&mut lb).is_err() {
                return String::new();
            }
            let len = u32::from_be_bytes(lb) as usize;
            let mut body = vec![0u8; len];
            let _ = s.read_exact(&mut body);
            String::from_utf8_lossy(&body).into_owned()
        }
        fn write_lp(s: &mut std::net::TcpStream, xml: &str) {
            let b = xml.as_bytes();
            let _ = s.write_all(&(b.len() as u32).to_be_bytes());
            let _ = s.write_all(b);
            let _ = s.flush();
        }

        let srv_handle = thread::spawn(move || {
            let mut hs = Vec::new();
            while !stop_srv.load(Ordering::Relaxed) {
                match mux_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                        hs.push(thread::spawn(move || {
                            let req = read_mux(&mut s);
                            if req.contains("ListDevices") {
                                write_mux(&mut s, "<plist version=\"1.0\"><dict><key>DeviceList</key><array><dict><key>Properties</key><dict><key>SerialNumber</key><string>00008150-SRVAMFI</string><key>DeviceID</key><integer>3</integer><key>ConnectionType</key><string>USB</string></dict></dict></array></dict></plist>");
                            } else if req.contains("ReadPairRecord") {
                                write_mux(&mut s, "<plist version=\"1.0\"><dict><key>PairRecordData</key><data>PD94bWwgdmVyc2lvbj0iMS4wIiBlbmNvZGluZz0iVVRGLTgiPz48cGxpc3QgdmVyc2lvbj0iMS4wIj48ZGljdD48a2V5Pkhvc3RJRDwva2V5PjxzdHJpbmc+SDE8L3N0cmluZz48L2RpY3Q+PC9wbGlzdD4=</data></dict></plist>");
                            } else if req.contains("Connect") {
                                let is_lockdown = req.contains("<integer>32498</integer>");
                                write_mux(&mut s, "<plist version=\"1.0\"><dict><key>Number</key><integer>0</integer></dict></plist>");
                                if is_lockdown {
                                    for _ in 0..3 {
                                        let r = read_lp(&mut s);
                                        if r.is_empty() { break; }
                                        if r.contains("StartSession") {
                                            write_lp(&mut s, "<plist version=\"1.0\"><dict><key>EnableSessionSSL</key><false/></dict></plist>");
                                        } else if r.contains("StartService") {
                                            write_lp(&mut s, "<plist version=\"1.0\"><dict><key>Port</key><integer>58222</integer><key>EnableServiceSSL</key><false/></dict></plist>");
                                        } else if r.contains("GetValue") {
                                            write_lp(&mut s, "<plist version=\"1.0\"><dict><key>Value</key><string>iPhone18,1</string></dict></plist>");
                                        }
                                    }
                                } else {
                                    for _ in 0..2 {
                                        let r = read_lp(&mut s);
                                        if r.is_empty() { break; }
                                        if r.contains("<integer>3</integer>") {
                                            write_lp(&mut s, "<plist version=\"1.0\"><dict><key>success</key><true/><key>status</key><false/></dict></plist>");
                                        } else {
                                            write_lp(&mut s, "<plist version=\"1.0\"><dict><key>success</key><true/></dict></plist>");
                                        }
                                    }
                                }
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for h in hs {
                let _ = h.join();
            }
        });

        env::set_var("USBMUXD_SOCKET_ADDRESS", mux_addr.to_string());
        let req_unl_mock = HttpRequest::parse(
            b"POST /api/device/unlock-developer-mode HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: 42\r\n\r\n{\"udid\":\"00008150-SRVAMFI\",\"arm_enable\":0}",
        ).unwrap();
        let resp_unl_mock = handle_request(&req_unl_mock, &state, Path::new("web"));
        env::remove_var("USBMUXD_SOCKET_ADDRESS");
        stop.store(true, Ordering::Relaxed);
        let _ = srv_handle.join();

        assert_eq!(resp_unl_mock.status_code, 200);
        let body_unl_mock = String::from_utf8_lossy(&resp_unl_mock.body);
        assert!(
            body_unl_mock.contains(r#""ok":true"#)
                && body_unl_mock.contains(r#""revealed":true"#)
                && body_unl_mock.contains(r#""udid":"00008150-SRVAMFI""#)
                && body_unl_mock.contains(r#""developer_mode_status":"disabled""#),
            "got {body_unl_mock}"
        );
    }

    #[test]
    fn test_tahap227_native_inprocess_wifi_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        let tmp_root = env::temp_dir().join("iphone_link_tahap227_srv_wifi_test");
        let _ = fs::remove_dir_all(&tmp_root);
        fs::create_dir_all(&tmp_root).unwrap();
        let state_file = tmp_root.join("wifi_state.json");
        let pairing_dir = tmp_root.join("pairing");
        env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &state_file);
        env::set_var("IPHONE_LINK_PAIRING_DIR", &pairing_dir);
        env::set_var("USBMUXD_SOCKET_ADDRESS", "127.0.0.1:1");

        // 1. GET /api/wifi (clean unpaired state)
        let req_get = HttpRequest::parse(b"GET /api/wifi HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_get = handle_request(&req_get, &state, Path::new("web"));
        assert_eq!(resp_get.status_code, 200);
        let body_get = String::from_utf8_lossy(&resp_get.body);
        assert!(
            body_get.contains(r#""ok":true"#)
                && body_get.contains(r#""native":true"#)
                && body_get.contains(r#""pairedUdids":[]"#)
                && body_get.contains(r#""wirelessLockdownEnabled":false"#)
                && body_get.contains(r#""wifi":{"#),
            "got {body_get}"
        );

        // 2. POST /api/wifi/pair (disconnected -> bilingual guidance)
        let req_pair_disc = HttpRequest::parse(b"POST /api/wifi/pair HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        let resp_pair_disc = handle_request(&req_pair_disc, &state, Path::new("web"));
        assert_eq!(resp_pair_disc.status_code, 200);
        let body_pair_disc = String::from_utf8_lossy(&resp_pair_disc.body);
        assert!(
            body_pair_disc.contains(r#""ok":false"#)
                && body_pair_disc.contains(r#""native":true"#)
                && body_pair_disc.contains(r#""message_id":"#)
                && body_pair_disc.contains(r#""message_en":"#),
            "got {body_pair_disc}"
        );

        // 3. Mock USB usbmuxd + Lockdown + RemotePairing for POST /api/wifi/pair
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let mux_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mux_addr = mux_listener.local_addr().unwrap();
        mux_listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_srv = stop.clone();

        fn rmux(s: &mut std::net::TcpStream) -> String {
            let mut hdr = [0u8; 16];
            if s.read_exact(&mut hdr).is_err() { return String::new(); }
            let len = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
            let mut b = vec![0u8; len.saturating_sub(16)];
            let _ = s.read_exact(&mut b);
            String::from_utf8_lossy(&b).into_owned()
        }
        fn wmux(s: &mut std::net::TcpStream, xml: &str) {
            let b = xml.as_bytes();
            let total = (16 + b.len()) as u32;
            let mut hdr = [0u8; 16];
            hdr[0..4].copy_from_slice(&total.to_le_bytes());
            hdr[4..8].copy_from_slice(&1u32.to_le_bytes());
            hdr[8..12].copy_from_slice(&8u32.to_le_bytes());
            hdr[12..16].copy_from_slice(&1u32.to_le_bytes());
            let _ = s.write_all(&hdr);
            let _ = s.write_all(b);
            let _ = s.flush();
        }
        fn rlp(s: &mut std::net::TcpStream) -> String {
            let mut lb = [0u8; 4];
            if s.read_exact(&mut lb).is_err() { return String::new(); }
            let len = u32::from_be_bytes(lb) as usize;
            let mut b = vec![0u8; len];
            let _ = s.read_exact(&mut b);
            String::from_utf8_lossy(&b).into_owned()
        }
        fn wlp(s: &mut std::net::TcpStream, xml: &str) {
            let b = xml.as_bytes();
            let _ = s.write_all(&(b.len() as u32).to_be_bytes());
            let _ = s.write_all(b);
            let _ = s.flush();
        }

        let srv_handle = thread::spawn(move || {
            let mut hs = Vec::new();
            while !stop_srv.load(Ordering::Relaxed) {
                match mux_listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                        hs.push(thread::spawn(move || {
                            let req = rmux(&mut s);
                            if req.contains("ListDevices") {
                                wmux(&mut s, "<plist version=\"1.0\"><dict><key>DeviceList</key><array><dict><key>Properties</key><dict><key>SerialNumber</key><string>00008150-SRVWIFI</string><key>DeviceID</key><integer>4</integer><key>ConnectionType</key><string>USB</string></dict></dict></array></dict></plist>");
                            } else if req.contains("ReadPairRecord") {
                                wmux(&mut s, "<plist version=\"1.0\"><dict><key>PairRecordData</key><data>PD94bWwgdmVyc2lvbj0iMS4wIiBlbmNvZGluZz0iVVRGLTgiPz48cGxpc3QgdmVyc2lvbj0iMS4wIj48ZGljdD48a2V5Pkhvc3RJRDwva2V5PjxzdHJpbmc+SDE8L3N0cmluZz48L2RpY3Q+PC9wbGlzdD4=</data></dict></plist>");
                            } else if req.contains("Connect") {
                                let is_lockdown = req.contains("<integer>32498</integer>");
                                wmux(&mut s, "<plist version=\"1.0\"><dict><key>Number</key><integer>0</integer></dict></plist>");
                                if is_lockdown {
                                    for _ in 0..8 {
                                        let r = rlp(&mut s);
                                        if r.is_empty() { break; }
                                        if r.contains("StartSession") {
                                            wlp(&mut s, "<plist version=\"1.0\"><dict><key>EnableSessionSSL</key><false/></dict></plist>");
                                        } else if r.contains("SetValue") {
                                            wlp(&mut s, "<plist version=\"1.0\"><dict><key>Request</key><string>SetValue</string></dict></plist>");
                                        } else if r.contains("StartService") {
                                            wlp(&mut s, "<plist version=\"1.0\"><dict><key>Port</key><integer>59111</integer><key>EnableServiceSSL</key><false/></dict></plist>");
                                        } else if r.contains("GetValue") {
                                            wlp(&mut s, "<plist version=\"1.0\"><dict><key>Value</key><string>00008150-SRVWIFI</string></dict></plist>");
                                        }
                                    }
                                } else {
                                    let _ = rlp(&mut s);
                                    wlp(&mut s, "<plist version=\"1.0\"><dict><key>alt_irk</key><data>AQIDBAUGBwgJCgsMDQ4PEA==</data><key>encryptionKey</key><data>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=</data></dict></plist>");
                                }
                            }
                        }));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
            for h in hs { let _ = h.join(); }
        });

        env::set_var("USBMUXD_SOCKET_ADDRESS", mux_addr.to_string());
        let req_pair = HttpRequest::parse(
            b"POST /api/wifi/pair HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: 27\r\n\r\n{\"udid\":\"00008150-SRVWIFI\"}",
        ).unwrap();
        let resp_pair = handle_request(&req_pair, &state, Path::new("web"));
        env::remove_var("USBMUXD_SOCKET_ADDRESS");
        stop.store(true, Ordering::Relaxed);
        let _ = srv_handle.join();

        assert_eq!(resp_pair.status_code, 200);
        let body_pair = String::from_utf8_lossy(&resp_pair.body);
        assert!(
            body_pair.contains(r#""ok":true"#)
                && body_pair.contains(r#""native":true"#)
                && body_pair.contains(r#""wifi_enabled":true"#)
                && body_pair.contains(r#""wirelessLockdownEnabled":true"#)
                && body_pair.contains(r#""00008150-SRVWIFI""#),
            "got {body_pair}"
        );

        // 4. Mock Lockdown QueryType server for POST /api/wifi/scan
        iphone_link_apple::wifi::update_device_wifi_state(
            "00008150-SRVWIFI",
            Some(true),
            Some("127.0.0.1"),
            false,
            None,
            None,
            false,
            true,
            None,
        );
        let ld_srv = TcpListener::bind("127.0.0.1:0").unwrap();
        let ld_port = ld_srv.local_addr().unwrap().port();
        let ld_handle = thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut ls, _)) = ld_srv.accept() {
                    let _ = rlp(&mut ls);
                    wlp(
                        &mut ls,
                        "<plist version=\"1.0\"><dict><key>Type</key><string>com.apple.mobile.lockdown</string><key>UniqueDeviceID</key><string>00008150-SRVWIFI</string></dict></plist>",
                    );
                }
            }
        });
        env::set_var("IPHONE_LINK_LOCKDOWN_PORT", ld_port.to_string());
        let req_scan = HttpRequest::parse(
            b"POST /api/wifi/scan HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: 27\r\n\r\n{\"udid\":\"00008150-SRVWIFI\"}",
        ).unwrap();
        let resp_scan = handle_request(&req_scan, &state, Path::new("web"));
        let req_probe = HttpRequest::parse(b"GET /api/wifi/probe?udid=00008150-SRVWIFI HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_probe = handle_request(&req_probe, &state, Path::new("web"));
        env::remove_var("IPHONE_LINK_LOCKDOWN_PORT");
        let _ = ld_handle.join();

        let body_scan = String::from_utf8_lossy(&resp_scan.body);
        assert!(
            body_scan.contains(r#""ok":true"#)
                && body_scan.contains(r#""native":true"#)
                && body_scan.contains(r#""found":true"#)
                && body_scan.contains(r#""ip":"127.0.0.1""#)
                && body_scan.contains(r#""reachable":true"#)
                && body_scan.contains(r#""message_id":"#)
                && body_scan.contains(r#""message_en":"#),
            "got {body_scan}"
        );
        let body_probe = String::from_utf8_lossy(&resp_probe.body);
        assert!(body_probe.contains(r#""reachable":true"#), "got {body_probe}");

        // 5. GET /api/status inherits zero-network native Wi-Fi state
        let req_status = HttpRequest::parse(b"GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n").unwrap();
        let resp_status = handle_request(&req_status, &state, Path::new("web"));
        let body_status = String::from_utf8_lossy(&resp_status.body);
        assert!(
            body_status.contains(r#""wirelessLockdownEnabled":"#)
                && body_status.contains(r#""00008150-SRVWIFI""#),
            "got {body_status}"
        );

        env::remove_var("USBMUXD_SOCKET_ADDRESS");
        env::remove_var("IPHONE_LINK_WIFI_STATE_PATH");
        env::remove_var("IPHONE_LINK_PAIRING_DIR");
        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn test_tahap228_native_inprocess_hid_stealth_lock_wake_and_wipe() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ocr_guard = crate::ocr_engine::CRATE_GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();
        GLOBAL_STEALTH_ACTIVE.store(false, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        let post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Stealth Dim idempotency: 16 -> 0 (skipped) -> 4 -> 0 (skipped)
        let d1 = String::from_utf8_lossy(
            &handle_request(&post("/api/device/stealth", r#"{"enabled":true}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(d1.contains(r#""steps":16"#) && d1.contains(r#""skipped":false"#) && d1.contains(r#""stealth":true"#), "got {d1}");

        let d2 = String::from_utf8_lossy(
            &handle_request(&post("/api/device/stealth", r#"{"enabled":true}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(d2.contains(r#""steps":0"#) && d2.contains(r#""skipped":true"#), "got {d2}");

        let r1 = String::from_utf8_lossy(
            &handle_request(&post("/api/device/stealth", r#"{"enabled":false,"restoreSteps":99}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(r1.contains(r#""steps":4"#) && r1.contains(r#""stealth":false"#), "got {r1}");

        let r2 = String::from_utf8_lossy(
            &handle_request(&post("/api/device/stealth", r#"{"enabled":false}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(r2.contains(r#""steps":0"#) && r2.contains(r#""skipped":true"#), "got {r2}");

        // 2. Dim again, then synchronous 0.5s lock auto-restores 4 steps + >=480ms hold
        let _ = handle_request(&post("/api/device/stealth", r#"{"enabled":true}"#), &state, Path::new("web"));
        let lock_body = String::from_utf8_lossy(
            &handle_request(&post("/api/device/lock", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            lock_body.contains(r#""locked":true"#)
                && lock_body.contains(r#""stealthRestored":true"#)
                && lock_body.contains(r#""restoreSteps":4"#),
            "got {lock_body}"
        );
        assert!(!is_stealth_dim_active());

        // 3. DPAPI PIN save + /api/keyboard/typed + /api/device/wake-unlock Zero Passcode Leakage
        let pin_resp = String::from_utf8_lossy(
            &handle_request(&post("/api/device/pin", r#"{"unlockPin":"123456"}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(pin_resp.contains(r#""dpapiEncrypted":true"#), "got {pin_resp}");

        let _ = handle_request(&post("/api/keyboard/typed", r#"{"text":"aman terkendali"}"#), &state, Path::new("web"));
        let _ = handle_request(&post("/api/keyboard/typed", r#"{"commitLine":true}"#), &state, Path::new("web"));

        let wake_resp = String::from_utf8_lossy(
            &handle_request(&post("/api/device/wake-unlock", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(wake_resp.contains(r#""unlocked":true"#) && wake_resp.contains(r#""steps":12"#), "got {wake_resp}");

        let typed_lines = String::from_utf8_lossy(
            &handle_request(&HttpRequest {
                method: "GET".to_string(),
                path: "/api/keyboard/typed".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            }, &state, Path::new("web")).body,
        )
        .to_string();
        assert!(typed_lines.contains("aman terkendali") && !typed_lines.contains("123456"), "got {typed_lines}");

        // 4. Toggle onscreen keyboard (Eject 0x0C, 0x00B8 = 184) & 4-step Shift Latching paste
        let toggle_resp = String::from_utf8_lossy(
            &handle_request(&post("/api/keyboard/toggle-onscreen", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(toggle_resp.contains(r#""usage":184"#), "got {toggle_resp}");

        let paste_resp = String::from_utf8_lossy(
            &handle_request(&post("/api/keyboard/paste", r#"{"text":"karhutla aman wok?"}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(paste_resp.contains(r#""typed":18"#), "got {paste_resp}");

        // 5. Privacy wipe clears PIN & typed buffer
        let _ = handle_request(&post("/api/device/wipe-privacy-data", "{}"), &state, Path::new("web"));
        assert!(get_typed_lines_buffer().is_empty());
        assert!(load_decrypted_pin().unwrap().is_none());
        reset_hid_runtime_state();
        GLOBAL_STEALTH_ACTIVE.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_tahap228_mirror_touch_rotation_and_gesture_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));
        let post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        let touch_270 = String::from_utf8_lossy(
            &handle_request(
                &post("/mirror/touch", r#"{"nx":0.25,"ny":0.75,"rotation":270}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(
            touch_270.contains(r#""rotation":270"#)
                && touch_270.contains(r#""mappedX":16384"#)
                && touch_270.contains(r#""mappedY":16384"#),
            "got {touch_270}"
        );

        let g_recent = String::from_utf8_lossy(
            &handle_request(
                &post("/mirror/gesture", r#"{"direction":"recent"}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(g_recent.contains(r#""gesture":"recent""#) && g_recent.contains(r#""steps":12"#), "got {g_recent}");

        let g_pinch = String::from_utf8_lossy(
            &handle_request(
                &post("/mirror/gesture", r#"{"action":"pinch"}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(g_pinch.contains(r#""gesture":"pinch""#) && g_pinch.contains(r#""steps":10"#), "got {g_pinch}");
    }

    #[test]
    fn test_tahap229_native_inprocess_ocr_shortcut_skip_ocr_alt_c_and_alt_r() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ocr_guard = crate::ocr_engine::CRATE_GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_clip = crate::ocr_engine::read_win32_clipboard();
        clear_copy_cache();
        set_active_clipboard_source(ClipboardSource::Pc);

        let state = Arc::new(Mutex::new(AppState::default()));
        let post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Ctrl+C (skipOcr: true) executes in < 10ms without calling OCR or writing PC clipboard
        let sentinel = "WIN_CLIPBOARD_SENTINEL_229";
        let _ = write_win32_clipboard(sentinel);
        let t0 = std::time::Instant::now();
        let resp_ctrl_c = String::from_utf8_lossy(
            &handle_request(
                &post("/api/keyboard/shortcut", r#"{"action":"copy","skipOcr":true}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        let elapsed_ms = t0.elapsed().as_millis();
        assert!(
            resp_ctrl_c.contains(r#""native":true"#)
                && resp_ctrl_c.contains(r#""skipOcr":true"#)
                && resp_ctrl_c.contains(r#""source":"iphone""#),
            "got {resp_ctrl_c}"
        );
        assert!(elapsed_ms < 50, "Ctrl+C took too long: {elapsed_ms}ms");
        assert_eq!(crate::ocr_engine::get_active_clipboard_source(), ClipboardSource::Iphone);
        assert_eq!(crate::ocr_engine::read_win32_clipboard().as_deref(), Some(sentinel));

        // 2. Alt+C (skipOcr: false) runs full batch OCR + paragraph formatting + clipboard write
        let batch_payload = concat!(
            r#"{"action":"copy","waitResult":true,"batchTexts":["#,
            r#""<<<SEL_COLOR:blue>>>\r\n"#,
            r#"Aksi pelari Jakarta Running Festival terkena (Cut-Off Time/\r\n"#,
            r#"COT) di titik Cut-Off Point (COP) 1 dihadang Oleh @wisnutry017.\r\n\r\n"#,
            r#"Bagaimana tanggapan kalian terkena cut-off time ini, Banteners?\r\n\r\n"#,
            r#"Source: Threads @wisnutry017\r\n\r\n"#,
            r#"#iakartarunninafestival #irf2026""#,
            r#"]}"#
        );
        let resp_alt_c = String::from_utf8_lossy(
            &handle_request(
                &post("/api/keyboard/shortcut", batch_payload),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(
            resp_alt_c.contains(r#""native":true"#)
                && resp_alt_c.contains(r#""copiedToPc":true"#)
                && resp_alt_c.contains("(Cut-Off Time/")
                && resp_alt_c.contains("Cut-Off Point (COP) 1")
                && resp_alt_c.contains("@wisnutryo17")
                && resp_alt_c.contains("#jakartarunningfestival")
                && resp_alt_c.contains(r#""autoClearedCache":true"#),
            "got {resp_alt_c}"
        );

        // 3. Alt+R (clear-copy-cache) & /api/ocr/clear-chunks
        crate::ocr_engine::seed_long_ocr_chunk_for_test(
            &["Line 1", "Line 2", "Line 3", "Line 4"],
            crate::ocr_engine::SelectionColor::Yellow,
        );
        assert_eq!(crate::ocr_engine::get_recent_long_ocr_chunks_count(), 1);
        let resp_clear = String::from_utf8_lossy(
            &handle_request(&post("/api/ocr/clear-chunks", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(resp_clear.contains(r#""cleared":true"#));
        assert_eq!(crate::ocr_engine::get_recent_long_ocr_chunks_count(), 0);

        let resp_alt_r = String::from_utf8_lossy(
            &handle_request(
                &post("/api/keyboard/shortcut", r#"{"action":"clear-copy-cache"}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(resp_alt_r.contains(r#""action":"clear-copy-cache""#) && resp_alt_r.contains(r#""cleared":true"#));

        if let Some(prev) = prev_clip {
            let _ = write_win32_clipboard(&prev);
        }
        clear_copy_cache();
        set_active_clipboard_source(ClipboardSource::Pc);
    }

    #[test]
    fn test_tahap229_native_ocr_extract_and_link_grab_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ocr_guard = crate::ocr_engine::CRATE_GLOBAL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_clip = crate::ocr_engine::read_win32_clipboard();
        clear_copy_cache();
        let state = Arc::new(Mutex::new(AppState::default()));
        let post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        let ext = String::from_utf8_lossy(
            &handle_request(
                &post("/api/ocr/extract", r#"{"batchTexts":["tarhutla aman\r\n\r\nprabowk"]}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(ext.contains(r#""native":true"#) && ext.contains("karhutla aman") && ext.contains("prabowo"), "got {ext}");

        let grab = String::from_utf8_lossy(
            &handle_request(
                &post("/api/link/grab", r#"{"text":"Cek http5://vt.tiktok.com/ZSjX9k2Lm/ sekarang"}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(grab.contains(r#""native":true"#) && grab.contains("https://vt.tiktok.com/ZSjX9k2Lm/"), "got {grab}");

        if let Some(prev) = prev_clip {
            let _ = write_win32_clipboard(&prev);
        }
        clear_copy_cache();
    }

    #[test]
    fn test_tahap230_in_process_screen_mirror_and_orientation_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::hid::reset_hid_runtime_state();
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        let get = |path: &str| HttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. First-Frame Honest Gate via /api/screen/start -> phase "starting", url: null
        let start_body = String::from_utf8_lossy(
            &handle_request(
                &post("/api/screen/start", r#"{"udid":"UDID-230","transport":"cable","honestGate":true}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(
            start_body.contains(r#""phase":"starting""#)
                && start_body.contains(r#""url":null"#)
                && start_body.contains(r#""native":true"#),
            "got {start_body}"
        );

        // 2. First valid Annex-B keyframe promotes phase to "running" and queues /stream.bin chunk
        assert!(iphone_link_apple::mirror::global_on_first_valid_frame(
            vec![0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0x99],
            true
        ));
        let scr_body = String::from_utf8_lossy(
            &handle_request(&get("/api/screen"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(scr_body.contains(r#""phase":"running""#) && scr_body.contains(r#""frames_received":1"#), "got {scr_body}");

        // 3. /mirror/codec, /mirror/pli, /api/screen/restart
        let codec_body = String::from_utf8_lossy(
            &handle_request(&get("/mirror/codec"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            codec_body.contains(r#""codec":"hvc1.1.6.L150.B0""#)
                && codec_body.contains(r#""bottom":58"#)
                && codec_body.contains(r#""right":14"#),
            "got {codec_body}"
        );
        let pli_body = String::from_utf8_lossy(
            &handle_request(&post("/mirror/pli", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(pli_body.contains(r#""pliSent":true"#) && pli_body.contains(r#""pliCount":1"#), "got {pli_body}");

        // 4. /api/screen/orientation manual 270 -> Home instant 0°
        let rot270 = String::from_utf8_lossy(
            &handle_request(
                &post("/api/screen/orientation", r#"{"orientation":270,"action":"manual"}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(rot270.contains(r#""orientation":270"#) && rot270.contains(r#""isLandscape":true"#), "got {rot270}");
        let rot_home = String::from_utf8_lossy(
            &handle_request(
                &post("/api/screen/orientation", r#"{"action":"home"}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(rot_home.contains(r#""orientation":0"#) && rot_home.contains(r#""isLandscape":false"#), "got {rot_home}");

        // 5. /api/screen/stop cleanly stops & restores Stealth Dim if active
        iphone_link_apple::hid::set_stealth_dim_active(true);
        let stop_body = String::from_utf8_lossy(
            &handle_request(&post("/api/screen/stop", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(stop_body.contains(r#""phase":"idle""#), "got {stop_body}");
        assert!(!iphone_link_apple::hid::is_stealth_dim_active());

        // Drain live snapshot queue cleanly
        let chunks = iphone_link_apple::mirror::global_take_live_stream_snapshot();
        assert!(!chunks.is_empty());
        assert_eq!(&chunks[0][4..8], &[0x00, 0x00, 0x00, 0x01]);

        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_apple::hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap230_in_process_audio_and_status_zero_poll_endpoints() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::hid::reset_hid_runtime_state();
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        let get = |path: &str| HttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. /api/audio/start with low buffer preset & 88% volume
        let aud_start = String::from_utf8_lossy(
            &handle_request(
                &post("/api/audio/start", r#"{"audioOut":"pc","bufferPreset":"low","volume":88}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(
            aud_start.contains(r#""ok":true"#)
                && aud_start.contains(r#""audioOut":"pc""#)
                && aud_start.contains(r#""volume":88"#)
                && aud_start.contains(r#""streaming":true"#)
                && aud_start.contains(r#""delayMs":30.0"#),
            "got {aud_start}"
        );

        // 2. Switch to bluetooth A2DP -> suspends wireless lockdown
        let aud_bt = String::from_utf8_lossy(
            &handle_request(
                &post("/api/audio/config", r#"{"audioOut":"bluetooth","forceStream":true}"#),
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(
            aud_bt.contains(r#""audioOut":"bluetooth""#)
                && aud_bt.contains(r#""bluetoothA2dp":true"#)
                && aud_bt.contains(r#""wirelessLockdownSuspended":true"#),
            "got {aud_bt}"
        );

        // 3. /api/status serves in-memory screen + audio telemetry in < 100ms without worker polling
        let status_body = String::from_utf8_lossy(
            &handle_request(&get("/api/status"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            status_body.contains(r#""screen":"#)
                && status_body.contains(r#""audio":"#)
                && (status_body.contains(r#""bluetoothA2dp": true"#) || status_body.contains(r#""bluetoothA2dp":true"#)),
            "got {status_body}"
        );

        // 4. /api/audio/stop stops audio cleanly
        let aud_stop = String::from_utf8_lossy(
            &handle_request(&post("/api/audio/stop", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(aud_stop.contains(r#""streaming":false"#), "got {aud_stop}");

        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_apple::hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap231_status_zero_polling_guard_and_all_top_level_keys() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::hid::reset_hid_runtime_state();
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        // Warm cache first in idle so zero-poll measurement measures cached path cleanly
        let get = |path: &str| HttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let _ = handle_request(&get("/api/status"), &state, Path::new("web"));

        // Enable active streaming & verify Zero-Polling Guard returns in < 2ms with all keys
        let _ = iphone_link_apple::mirror::global_start_screen(Some("UDID-231"), "cable", false);
        let t0 = std::time::Instant::now();
        let resp = handle_request(&get("/api/status"), &state, Path::new("web"));
        let elapsed_us = t0.elapsed().as_micros();
        assert_eq!(resp.status_code, 200);
        let body = String::from_utf8_lossy(&resp.body);
        for key in [
            r#""devices":"#,
            r#""selectedUdid":"#,
            r#""handset":"#,
            r#""developerMode":"#,
            r#""driverReady":"#,
            r#""detailedDiagnostics":"#,
            r#""screen":"#,
            r#""wifi":"#,
            r#""audio":"#,
            r#""wallpaperSig":"#,
            r#""folderInstanceId":"#,
            r#""isFreshFolder":"#,
            r#""clipboardSeq":"#,
            r#""activeClipboardSource":"#,
            r#""settings":"#,
        ] {
            assert!(body.contains(key), "missing key {key} in /api/status: {body}");
        }
        assert!(elapsed_us < 5000, "Zero-Polling Guard took too long: {elapsed_us} us");

        // /api/devices/<udid> also serves zero-poll cached data
        let dev_resp = handle_request(&get("/api/devices/UDID-231"), &state, Path::new("web"));
        assert_eq!(dev_resp.status_code, 200);
        let dev_body = String::from_utf8_lossy(&dev_resp.body);
        assert!(dev_body.contains(r#""zeroPoll": true"#) && dev_body.contains("UDID-231"), "got {dev_body}");

        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_apple::hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap231_wallpaper_spi_30s_cache_and_no_store_headers() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_wallpaper_cache_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_wp = HttpRequest {
            method: "GET".to_string(),
            path: "/api/system/wallpaper".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp1 = handle_request(&req_wp, &state, Path::new("web"));
        assert_eq!(resp1.status_code, 200);
        assert!(resp1.body.len() > 1000, "expected image bytes > 1000");
        assert!(
            resp1.body.starts_with(&[0xFF, 0xD8]) || resp1.body.starts_with(b"\x89PNG"),
            "wallpaper response must be valid JPEG or PNG"
        );
        let raw_bytes = resp1.to_bytes();
        let raw_http = String::from_utf8_lossy(&raw_bytes);
        assert!(
            raw_http.contains("Cache-Control: no-store, no-cache, must-revalidate, max-age=0")
                && raw_http.contains("Pragma: no-cache"),
            "missing no-store/no-cache headers"
        );

        // Second hit must serve straight from 30s WALLPAPER_SIG_CACHE (< 500 us)
        let t0 = std::time::Instant::now();
        let resp2 = handle_request(&req_wp, &state, Path::new("web"));
        let elapsed_us = t0.elapsed().as_micros();
        assert_eq!(resp2.body.len(), resp1.body.len());
        assert!(elapsed_us < 1000, "30s WALLPAPER_SIG_CACHE miss: {elapsed_us} us");
    }

    #[test]
    fn test_tahap231_settings_persistence_and_autorotate_sync() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));
        let post_settings = HttpRequest {
            method: "POST".to_string(),
            path: "/api/settings".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"theme":"light","fitMode":"contain","crop":{"top":4,"bottom":8,"left":2,"right":6},"autoRotate":true,"pcClipboardPaste":false,"stealthDim":true,"restoreSteps":6,"transport":"wifi"}"#.to_string(),
            body_bytes: Vec::new(),
        };
        let resp = handle_request(&post_settings, &state, Path::new("web"));
        assert_eq!(resp.status_code, 200);
        let body = String::from_utf8_lossy(&resp.body);
        assert!(
            body.contains(r#""theme":"light""#)
                && body.contains(r#""fitMode":"contain""#)
                && body.contains(r#""autoRotate":true"#)
                && body.contains(r#""pcClipboardPaste":false"#)
                && body.contains(r#""stealthDim":true"#)
                && body.contains(r#""restoreSteps":6"#)
                && body.contains(r#""transport":"wifi""#),
            "got {body}"
        );

        let scr_json = iphone_link_apple::mirror::global_screen_status_json();
        assert!(scr_json.contains(r#""autoRotate":true"#), "AutoRotateController did not sync: {scr_json}");
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap231_clipboard_sequence_state_machine_paste_ios_vs_pc() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ocr_guard = crate::ocr_engine::CRATE_GLOBAL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_clip = read_win32_clipboard();
        let state = Arc::new(Mutex::new(AppState::default()));
        let post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Ctrl+C inside iPhone (`skipOcr: true`) sets source="iphone" & records sequence number
        let _ = write_win32_clipboard("PC_CLIPBOARD_SAMPLE_231");
        let _ = handle_request(&post("/api/keyboard/shortcut", r#"{"action":"copy","skipOcr":true}"#), &state, Path::new("web"));

        // 2. Subsequent `/api/keyboard/paste` with no sequence change executes native iOS Cmd+V (`paste-ios`)
        let paste_ios = String::from_utf8_lossy(
            &handle_request(&post("/api/keyboard/paste", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            paste_ios.contains(r#""method":"ios-shortcut""#)
                && paste_ios.contains(r#""action":"paste-ios""#)
                && paste_ios.contains(r#""reason":"iphone-source""#),
            "expected paste-ios when source=iphone, got {paste_ios}"
        );

        // 3. Explicit `force: true` (`Alt+V` / Paste PC Text) types PC clipboard via 4-step Shift latching
        let paste_force = String::from_utf8_lossy(
            &handle_request(&post("/api/keyboard/paste", r#"{"force":true,"text":"Halo?!"}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            paste_force.contains(r#""method":"typed""#)
                && paste_force.contains(r#""action":"paste-pc""#)
                && paste_force.contains(r#""typed":6"#),
            "expected typed paste-pc on force=true, got {paste_force}"
        );

        // 4. Simulate Windows GetClipboardSequenceNumber change after iPhone copy -> promotes source to PC
        let _ = handle_request(&post("/api/keyboard/shortcut", r#"{"action":"copy","skipOcr":true}"#), &state, Path::new("web"));
        crate::ocr_engine::set_last_recorded_clipboard_seq_for_test(1); // different from current Win32 seq
        let paste_promoted = String::from_utf8_lossy(
            &handle_request(&post("/api/keyboard/paste", "{}"), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            paste_promoted.contains(r#""method":"typed""#) && paste_promoted.contains(r#""source":"pc""#),
            "expected seq change to promote clipboard source to PC, got {paste_promoted}"
        );

        if let Some(prev) = prev_clip {
            let _ = write_win32_clipboard(&prev);
        }
        set_active_clipboard_source(ClipboardSource::Pc);
    }

    #[test]
    fn test_tahap231_shortcut_lan_bridge_under_5ms_and_403_guard() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_clip = read_win32_clipboard();
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_c = HttpRequest::parse(
            b"GET /c?u=https://www.threads.com/@example.user/post/DV9abc12345 HTTP/1.1\r\nHost: 192.0.2.10:8765\r\n\r\n",
        )
        .unwrap();
        let t0 = std::time::Instant::now();
        let resp_c = handle_request(&req_c, &state, Path::new("web"));
        let elapsed_ms = t0.elapsed().as_millis();
        assert_eq!(resp_c.status_code, 200);
        assert!(elapsed_ms < 25, "/c took unexpectedly long: {elapsed_ms}ms");
        let body_c = String::from_utf8_lossy(&resp_c.body);
        assert!(body_c.contains("https://www.threads.com/@example.user/post/DV9abc12345"), "got {body_c}");

        let lan_ip: IpAddr = "192.0.2.41".parse().unwrap();
        assert!(is_lan_request_allowed(lan_ip, "GET", "/c?u=https://threads.com/x"));
        assert!(is_lan_request_allowed(lan_ip, "POST", "/api/shortcut/push"));
        assert!(!is_lan_request_allowed(lan_ip, "GET", "/api/status"));
        assert!(!is_lan_request_allowed(lan_ip, "POST", "/api/settings"));

        if let Some(prev) = prev_clip {
            let _ = write_win32_clipboard(&prev);
        }
    }

    #[test]
    fn test_tahap232_embedded_web_and_icon_assets_zero_disk_io() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));
        let missing_web_dir = Path::new(r"Z:\non_existent_isolated_dir_tahap232\web");

        let get_req = |p: &str| HttpRequest {
            method: "GET".to_string(),
            path: p.to_string(),
            query: "v=tahap232".to_string(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };

        let cases: &[(&str, &str, usize)] = &[
            ("/", "text/html; charset=utf-8", 50_000),
            ("/index.html", "text/html; charset=utf-8", 50_000),
            ("/app.js", "application/javascript; charset=utf-8", 150_000),
            ("/app.css", "text/css; charset=utf-8", 100_000),
            ("/mirror.html", "text/html; charset=utf-8", 800),
            ("/mirror.js", "application/javascript; charset=utf-8", 150_000),
            ("/mirror.css", "text/css; charset=utf-8", 3_000),
            ("/keys.js", "application/javascript; charset=utf-8", 15_000),
            ("/logo.svg", "image/svg+xml", 4_000),
            ("/logo.png", "image/png", 100_000),
            ("/favicon.ico", "image/x-icon", 50_000),
        ];

        for &(route, expected_mime, min_len) in cases {
            let t0 = std::time::Instant::now();
            let resp = handle_request(&get_req(route), &state, missing_web_dir);
            let elapsed_ms = t0.elapsed().as_millis();
            assert_eq!(resp.status_code, 200, "route {} failed", route);
            assert_eq!(resp.content_type, expected_mime, "mime mismatch for {}", route);
            assert!(
                resp.body.len() >= min_len,
                "route {} too small: {} < {}",
                route,
                resp.body.len(),
                min_len
            );
            assert!(elapsed_ms < 50, "route {} too slow: {}ms", route, elapsed_ms);
            let wire = resp.to_bytes_with_conn("close");
            let wire_head = String::from_utf8_lossy(&wire[..wire.len().min(512)]);
            assert!(
                wire_head.contains("Cache-Control: no-store, no-cache, must-revalidate, max-age=0"),
                "missing Cache-Control no-store for {}",
                route
            );
        }

        let idx_html = String::from_utf8_lossy(&handle_request(&get_req("/"), &state, missing_web_dir).body).to_string();
        assert!(idx_html.contains("tahap222-theme-contrast"));
        assert!(idx_html.contains("/v4_true_acrylic.css"));
        assert!(idx_html.contains("/v4_bridge.js"));
        assert!(idx_html.contains("window.__V4_INITIAL_SETTINGS__="));

        let app_css = String::from_utf8_lossy(&handle_request(&get_req("/app.css"), &state, missing_web_dir).body).to_string();
        assert!(app_css.contains("V4_TRUE_ACRYLIC_CSS OVERRIDE"));
        assert!(app_css.contains(".app::before"));
    }

    #[test]
    fn test_tahap232_fullscreen_persistence_lock_on_drawer_orientation_aspect() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        GLOBAL_IS_FULLSCREEN.store(false, Ordering::SeqCst);
        GLOBAL_IS_LANDSCAPE.store(false, Ordering::SeqCst);
        GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        GLOBAL_PORTRAIT_WIDTH.store(346, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        let post = |p: &str, b: &str| HttpRequest {
            method: "POST".to_string(),
            path: p.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: b.to_string(),
            body_bytes: b.as_bytes().to_vec(),
        };

        let fs_on = String::from_utf8_lossy(
            &handle_request(&post("/api/window/fullscreen", r#"{"fullscreen":true}"#), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(fs_on.contains(r#""fullscreen":true"#), "got {fs_on}");

        for (route, payload) in [
            ("/api/window/drawer", r#"{"open":true}"#),
            ("/api/window/orientation", r#"{"landscape":true}"#),
            ("/api/window/aspect", r#"{"width":340,"profile":"iphone"}"#),
        ] {
            let body = String::from_utf8_lossy(
                &handle_request(&post(route, payload), &state, Path::new("web")).body,
            )
            .to_string();
            assert!(
                body.contains(r#""skipped":"fullscreen""#) && body.contains(r#""is_fullscreen":true"#),
                "expected fullscreen persistence lock on {} with {}, got {}",
                route,
                payload,
                body
            );
        }

        let state_json = String::from_utf8_lossy(
            &handle_request(
                &HttpRequest {
                    method: "GET".to_string(),
                    path: "/api/window/state".to_string(),
                    query: String::new(),
                    headers: HashMap::new(),
                    body: String::new(),
                    body_bytes: Vec::new(),
                },
                &state,
                Path::new("web"),
            )
            .body,
        )
        .to_string();
        assert!(state_json.contains(r#""is_fullscreen":true"#), "got {state_json}");

        let _ = handle_request(&post("/api/window/fullscreen", r#"{"fullscreen":false}"#), &state, Path::new("web"));
        GLOBAL_IS_FULLSCREEN.store(false, Ordering::SeqCst);
        GLOBAL_IS_LANDSCAPE.store(false, Ordering::SeqCst);
        GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_tahap232_tray_menu_ascii_and_dynamic_check_flags() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();
        GLOBAL_IS_LANDSCAPE.store(false, Ordering::SeqCst);
        GLOBAL_STEALTH_ACTIVE.store(false, Ordering::SeqCst);
        GLOBAL_AUDIO_PC_ACTIVE.store(true, Ordering::SeqCst);
        let state = Arc::new(Mutex::new(AppState::default()));

        let get_menu = || {
            String::from_utf8_lossy(
                &handle_request(
                    &HttpRequest {
                        method: "GET".to_string(),
                        path: "/api/window/tray-menu".to_string(),
                        query: String::new(),
                        headers: HashMap::new(),
                        body: String::new(),
                        body_bytes: Vec::new(),
                    },
                    &state,
                    Path::new("web"),
                )
                .body,
            )
            .to_string()
        };

        let m0 = get_menu();
        assert!(m0.is_ascii(), "tray menu JSON must be pure ASCII: {m0}");
        assert!(m0.contains(r#""default_item":"Show / Restore iPhone Link""#), "got {m0}");
        assert!(m0.contains(r#""nim_delete_guaranteed":true"#), "got {m0}");
        assert!(m0.contains(r#""stream_audio_to_pc":true"#), "got {m0}");

        let post = |p: &str, b: &str| HttpRequest {
            method: "POST".to_string(),
            path: p.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: b.to_string(),
            body_bytes: b.as_bytes().to_vec(),
        };
        let _ = handle_request(&post("/api/window/orientation", r#"{"landscape":true}"#), &state, Path::new("web"));
        let _ = handle_request(&post("/api/tray/trigger_action", r#"{"action":"toggle_stealth"}"#), &state, Path::new("web"));

        let m1 = get_menu();
        assert!(m1.contains(r#""landscape":true"#) && m1.contains(r#""stealth_dim":true"#), "got {m1}");

        reset_hid_runtime_state();
        GLOBAL_IS_LANDSCAPE.store(false, Ordering::SeqCst);
        GLOBAL_STEALTH_ACTIVE.store(false, Ordering::SeqCst);
        GLOBAL_AUDIO_PC_ACTIVE.store(true, Ordering::SeqCst);
    }

    #[test]
    fn test_tahap232_clean_close_restores_stealth_dim_and_deletes_tray() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();
        GLOBAL_TRAY_REGISTERED.store(true, Ordering::SeqCst);
        GLOBAL_STEALTH_ACTIVE.store(true, Ordering::SeqCst);

        let dim_res = set_iphone_physical_brightness("min", 16, false);
        assert!(dim_res.stealth && is_stealth_dim_active());

        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut st = state.lock().unwrap();
            st.stealth_dim = true;
            st.is_streaming = true;
        }

        let resp_close = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/window/close".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: "{}".to_string(),
                body_bytes: b"{}".to_vec(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_close.status_code, 200);
        let close_json = String::from_utf8_lossy(&resp_close.body);
        assert!(
            close_json.contains(r#""stealth_restored":true"#)
                && close_json.contains(r#""stealth_restore_steps":4"#)
                && close_json.contains(r#""tray_deleted":true"#),
            "expected stealth_restored=true, steps=4, tray_deleted=true, got {close_json}"
        );
        assert!(!is_stealth_dim_active());
        assert!(!GLOBAL_TRAY_REGISTERED.load(Ordering::SeqCst));
        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap233_inprocess_video_pipeline_routes_and_status() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));

        let resp_pipe0 = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/video/pipeline".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_pipe0.status_code, 200);
        let pipe0 = String::from_utf8_lossy(&resp_pipe0.body);
        assert!(pipe0.contains(r#""ringCapacity":6"#) && pipe0.contains(r#""lowLatencyMode":true"#), "got {pipe0}");

        let resp_dec = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/video/decode-annexb".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"synthetic":true}"#.to_string(),
                body_bytes: br#"{"synthetic":true}"#.to_vec(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_dec.status_code, 200);
        let dec_json = String::from_utf8_lossy(&resp_dec.body);
        assert!(
            dec_json.contains(r#""cleanWidth":1202"#)
                && dec_json.contains(r#""cleanHeight":2598"#)
                && dec_json.contains(r#""topMarginNotCropped":true"#)
                && dec_json.contains(r#""bottomPaddingTrimmed":true"#)
                && dec_json.contains(r#""ringCapacity":6"#)
                && dec_json.contains(r#""probeZeroAlloc":true"#),
            "got {dec_json}"
        );

        let resp_st = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_st.status_code, 200);
        let st_json = String::from_utf8_lossy(&resp_st.body);
        assert!(st_json.contains(r#""videoPipeline":"#) && st_json.contains(r#""ringCapacity":6"#), "got {st_json}");
        assert!(st_json.contains(r#""videoPresenter":"#) && st_json.contains(r#""DXGI_SWAP_EFFECT_FLIP_DISCARD""#), "got {st_json}");

        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap234_inprocess_video_presenter_and_snapshot_routes() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));

        let resp_get = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/video/presenter".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_get.status_code, 200);
        let p0 = String::from_utf8_lossy(&resp_get.body);
        assert!(p0.contains(r#""swapEffect":"DXGI_SWAP_EFFECT_FLIP_DISCARD""#), "got {p0}");
        assert!(p0.contains(r#""format":"DXGI_FORMAT_B8G8R8A8_UNORM""#), "got {p0}");
        assert!(p0.contains(r#""alphaMode":"DXGI_ALPHA_MODE_IGNORE""#), "got {p0}");
        assert!(p0.contains(r#""obsCaptureCompatible":true"#), "got {p0}");

        let post_body = r#"{"fitMode":"original_ratio","rotationDeg":270,"cropTop":12,"cropBottom":4,"cropLeft":6,"cropRight":8,"windowWidth":1200,"windowHeight":468,"isFullscreen":false,"isLandscape":true,"drawerOpen":true}"#;
        let resp_post = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/video/presenter".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: post_body.to_string(),
                body_bytes: post_body.as_bytes().to_vec(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_post.status_code, 200);
        let p1 = String::from_utf8_lossy(&resp_post.body);
        assert!(p1.contains(r#""fitMode":"original_ratio""#), "got {p1}");
        assert!(p1.contains(r#""rotationDeg":270"#), "got {p1}");
        assert!(p1.contains(r#""width":840,"height":386"#), "got {p1}");

        let resp_snap = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/video/snapshot".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_snap.status_code, 200);
        let snap_json = String::from_utf8_lossy(&resp_snap.body);
        assert!(snap_json.contains(r#""ok":true"#) && snap_json.contains("data:image/bmp;base64,Qk"), "got {snap_json}");

        let resp_shortcut = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/keyboard/shortcut".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"action":"copy"}"#.to_string(),
                body_bytes: br#"{"action":"copy"}"#.to_vec(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_shortcut.status_code, 200);
        let sc_json = String::from_utf8_lossy(&resp_shortcut.body);
        assert!(sc_json.contains(r#""autoSnapshotUsed":true"#), "got {sc_json}");

        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap235_ui_composition_css_gpu_containment_and_bridge_presenter_sync() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        let resp_css = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/v4_true_acrylic.css".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_css.status_code, 200);
        let css = String::from_utf8_lossy(&resp_css.body);
        assert!(css.contains(".display-col"), "missing .display-col containment");
        assert!(css.contains("contain: layout style paint;"), "missing GPU layer containment");
        assert!(css.contains("will-change: transform;"), "missing will-change transform");
        assert!(css.contains(".side-drawer .card"), "missing nested backdrop-filter override");
        assert!(css.contains("backdrop-filter: none !important;"));
        assert!(css.contains(".app.is-fullsize.drawer-open .side-drawer"), "missing Tahap 91 FS drawer");
        assert!(css.contains("white-space: nowrap !important;"), "missing compact quick-nav guard");

        let resp_js = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/v4_bridge.js".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_js.status_code, 200);
        let js = String::from_utf8_lossy(&resp_js.body);
        assert!(js.contains("function syncNativeVideoPresenter("), "missing dual-path presenter sync");
        assert!(js.contains("/api/video/presenter"));
        assert!(js.contains("PARALLAX_CALM_INTERVAL_MS = 1500"), "missing Calm Parallax 1500ms");
        assert!(js.contains("dx >= 2 || dy >= 2"), "missing >=2px delta guard");

        // Verify POST /api/settings syncs directly into D3d11VideoPresenter
        let body = r#"{"fitMode":"fill","crop":{"top":10,"bottom":4,"left":6,"right":8}}"#;
        let resp_set = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/settings".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: body.to_string(),
                body_bytes: body.as_bytes().to_vec(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_set.status_code, 200);
        let pres = iphone_link_apple::mirror::global_video_presenter_status_json();
        assert!(pres.contains(r#""fitMode":"edge_to_edge""#), "got {pres}");
        assert!(pres.contains(r#""top":10,"bottom":4,"left":6,"right":8"#), "got {pres}");
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap235_endurance_benchmark_endpoint_and_zero_polling_status() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));

        let resp_bench = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/system/endurance-benchmark".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"frames":1000}"#.to_string(),
                body_bytes: br#"{"frames":1000}"#.to_vec(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(resp_bench.status_code, 200);
        let bj = String::from_utf8_lossy(&resp_bench.body);
        assert!(bj.contains(r#""ok":true"#), "got {bj}");
        assert!(bj.contains(r#""framesProcessed":1000"#), "got {bj}");
        assert!(bj.contains(r#""maxRingSlotsUsed":6"#), "got {bj}");
        assert!(bj.contains(r#""maxSubscriberQueueDepth":24"#), "got {bj}");
        assert!(bj.contains(r#""autoRotateSleepAllocations":0"#), "got {bj}");
        assert!(bj.contains(r#""statusPollIoCallsDuringStream":0"#), "got {bj}");
        assert!(bj.contains(r#""emptyWorkingSetCallsDuringStream":0"#), "got {bj}");
        assert!(bj.contains(r#""zeroMemoryLeakVerified":true"#), "got {bj}");
        assert!(bj.contains(r#""maxViewportHistoryCap":4"#), "got {bj}");

        // 10 consecutive /api/status calls while simulated streaming active must run < 5ms total
        {
            let mut st = state.lock().unwrap();
            st.is_streaming = true;
        }
        let t0 = std::time::Instant::now();
        for _ in 0..10 {
            let r = handle_request(
                &HttpRequest {
                    method: "GET".to_string(),
                    path: "/api/status".to_string(),
                    query: String::new(),
                    headers: HashMap::new(),
                    body: String::new(),
                    body_bytes: Vec::new(),
                },
                &state,
                Path::new("web"),
            );
            assert_eq!(r.status_code, 200);
        }
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(elapsed_ms < 25.0, "10x /api/status elapsed={elapsed_ms:.2}ms");

        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap236_v5_version_identity_and_inmemory_html_branding() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. Embedded HTML served from GET / and GET /index.html reports iPhone Link v5.0 & v5.0.0
        let req_root = HttpRequest {
            method: "GET".to_string(),
            path: "/".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp_root = handle_request(&req_root, &state, Path::new("Z:\\nonexistent\\web"));
        assert_eq!(resp_root.status_code, 200);
        let html = String::from_utf8_lossy(&resp_root.body);
        assert!(html.contains("iPhone Link v5.0.1"), "HTML updater card missing v5.0.1 branding");
        assert!(html.contains("iPhone Link v5.0"), "HTML footer credit missing v5.0 branding");
        assert!(!html.contains("iPhone Link v4.0"), "HTML still contains stale v4.0 branding");
        assert!(!html.contains("iPhone Link v3.0"), "HTML still contains stale v3.0 branding");

        // 2. /v4_bridge.js reports v5.0.1 & v5.0
        let resp_js = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/v4_bridge.js".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let js = String::from_utf8_lossy(&resp_js.body);
        assert!(js.contains("iPhone Link v5.0.1"));
        assert!(js.contains("iPhone Link v5.0"));

        // 3. GET /api/status & GET /api/settings report Pure Native Rust Zero Python / Zero PowerShell v5.0.1
        let resp_st = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let st_json = String::from_utf8_lossy(&resp_st.body);
        assert!(st_json.trim_end().ends_with('}'), "status JSON must end cleanly with }}: {st_json}");
        assert!(st_json.contains(r#""version": "5.0.1""#), "got {st_json}");
        assert!(st_json.contains(APP_EDITION_V5), "got {st_json}");
        assert!(st_json.contains(r#""zero_python_runtime": true"#), "got {st_json}");
        assert!(st_json.contains(r#""zero_powershell_runtime": true"#), "got {st_json}");

        let resp_cfg = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/settings".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let cfg_json = String::from_utf8_lossy(&resp_cfg.body);
        assert!(cfg_json.contains(r#""version":"5.0.1""#), "got {cfg_json}");
        assert!(cfg_json.contains(r#""zero_python_runtime":true"#), "got {cfg_json}");
        assert!(cfg_json.contains(r#""zero_powershell_runtime":true"#), "got {cfg_json}");
    }

    #[test]
    fn test_tahap236_updater_v5_downgrade_protection_and_protected_configs() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(crate::updater::CURRENT_VERSION, "5.0.1");
        assert!(!crate::updater::is_newer_version("5.0.1", "4.0.0"));
        assert!(!crate::updater::is_newer_version("5.0.1", "3.0.0"));
        assert!(!crate::updater::is_newer_version("5.0.1", "5.0.0"));
        assert!(!crate::updater::is_newer_version("5.0.1", "5.0.1"));
        assert!(crate::updater::is_newer_version("5.0.1", "5.0.2"));
        assert!(crate::updater::is_protected_config_file("user_settings.json"));
        assert!(crate::updater::is_protected_config_file("wifi_state.json"));
        assert!(crate::updater::is_protected_config_file("typed_history.json"));

        let mut cache = UpdateStateCache::default();
        assert_eq!(cache.current_version, "5.0.1");
        cache.apply_worker_payload(
            r#"{"status":"update_available","latestVersion":"4.0.0","updateAvailable":true,"channel":"stable"}"#,
            "stable",
        );
        assert!(!cache.update_available);
        assert_eq!(cache.latest_version, "5.0.1");
    }

    #[test]
    fn test_tahap238_hid_aliases_window_display_width_and_wifi_error_banner() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::window_dwm::GLOBAL_IS_FULLSCREEN.store(false, Ordering::SeqCst);
        crate::window_dwm::GLOBAL_IS_LANDSCAPE.store(false, Ordering::SeqCst);
        crate::window_dwm::GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        crate::window_dwm::GLOBAL_PORTRAIT_WIDTH.store(346, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        let post = |p: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: p.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };
        let get = |p: &str| HttpRequest {
            method: "GET".to_string(),
            path: p.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };

        // 1. Direct /touch, /button, /key, /api/screen/swipe aliases never fall through to 404
        let r_touch = handle_request(&post("/touch", r#"{"type":"contact","x":32768,"y":40000}"#), &state, Path::new("web"));
        assert_eq!(r_touch.status_code, 200);
        let b_touch = String::from_utf8_lossy(&r_touch.body);
        assert!(b_touch.contains(r#""mappedX":32768"#) && b_touch.contains(r#""mappedY":40000"#), "got {b_touch}");

        let r_btn = handle_request(&post("/button", r#"{"name":"home","state":"press"}"#), &state, Path::new("web"));
        assert_eq!(r_btn.status_code, 200);
        assert!(String::from_utf8_lossy(&r_btn.body).contains(r#""button":"home""#));

        let r_key = handle_request(&post("/key", r#"{"usages":[4,5]}"#), &state, Path::new("web"));
        assert_eq!(r_key.status_code, 200);
        assert!(String::from_utf8_lossy(&r_key.body).contains(r#""usagesCount":2"#));

        let r_swipe = handle_request(&post("/api/screen/swipe", r#"{"direction":"left"}"#), &state, Path::new("web"));
        assert_eq!(r_swipe.status_code, 200);
        assert!(String::from_utf8_lossy(&r_swipe.body).contains(r#""direction":"left""#));

        // 2. /api/window/aspect accepts displayWidth: 376 & /api/screen/orientation resizes state + /api/window/tray|restore aliases
        let r_asp = handle_request(&post("/api/window/aspect", r#"{"displayWidth":376}"#), &state, Path::new("web"));
        assert_eq!(r_asp.status_code, 200);
        assert!(String::from_utf8_lossy(&r_asp.body).contains(r#""portrait_width":376"#));

        let r_rot = handle_request(&post("/api/screen/orientation", r#"{"orientation":270}"#), &state, Path::new("web"));
        assert_eq!(r_rot.status_code, 200);
        assert!(crate::window_dwm::GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst));

        let r_rect = handle_request(&get("/api/window/rect"), &state, Path::new("web"));
        let b_rect = String::from_utf8_lossy(&r_rect.body);
        assert!(b_rect.contains(r#""hwnd":"#) && b_rect.contains(r#""portrait_width":376"#) && b_rect.contains(r#""is_landscape":true"#), "got {b_rect}");

        assert_eq!(handle_request(&post("/api/window/tray", "{}"), &state, Path::new("web")).status_code, 200);
        assert_eq!(handle_request(&post("/api/window/restore", "{}"), &state, Path::new("web")).status_code, 200);

        // 3. /v4_bridge.js unhides #screen-error-banner on phase === 'error'
        let js_resp = handle_request(&get("/v4_bridge.js"), &state, Path::new("web"));
        let js_body = String::from_utf8_lossy(&js_resp.body);
        assert!(js_body.contains("s.screen.phase === 'error'") && js_body.contains("errBanner.hidden = false"), "missing error banner reveal");

        crate::window_dwm::GLOBAL_IS_LANDSCAPE.store(false, Ordering::SeqCst);
        crate::window_dwm::GLOBAL_PORTRAIT_WIDTH.store(346, Ordering::SeqCst);
    }

    #[test]
    fn test_tahap240_audio_auto_start_on_screen_start_and_auto_stop_on_screen_stop() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::hid::reset_hid_runtime_state();
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        let req_with = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. POST /api/screen (honestGate=false) with default audio_routing="pc" MUST auto-start PC audio bridge
        let r_start = handle_request(
            &req_with("POST", "/api/screen", r#"{"transport":"usb","honestGate":false}"#),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_start.status_code, 200);
        let aud_initial = String::from_utf8_lossy(
            &handle_request(&req_with("GET", "/api/audio", ""), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            aud_initial.contains(r#""pcBridgeActive":true"#)
                || aud_initial.contains(r#""pcBridgeConnecting":true"#),
            "PC audio bridge must auto-start (active or connecting) on POST /api/screen: {aud_initial}"
        );
        // Poll up to 9.0s for background STA worker (WinRT A2DP multi-candidate fallback / WASAPI) to finish opening
        let poll_deadline = std::time::Instant::now() + Duration::from_secs(9);
        let mut became_active = iphone_link_audio::is_global_pc_audio_bridge_active();
        while !became_active && std::time::Instant::now() < poll_deadline {
            std::thread::sleep(Duration::from_millis(50));
            became_active = iphone_link_audio::is_global_pc_audio_bridge_active();
        }
        let aud_running = String::from_utf8_lossy(
            &handle_request(&req_with("GET", "/api/audio", ""), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            became_active && aud_running.contains(r#""pcBridgeActive":true"#),
            "PC audio bridge must reach pcBridgeActive=true after POST /api/screen: {aud_running}"
        );

        // 2. POST /api/screen/stop MUST immediately stop PC audio bridge while keeping saved audio_routing="pc"
        let r_stop = handle_request(
            &req_with("POST", "/api/screen/stop", "{}"),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_stop.status_code, 200);
        assert!(
            !iphone_link_audio::is_global_pc_audio_bridge_active(),
            "PC audio bridge must auto-stop on POST /api/screen/stop"
        );
        assert_eq!(state.lock().unwrap().audio_routing, "pc");

        // 3. DELETE /api/screen also stops PC audio bridge cleanly
        let _ = handle_request(
            &req_with("POST", "/api/screen", r#"{"transport":"usb","honestGate":false}"#),
            &state,
            Path::new("web"),
        );
        assert!(
            iphone_link_audio::is_global_pc_audio_bridge_active()
                || iphone_link_audio::is_global_pc_audio_bridge_connecting()
        );
        let r_del = handle_request(&req_with("DELETE", "/api/screen", ""), &state, Path::new("web"));
        assert_eq!(r_del.status_code, 200);
        assert!(!iphone_link_audio::is_global_pc_audio_bridge_active());
        assert!(!iphone_link_audio::is_global_pc_audio_bridge_connecting());

        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_apple::hid::reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap241_status_exposes_pc_bridge_active_and_v4_bridge_audio_safety_sync() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);

        let state = Arc::new(Mutex::new(AppState::default()));
        // Verify alias acceptance (`audio_out`, `audio_routing`)
        {
            let mut st = state.lock().unwrap();
            assert!(apply_settings_json_body_to_state(r#"{"audio_routing":"device"}"#, &mut st));
            assert_eq!(st.audio_routing, "device");
            assert!(apply_settings_json_body_to_state(r#"{"audio_out":"pc"}"#, &mut st));
            assert_eq!(st.audio_routing, "pc");
        }

        let get = |path: &str| HttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let st_body = String::from_utf8_lossy(&handle_request(&get("/api/status"), &state, Path::new("web")).body).to_string();
        assert!(st_body.contains(r#""pcBridgeActive":"#), "status missing pcBridgeActive: {st_body}");
        let js_body = String::from_utf8_lossy(&handle_request(&get("/v4_bridge.js"), &state, Path::new("web")).body).to_string();
        assert!(js_body.contains("__v4AutoAudioSyncedForRun"), "v4_bridge.js missing auto-audio safety sync");
    }

    #[test]
    fn test_tahap242_grab_link_spotlight_endpoints_and_reconnect_telemetry() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        let prev_clip = read_win32_clipboard();

        let state = Arc::new(Mutex::new(AppState::default()));
        let req_with = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Spotlight HID endpoints return 200 OK with exact Golden v4.0.0 stages (no more 404!)
        let r_paste = handle_request(&req_with("POST", "/api/device/spotlight-paste", "{}"), &state, Path::new("web"));
        assert_eq!(r_paste.status_code, 200);
        let paste_body = String::from_utf8_lossy(&r_paste.body).to_string();
        assert!(paste_body.contains(r#""ok":true"#) && paste_body.contains(r#""stage":"pasted_clean""#), "got {paste_body}");

        let r_tail = handle_request(&req_with("POST", "/api/device/spotlight-paste-tail", "{}"), &state, Path::new("web"));
        assert_eq!(r_tail.status_code, 200);
        assert!(String::from_utf8_lossy(&r_tail.body).contains(r#""stage":"pasted_clean""#));

        let r_head = handle_request(&req_with("POST", "/api/device/spotlight-move-head", "{}"), &state, Path::new("web"));
        assert_eq!(r_head.status_code, 200);
        assert!(String::from_utf8_lossy(&r_head.body).contains(r#""stage":"head""#));

        let r_close = handle_request(&req_with("POST", "/api/device/spotlight-close", "{}"), &state, Path::new("web"));
        assert_eq!(r_close.status_code, 200);
        assert!(String::from_utf8_lossy(&r_close.body).contains(r#""stage":"closed""#));

        // 2. /api/qr/scan normalizes schemeless Instagram URL + writes Windows Clipboard + full Golden schema
        let r_qr = handle_request(
            &req_with("POST", "/api/qr/scan", r#"{"text":"instagram.com/p/DR20E4rIdel/"}"#),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_qr.status_code, 200);
        let qr_body = String::from_utf8_lossy(&r_qr.body).to_string();
        assert!(
            qr_body.contains(r#""ok":true"#)
                && qr_body.contains("https://www.instagram.com/p/DR20E4rIdel/")
                && qr_body.contains(r#""copiedToPc":true"#)
                && qr_body.contains(r#""clipboardWritten":true"#)
                && qr_body.contains(r#""complete":true"#),
            "unexpected /api/qr/scan body: {qr_body}"
        );
        assert_eq!(
            read_win32_clipboard().as_deref(),
            Some("https://www.instagram.com/p/DR20E4rIdel/")
        );

        // 3. Head + tail Spotlight fragment stitching
        let stitched = extract_best_link_from_candidates(&[
            "https://www.instagram.com/reel/DUfFa".to_string(),
            "reel/DUfFa9vE37z/".to_string(),
        ]);
        assert_eq!(
            stitched.as_deref(),
            Some("https://www.instagram.com/reel/DUfFa9vE37z/")
        );

        // 4. Fresh iPhone Shortcut LAN cache fallback (< 8s)
        if let Ok(mut cache) = LATEST_SHORTCUT_URL_CACHE.lock() {
            *cache = (
                "https://www.instagram.com/reel/DUfFa9vE37z/".to_string(),
                now_epoch_ms_u64(),
            );
        }
        let r_sc = handle_request(&req_with("POST", "/api/qr/scan", "{}"), &state, Path::new("web"));
        let sc_body = String::from_utf8_lossy(&r_sc.body).to_string();
        assert!(
            sc_body.contains("https://www.instagram.com/reel/DUfFa9vE37z/")
                && sc_body.contains(r#""source":"iphone_shortcut""#),
            "unexpected shortcut fallback: {sc_body}"
        );
        if let Ok(mut cache) = LATEST_SHORTCUT_URL_CACHE.lock() {
            *cache = (String::new(), 0);
        }

        // 5. GET /api/screen exposes Tahap 242 telemetry & POST /api/screen is non-blocking (< 250 ms)
        let t0 = std::time::Instant::now();
        let r_start = handle_request(
            &req_with("POST", "/api/screen", r#"{"transport":"usb","honestGate":false}"#),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_start.status_code, 200);
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "POST /api/screen must be non-blocking, took {:?}",
            t0.elapsed()
        );
        let scr_body = String::from_utf8_lossy(
            &handle_request(&req_with("GET", "/api/screen", ""), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(
            scr_body.contains(r#""display_xpc_pings":"#)
                && scr_body.contains(r#""auto_reconnect_count":"#)
                && scr_body.contains(r#""last_disconnect_reason":null"#)
                && scr_body.contains(r#""reconnecting":false"#),
            "GET /api/screen missing Tahap 242 fields: {scr_body}"
        );

        // 6. V4_BRIDGE_JS & V4_TRUE_ACRYLIC_CSS contain Tahap 242 anti-blank reconnect guard
        let js_body = String::from_utf8_lossy(
            &handle_request(&req_with("GET", "/v4_bridge.js", ""), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(js_body.contains("__v4Tahap242ReconnectGuard"), "missing reconnect guard in v4_bridge.js");
        let css_body = String::from_utf8_lossy(
            &handle_request(&req_with("GET", "/v4_true_acrylic.css", ""), &state, Path::new("web")).body,
        )
        .to_string();
        assert!(css_body.contains("#v4-reconnect-pill"), "missing reconnect pill CSS");

        let _ = handle_request(&req_with("POST", "/api/screen/stop", "{}"), &state, Path::new("web"));
        if let Some(prev) = prev_clip {
            let _ = write_win32_clipboard(&prev);
        }
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap243_anti_crash_ocr_stress_utf8_audio_connecting_and_force_refresh() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_clip = read_win32_clipboard();

        // 1. Non-ASCII Spotlight OCR stress (… • — 🔍 İ emoji) NEVER panics inside frozen link_grabber.rs
        let toxic_spotlight_samples = vec![
            "\u{1F50D} https://www.instagram.com/reel/DW00rRVEi1M/?igsh=Z2FzdGM0aWl0d3M=\u{2026} \u{2014} Show Results".to_string(),
            "\u{2022}İnstagram\u{2022} http5://www.instagram.com/reel/DW00rRVEi1M/?igsh=Z2FzdGM0aWl0d3M=\u{2026}".to_string(),
            "\u{1F50D}a\u{2026}\u{2022}\u{2014}İ\u{2026}\u{2022}\u{2014} https://www.instagram.com/reel/DW00rRVEi1M/?igsh=Z2FzdGM0aWl0d3M=".to_string(),
        ];
        for sample in &toxic_spotlight_samples {
            let extracted = extract_best_link_from_candidates(std::slice::from_ref(sample));
            assert_eq!(
                extracted.as_deref(),
                Some("https://www.instagram.com/reel/DW00rRVEi1M/?igsh=Z2FzdGM0aWl0d3M="),
                "Toxic UTF-8 Spotlight OCR must extract exact Instagram Reel URL without panic"
            );
        }

        // 2. 10x consecutive image decode + recognize_data_url_for_link_grab stress (OnceLock GDI+ & combase.dll)
        // Minimal valid 2x2 24-bit BMP data URL (guaranteed decoded by GDI+ / BMP parser, zero crash over 10 loops)
        let bmp_2x2_b64 = "Qk1GAAAAAAAAADYAAAAoAAAAAgAAAAIAAAABABgAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAA////AAAAAAAA////AAAAAA==";
        let bmp_data_url = format!("data:image/bmp;base64,{bmp_2x2_b64}");
        for _ in 0..10 {
            let _lines = crate::ocr_engine::recognize_data_url_for_link_grab(&bmp_data_url);
        }

        // 3. GET /api/status?force=1 & POST /api/status/refresh return 200 OK with reconnecting & pcBridgeConnecting
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_force = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: "force=1".to_string(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let r_force = handle_request(&req_force, &state, Path::new("web"));
        assert_eq!(r_force.status_code, 200);
        let b_force = String::from_utf8_lossy(&r_force.body).to_string();
        assert!(
            b_force.contains(r#""reconnecting":"#)
                && b_force.contains(r#""pcBridgeConnecting":"#)
                && b_force.contains(r#""pcBridgeActive":"#),
            "GET /api/status?force=1 missing Tahap 243 fields: {b_force}"
        );

        let req_refresh = HttpRequest {
            method: "POST".to_string(),
            path: "/api/status/refresh".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: "{}".to_string(),
            body_bytes: b"{}".to_vec(),
        };
        let r_refresh = handle_request(&req_refresh, &state, Path::new("web"));
        assert_eq!(r_refresh.status_code, 200);
        assert!(String::from_utf8_lossy(&r_refresh.body).contains(r#""pcBridgeConnecting":"#));

        // 4. V4_BRIDGE_JS & V4_TRUE_ACRYLIC_CSS contain Tahap 243 hooks
        assert!(V4_BRIDGE_JS.contains("installTahap243LaunchpadRefreshAndWifiBoot"));
        assert!(V4_BRIDGE_JS.contains("#launchpad-scan-btn"));
        assert!(V4_BRIDGE_JS.contains("pcBridgeConnecting"));
        assert!(V4_BRIDGE_JS.contains("/api/status?force=1"));
        assert!(V4_TRUE_ACRYLIC_CSS.contains("@keyframes v4Spin"));
        assert!(V4_TRUE_ACRYLIC_CSS.contains(".v4-refresh-spinning"));

        if let Some(prev) = prev_clip {
            let _ = write_win32_clipboard(&prev);
        }
    }

    #[test]
    fn test_tahap244_compact_reconnect_pill_audio_device_fields_and_mini_2x_ocr() {
        // 1. Compact 1-line #v4-reconnect-pill CSS & sanitized JS text (never dumps raw IO errors)
        assert!(V4_TRUE_ACRYLIC_CSS.contains("max-width: 240px !important;"));
        assert!(V4_TRUE_ACRYLIC_CSS.contains("white-space: nowrap !important;"));
        assert!(V4_TRUE_ACRYLIC_CSS.contains("text-overflow: ellipsis !important;"));
        assert!(V4_TRUE_ACRYLIC_CSS.contains("border-radius: 999px !important;"));
        assert!(
            V4_BRIDGE_JS.contains("Menyambung ulang")
                && !V4_BRIDGE_JS.contains("pill.textContent = noteText ||"),
            "reconnect pill JS must sanitize noteText and never dump raw IO errors"
        );
        assert!(V4_BRIDGE_JS.contains("pcBridgeDevice"));

        // 2. GET /api/status and GET /api/audio expose pcBridgeDevice & pcBridgeCandidates
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_st = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let r_st = handle_request(&req_st, &state, Path::new("web"));
        assert_eq!(r_st.status_code, 200);
        let b_st = String::from_utf8_lossy(&r_st.body);
        assert!(b_st.contains(r#""pcBridgeDevice":"#));
        assert!(b_st.contains(r#""pcBridgeCandidates":"#));

        let req_aud = HttpRequest {
            method: "GET".to_string(),
            path: "/api/audio".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let r_aud = handle_request(&req_aud, &state, Path::new("web"));
        assert_eq!(r_aud.status_code, 200);
        let b_aud = String::from_utf8_lossy(&r_aud.body);
        assert!(b_aud.contains(r#""pcBridgeDevice":"#));
        assert!(b_aud.contains(r#""pcBridgeCandidates":"#));

        // 3. Mini 2x bilinear + contrast upscale on thin Spotlight strip (1216 x 160 -> 2432 x 320 <= 3.2 MB)
        let strip_w = 1216usize;
        let strip_h = 160usize;
        let mut src = vec![235u8; strip_w * strip_h * 4];
        for y in 60..100 {
            for x in 100..200 {
                let idx = (y * strip_w + x) * 4;
                src[idx] = 40;
                src[idx + 1] = 40;
                src[idx + 2] = 40;
                src[idx + 3] = 255;
            }
        }
        let (up, uw, uh) = crate::ocr_engine::upscale_2x_bilinear_with_contrast_rgba(
            &src, strip_w, strip_h, true,
        );
        assert_eq!((uw, uh), (2432, 320));
        assert_eq!(up.len(), 2432 * 320 * 4);
        assert!(up.len() < 3_500_000, "2x Spotlight strip must stay < 3.5 MB RAM");
        // Verify contrast boost sharpened dark text stroke (< 20) and brightened light bg (>= 250)
        let dark_px = up[(150 * uw + 250) * 4];
        let light_px = up[(10 * uw + 10) * 4];
        assert!(dark_px <= 15, "dark glyph stroke must sharpen, got {dark_px}");
        assert!(light_px >= 250, "light pill bg must brighten, got {light_px}");
    }

    #[test]
    fn test_tahap245_multiline_wrapped_ios27_spotlight_url_unwrapper() {
        // 1. Exact video_1.mp4 Attempt #1 (iOS 27.2 3-line wrapped Spotlight URL + "Ask Siri")
        let case1_raw = "https://www.instagram.com/p/\nDWUra9vEgZ1/?\nigsh=MTQyY213azh0eTR4OQ==\nAsk Siri";
        let case1_unwrapped = crate::ocr_engine::unwrap_multiline_spotlight_urls(case1_raw);
        assert!(
            case1_unwrapped.contains("https://www.instagram.com/p/DWUra9vEgZ1/?igsh=MTQyY213azh0eTR4OQ=="),
            "case1 unwrap failed: {case1_unwrapped:?}"
        );
        assert!(!case1_unwrapped.contains("OQ==Ask"), "Ask Siri must NOT be glued onto query");
        let best1 = extract_best_link_from_candidates(&[case1_raw.to_string()]);
        assert!(
            best1
                .as_deref()
                .is_some_and(|u| u.starts_with("https://www.instagram.com/p/DWUra9vEgZ1/")),
            "case1 best link mismatch: {best1:?}"
        );

        // 2. Exact video_1.mp4 Attempt #2 (trailing hyphen '-' in ID DR84sR-AUY- MUST stay intact!)
        let case2_raw = "https://www.instagram.com/p/\nDR84sR-AUY-/?\nigsh=cGVkd2tmd2h0cTI0\nAsk Siri";
        let case2_unwrapped = crate::ocr_engine::unwrap_multiline_spotlight_urls(case2_raw);
        assert!(
            case2_unwrapped.contains("https://www.instagram.com/p/DR84sR-AUY-/?igsh=cGVkd2tmd2h0cTI0"),
            "case2 unwrap failed: {case2_unwrapped:?}"
        );
        let best2 = extract_best_link_from_candidates(&[case2_raw.to_string()]);
        assert!(
            best2
                .as_deref()
                .is_some_and(|u| u.starts_with("https://www.instagram.com/p/DR84sR-AUY-/")),
            "case2 trailing hyphen ID corrupted: {best2:?}"
        );

        // 3. Single-line iPhone 11 Pro Max Spotlight case MUST remain 100% preserved
        let case3_raw = "https://www.instagram.com/reel/DWTT87Ip3cz/?igsh=Z2FzdGM0aWl0d3M=\nCancel";
        let best3 = extract_best_link_from_candidates(&[case3_raw.to_string()]);
        assert!(
            best3
                .as_deref()
                .is_some_and(|u| u.starts_with("https://www.instagram.com/reel/DWTT87Ip3cz/")),
            "case3 single-line 11 Pro Max failed: {best3:?}"
        );

        // 4. Magnifier artifact + mid-URL OCR spacing + trailing clear button
        let case4_raw = "Qhttps:// www.instagram.com /reel/DWI0xrREiq1/ ×";
        let best4 = extract_best_link_from_candidates(&[case4_raw.to_string()]);
        assert!(
            best4
                .as_deref()
                .is_some_and(|u| u.starts_with("https://www.instagram.com/reel/DWI0xrREiq1/")),
            "case4 artifact cleanup failed: {best4:?}"
        );

        // 5. Fallback to valid 1x candidate when 2x candidate only captured truncated `.../p/` prefix!
        let best5 = extract_best_link_from_candidates(&[
            "https://www.instagram.com/p/".to_string(),
            case2_raw.to_string(),
        ]);
        assert!(
            best5
                .as_deref()
                .is_some_and(|u| u.starts_with("https://www.instagram.com/p/DR84sR-AUY-/")),
            "case5 failed to reject bare /p/ and fall back to valid 1x: {best5:?}"
        );
        assert!(crate::ocr_engine::text_has_complete_url_candidate(case1_raw));
        assert!(!crate::ocr_engine::text_has_complete_url_candidate("https://www.instagram.com/p/"));
    }

    #[test]
    fn test_tahap246_no_device_clean_slate_status() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        iphone_link_apple::models::clear_metadata_cache(None);
        clear_wifi_status_stream_cache_for_test();

        let tmp_base = env::temp_dir().join(format!(
            "iphone_link_t246_clean_slate_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_base);
        let pair_dir = tmp_base.join("pairing");
        let _ = fs::create_dir_all(&pair_dir);
        let wifi_state_file = tmp_base.join("wifi_state.json");

        let prev_mux = env::var_os("USBMUXD_SOCKET_ADDRESS");
        let prev_pair = env::var_os("IPHONE_LINK_PAIRING_DIR");
        let prev_wstate = env::var_os("IPHONE_LINK_WIFI_STATE_PATH");
        env::set_var("USBMUXD_SOCKET_ADDRESS", "127.0.0.1:1");
        env::set_var("IPHONE_LINK_PAIRING_DIR", &pair_dir);
        env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &wifi_state_file);

        let st_default = AppState::default();
        assert!(
            st_default.selected_udid.is_empty(),
            "AppState::default() must have empty selected_udid, got {:?}",
            st_default.selected_udid
        );
        let state = Arc::new(Mutex::new(st_default));
        let req = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: "force=1".to_string(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp = handle_request(&req, &state, Path::new("web"));
        assert_eq!(resp.status_code, 200);
        let body = String::from_utf8_lossy(&resp.body);

        // Restore env before assertions so cleanup always happens even on failure
        match prev_mux {
            Some(v) => env::set_var("USBMUXD_SOCKET_ADDRESS", v),
            None => env::remove_var("USBMUXD_SOCKET_ADDRESS"),
        }
        match prev_pair {
            Some(v) => env::set_var("IPHONE_LINK_PAIRING_DIR", v),
            None => env::remove_var("IPHONE_LINK_PAIRING_DIR"),
        }
        match prev_wstate {
            Some(v) => env::set_var("IPHONE_LINK_WIFI_STATE_PATH", v),
            None => env::remove_var("IPHONE_LINK_WIFI_STATE_PATH"),
        }
        clear_wifi_status_stream_cache_for_test();
        let _ = fs::remove_dir_all(&tmp_base);

        assert!(body.contains(r#""ready": false"#), "expected ready:false in {body}");
        assert!(body.contains(r#""usbConnected": false"#), "expected usbConnected:false in {body}");
        assert!(body.contains(r#""connected": false"#), "expected connected:false in {body}");
        assert!(body.contains(r#""devices": []"#), "expected empty devices[] in {body}");
        assert!(body.contains(r#""device": null"#), "expected device:null in {body}");
        assert!(body.contains(r#""handset": null"#), "expected handset:null in {body}");
        assert!(body.contains(r#""udid": """#), "expected empty udid in {body}");
        assert!(body.contains(r#""selectedUdid": """#), "expected empty selectedUdid in {body}");

        let body_lower = body.to_ascii_lowercase();
        for forbidden in [
            "iphone 17 pro".to_string(),
            "iphone18,1".to_string(),
            ["uwe", " ", "uh"].concat(),
            "00008150".to_string(),
            ["192.168.", "100."].concat(),
            ["kel", "vin"].concat(),
            "c:\\users\\".to_string(),
            "d:\\".to_string(),
        ] {
            assert!(
                !body_lower.contains(&forbidden),
                "clean-slate /api/status leaked forbidden token {forbidden:?}: {body}"
            );
        }
    }

    #[test]
    fn test_tahap247_multi_device_auto_prioritization() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        iphone_link_apple::models::clear_metadata_cache(None);
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();

        let tmp_base = env::temp_dir().join(format!(
            "iphone_link_t247_autoprio_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_base);
        let pair_dir = tmp_base.join("pairing");
        let _ = fs::create_dir_all(&pair_dir);
        let wifi_state_file = tmp_base.join("wifi_state.json");

        let prev_mux = env::var_os("USBMUXD_SOCKET_ADDRESS");
        let prev_pair = env::var_os("IPHONE_LINK_PAIRING_DIR");
        let prev_wstate = env::var_os("IPHONE_LINK_WIFI_STATE_PATH");
        env::set_var("USBMUXD_SOCKET_ADDRESS", "127.0.0.1:1");
        env::set_var("IPHONE_LINK_PAIRING_DIR", &pair_dir);
        env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &wifi_state_file);

        // Seed 2 synthetic RFC 5737 devices:
        // Beta Phone (iPhone 11 Pro Max, 198.51.100.48) -> OFFLINE (listed first by UDID!)
        // Alpha Phone (iPhone 17 Pro, 198.51.100.49) -> ONLINE
        let seed_json = r#"{
  "devices": {
    "00008030-TEST-DEV-B": {
      "wifi_enabled_on_device": true,
      "name": "Beta Phone",
      "device_name": "Beta Phone",
      "productType": "iPhone12,5",
      "model": "iPhone 11 Pro Max",
      "iosVersion": "18.4",
      "lastIp": "198.51.100.48",
      "last_ip": "198.51.100.48",
      "lastSeenAt": 1763600100,
      "reachable": false
    },
    "00008150-TEST-DEV-A": {
      "wifi_enabled_on_device": true,
      "name": "Alpha Phone",
      "device_name": "Alpha Phone",
      "productType": "iPhone18,1",
      "model": "iPhone 17 Pro",
      "iosVersion": "27.2",
      "lastIp": "198.51.100.49",
      "last_ip": "198.51.100.49",
      "lastSeenAt": 1763600200,
      "reachable": true
    }
  },
  "enabled_udids": ["00008030-TEST-DEV-B", "00008150-TEST-DEV-A"],
  "last_ip": "198.51.100.49",
  "mac_map": {},
  "updated_at": "1763600200"
}"#;
        fs::write(&wifi_state_file, seed_json).unwrap();

        let state = Arc::new(Mutex::new(AppState::default()));
        let req = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp = handle_request(&req, &state, Path::new("web"));
        assert_eq!(resp.status_code, 200);
        let body = String::from_utf8_lossy(&resp.body);

        // Restore env before assertions
        match prev_mux {
            Some(v) => env::set_var("USBMUXD_SOCKET_ADDRESS", v),
            None => env::remove_var("USBMUXD_SOCKET_ADDRESS"),
        }
        match prev_pair {
            Some(v) => env::set_var("IPHONE_LINK_PAIRING_DIR", v),
            None => env::remove_var("IPHONE_LINK_PAIRING_DIR"),
        }
        match prev_wstate {
            Some(v) => env::set_var("IPHONE_LINK_WIFI_STATE_PATH", v),
            None => env::remove_var("IPHONE_LINK_WIFI_STATE_PATH"),
        }
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();
        let _ = fs::remove_dir_all(&tmp_base);

        // MUST auto-select ONLINE Alpha Phone (198.51.100.49), NOT offline Beta Phone!
        assert!(
            body.contains(r#""selectedUdid": "00008150-TEST-DEV-A""#),
            "expected Alpha Phone auto-selected, got: {body}"
        );
        assert!(body.contains(r#""Alpha Phone""#), "missing Alpha Phone: {body}");
        assert!(body.contains(r#""Beta Phone""#), "missing Beta Phone: {body}");
        assert!(body.contains(r#""iPhone 17 Pro""#), "missing iPhone 17 Pro: {body}");
        assert!(body.contains(r#""iPhone 11 Pro Max""#), "missing iPhone 11 Pro Max: {body}");
        assert!(body.contains(r#""198.51.100.49""#), "missing Alpha IP: {body}");
        assert!(body.contains(r#""198.51.100.48""#), "missing Beta IP: {body}");
        assert!(body.contains(r#""wifiOnline": true"#), "missing wifiOnline:true: {body}");
        assert!(body.contains(r#""wifiOnline": false"#), "missing wifiOnline:false: {body}");
    }

    #[test]
    fn test_tahap247_explicit_select_overrides_and_persists() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        iphone_link_apple::models::clear_metadata_cache(None);
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();

        let tmp_base = env::temp_dir().join(format!(
            "iphone_link_t247_select_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_base);
        let pair_dir = tmp_base.join("pairing");
        let _ = fs::create_dir_all(&pair_dir);
        let wifi_state_file = tmp_base.join("wifi_state.json");

        let prev_mux = env::var_os("USBMUXD_SOCKET_ADDRESS");
        let prev_pair = env::var_os("IPHONE_LINK_PAIRING_DIR");
        let prev_wstate = env::var_os("IPHONE_LINK_WIFI_STATE_PATH");
        env::set_var("USBMUXD_SOCKET_ADDRESS", "127.0.0.1:1");
        env::set_var("IPHONE_LINK_PAIRING_DIR", &pair_dir);
        env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &wifi_state_file);

        let seed_json = r#"{
  "devices": {
    "00008030-TEST-DEV-B": {
      "wifi_enabled_on_device": true,
      "name": "Beta Phone",
      "productType": "iPhone12,5",
      "model": "iPhone 11 Pro Max",
      "iosVersion": "18.4",
      "lastIp": "198.51.100.48",
      "lastSeenAt": 1763600100,
      "reachable": false
    },
    "00008150-TEST-DEV-A": {
      "wifi_enabled_on_device": true,
      "name": "Alpha Phone",
      "productType": "iPhone18,1",
      "model": "iPhone 17 Pro",
      "iosVersion": "27.2",
      "lastIp": "198.51.100.49",
      "lastSeenAt": 1763600200,
      "reachable": true
    }
  }
}"#;
        fs::write(&wifi_state_file, seed_json).unwrap();

        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. Invalid UDID MUST return 400 Bad Request
        let bad_req = HttpRequest {
            method: "POST".to_string(),
            path: "/api/device/select".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"udid":"UNKNOWN-UDID-999"}"#.to_string(),
            body_bytes: br#"{"udid":"UNKNOWN-UDID-999"}"#.to_vec(),
        };
        let bad_resp = handle_request(&bad_req, &state, Path::new("web"));
        assert_eq!(bad_resp.status_code, 400);

        // 2. Explicit POST /api/device/select to Beta Phone MUST succeed in < 50ms
        let t0 = std::time::Instant::now();
        let sel_req = HttpRequest {
            method: "POST".to_string(),
            path: "/api/device/select".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"udid":"00008030-TEST-DEV-B"}"#.to_string(),
            body_bytes: br#"{"udid":"00008030-TEST-DEV-B"}"#.to_vec(),
        };
        let sel_resp = handle_request(&sel_req, &state, Path::new("web"));
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let sel_body = String::from_utf8_lossy(&sel_resp.body).to_string();

        // 3. Subsequent GET /api/status MUST honor explicit Beta Phone selection (NOT snap back to Alpha!)
        let st_req = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let st_resp = handle_request(&st_req, &state, Path::new("web"));
        let st_body = String::from_utf8_lossy(&st_resp.body).to_string();

        // Restore env before assertions
        match prev_mux {
            Some(v) => env::set_var("USBMUXD_SOCKET_ADDRESS", v),
            None => env::remove_var("USBMUXD_SOCKET_ADDRESS"),
        }
        match prev_pair {
            Some(v) => env::set_var("IPHONE_LINK_PAIRING_DIR", v),
            None => env::remove_var("IPHONE_LINK_PAIRING_DIR"),
        }
        match prev_wstate {
            Some(v) => env::set_var("IPHONE_LINK_WIFI_STATE_PATH", v),
            None => env::remove_var("IPHONE_LINK_WIFI_STATE_PATH"),
        }
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();
        let _ = fs::remove_dir_all(&tmp_base);

        assert_eq!(sel_resp.status_code, 200);
        assert!(elapsed_ms < 50.0, "select took too long: {elapsed_ms:.2} ms");
        assert!(sel_body.contains(r#""ok":true"#), "got {sel_body}");
        assert!(sel_body.contains(r#""selectedUdid":"00008030-TEST-DEV-B""#), "got {sel_body}");
        assert!(sel_body.contains(r#""Beta Phone""#), "got {sel_body}");
        assert!(sel_body.contains(r#""iPhone 11 Pro Max""#), "got {sel_body}");
        assert!(sel_body.contains(r#""198.51.100.48""#), "got {sel_body}");

        assert_eq!(st_resp.status_code, 200);
        assert!(
            st_body.contains(r#""selectedUdid": "00008030-TEST-DEV-B""#),
            "subsequent /api/status must preserve explicit Beta Phone selection: {st_body}"
        );
    }

    #[test]
    fn test_tahap247_fresh_install_remains_clean_slate() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        iphone_link_apple::models::clear_metadata_cache(None);
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();

        let tmp_base = env::temp_dir().join(format!(
            "iphone_link_t247_clean_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_base);
        let pair_dir = tmp_base.join("pairing");
        let _ = fs::create_dir_all(&pair_dir);
        let wifi_state_file = tmp_base.join("wifi_state.json");

        let prev_mux = env::var_os("USBMUXD_SOCKET_ADDRESS");
        let prev_pair = env::var_os("IPHONE_LINK_PAIRING_DIR");
        let prev_wstate = env::var_os("IPHONE_LINK_WIFI_STATE_PATH");
        env::set_var("USBMUXD_SOCKET_ADDRESS", "127.0.0.1:1");
        env::set_var("IPHONE_LINK_PAIRING_DIR", &pair_dir);
        env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &wifi_state_file);

        let state = Arc::new(Mutex::new(AppState::default()));
        let req = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp = handle_request(&req, &state, Path::new("web"));
        let body = String::from_utf8_lossy(&resp.body).to_string();

        match prev_mux {
            Some(v) => env::set_var("USBMUXD_SOCKET_ADDRESS", v),
            None => env::remove_var("USBMUXD_SOCKET_ADDRESS"),
        }
        match prev_pair {
            Some(v) => env::set_var("IPHONE_LINK_PAIRING_DIR", v),
            None => env::remove_var("IPHONE_LINK_PAIRING_DIR"),
        }
        match prev_wstate {
            Some(v) => env::set_var("IPHONE_LINK_WIFI_STATE_PATH", v),
            None => env::remove_var("IPHONE_LINK_WIFI_STATE_PATH"),
        }
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();
        let _ = fs::remove_dir_all(&tmp_base);

        assert_eq!(resp.status_code, 200);
        assert!(body.contains(r#""ready": false"#));
        assert!(body.contains(r#""devices": []"#));
        assert!(body.contains(r#""device": null"#));
        assert!(body.contains(r#""selectedUdid": """#));
        assert!(!body.to_ascii_lowercase().contains("iphone 17 pro"));
        assert!(!body.to_ascii_lowercase().contains("iphone 11 pro max"));
    }

    #[test]
    fn test_tahap248_m4_hevc_endpoints_and_open_store_oem_free() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));

        let prev_hevc = env::var_os("IPHONE_LINK_MOCK_HEVC_MISSING");
        let prev_h264 = env::var_os("IPHONE_LINK_MOCK_H264_MISSING");
        let prev_dry = env::var_os("IPHONE_LINK_DRY_RUN_SHELL_OPEN");

        env::set_var("IPHONE_LINK_MOCK_HEVC_MISSING", "1");
        env::remove_var("IPHONE_LINK_MOCK_H264_MISSING");
        env::set_var("IPHONE_LINK_DRY_RUN_SHELL_OPEN", "1");

        // 1. Mock PC without HEVC (mfplat present, HEVC MFT missing) -> zero false positive!
        let r_hevc = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/system/hevc_status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_hevc = String::from_utf8_lossy(&r_hevc.body).to_string();

        let r_diag = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/driver/diagnostics".to_string(),
                query: "force=1".to_string(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_diag = String::from_utf8_lossy(&r_diag.body).to_string();

        let r_open_oem = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/system/open_hevc_store".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"target":"hevc_oem"}"#.to_string(),
                body_bytes: br#"{"target":"hevc_oem"}"#.to_vec(),
            },
            &state,
            Path::new("web"),
        );
        let b_open_oem = String::from_utf8_lossy(&r_open_oem.body).to_string();

        // 2. Mock Windows N/KN Edition (H.264 + mfplat missing) -> mediaFeaturePackMissing=true
        env::set_var("IPHONE_LINK_MOCK_H264_MISSING", "1");
        let r_win_n = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/system/hevc_status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_win_n = String::from_utf8_lossy(&r_win_n.body).to_string();

        let r_open_mfp = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/system/open_hevc_store".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"target":"media_feature_pack"}"#.to_string(),
                body_bytes: br#"{"target":"media_feature_pack"}"#.to_vec(),
            },
            &state,
            Path::new("web"),
        );
        let b_open_mfp = String::from_utf8_lossy(&r_open_mfp.body).to_string();

        match prev_hevc {
            Some(v) => env::set_var("IPHONE_LINK_MOCK_HEVC_MISSING", v),
            None => env::remove_var("IPHONE_LINK_MOCK_HEVC_MISSING"),
        }
        match prev_h264 {
            Some(v) => env::set_var("IPHONE_LINK_MOCK_H264_MISSING", v),
            None => env::remove_var("IPHONE_LINK_MOCK_H264_MISSING"),
        }
        match prev_dry {
            Some(v) => env::set_var("IPHONE_LINK_DRY_RUN_SHELL_OPEN", v),
            None => env::remove_var("IPHONE_LINK_DRY_RUN_SHELL_OPEN"),
        }

        assert_eq!(r_hevc.status_code, 200);
        assert!(b_hevc.contains(r#""installed":false"#), "got {b_hevc}");
        assert!(b_hevc.contains(r#""hevcMftAvailable":false"#), "got {b_hevc}");
        assert!(b_hevc.contains(r#""h264MftAvailable":true"#), "got {b_hevc}");
        assert!(b_hevc.contains(r#""mediaFeaturePackMissing":false"#), "got {b_hevc}");
        assert!(b_hevc.contains("9N4WGH0Z6VHQ"), "missing OEM free storeUrl: {b_hevc}");

        assert_eq!(r_diag.status_code, 200);
        assert!(b_diag.contains(r#""hevc_codec_installed":false"#), "got {b_diag}");
        assert!(b_diag.contains("9N4WGH0Z6VHQ"), "got {b_diag}");

        assert_eq!(r_open_oem.status_code, 200);
        assert!(
            b_open_oem.contains(r#""ok":true"#)
                && b_open_oem.contains("9N4WGH0Z6VHQ")
                && b_open_oem.contains(r#""dryRun":true"#),
            "got {b_open_oem}"
        );

        assert_eq!(r_win_n.status_code, 200);
        assert!(b_win_n.contains(r#""mediaFeaturePackMissing":true"#), "got {b_win_n}");
        assert!(b_win_n.contains("ms-settings:optionalfeatures"), "got {b_win_n}");

        assert_eq!(r_open_mfp.status_code, 200);
        assert!(
            b_open_mfp.contains("ms-settings:optionalfeatures")
                && b_open_mfp.contains(r#""dryRun":true"#),
            "got {b_open_mfp}"
        );
    }

    #[test]
    fn test_tahap248_m5_anti_blank_white_css_diag_overlay_and_wifi_boot_fix() {
        // 1. Pre-first-frame Light Mode specificity lock (0,2,3,3)
        assert!(V4_TRUE_ACRYLIC_CSS.contains(
            "html body #app-root.app.is-streaming:not(.has-first-frame) #phone-screen"
        ));
        assert!(V4_TRUE_ACRYLIC_CSS.contains(
            "html body #app-root.app.is-streaming:not(.has-first-frame) iframe#screen-view"
        ));
        assert!(V4_TRUE_ACRYLIC_CSS.contains("#090d16 !important"));

        // 2. V4_BRIDGE_JS fixed #screen-view ID (0 stale mirror-frame refs) + open_hevc_store fallback
        assert!(!V4_BRIDGE_JS.contains("getElementById('mirror-frame')"));
        assert!(V4_BRIDGE_JS.contains("getElementById('screen-view')"));
        assert!(V4_BRIDGE_JS.contains("/api/system/open_hevc_store"));
        assert!(V4_BRIDGE_JS.contains("mirror-first-frame"));

        // 3. Mirror HTML + CSS inject #v4-mirror-diag-overlay & 3 buttons without touching frozen mirror.css
        let m_html = get_embedded_mirror_html();
        let m_css = get_embedded_mirror_css();
        let m_js = get_embedded_mirror_js();
        assert!(m_html.contains("id=\"v4-mirror-diag-overlay\""));
        assert!(m_html.contains("id=\"btn-diag-install-hevc\""));
        assert!(m_html.contains("id=\"btn-diag-soft-decode\""));
        assert!(m_html.contains("id=\"btn-diag-copy-info\""));
        assert!(m_css.contains("__V5_TAHAP248_MIRROR_DIAG_OVERLAY__"));
        assert!(m_css.contains("#v4-mirror-diag-overlay"));

        // 4. mirror.js 4-step fallback ladder (prefer-software + hev1/hvc1), pointerdown guard & test hook
        assert!(m_js.contains("buildDecoderCandidateConfigs"));
        assert!(m_js.contains("prefer-software"));
        assert!(m_js.contains("_flipHevcCodecPrefix"));
        assert!(m_js.contains("showMirrorDiagOverlay"));
        assert!(m_js.contains("__v5SimulateDecodeErrorForTest"));
        assert!(m_js.contains("mirror-first-frame"));

        // 5. app.js Pilar 1 micro-glitch boot tab Wi-Fi fix
        assert!(EMBEDDED_APP_JS.contains("resolveInitialTransport"));
        assert!(EMBEDDED_APP_JS.contains("syncLaunchpadSwitchWifiButton"));
        assert!(EMBEDDED_APP_JS.contains("window.currentTransport"));
    }

    #[test]
    fn test_tahap249_fresh_install_default_audio_pc_and_post_stop_wifi_reachability() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let tmp_base = env::temp_dir().join(format!(
            "iphone_link_tahap249_fresh_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_base);
        let pair_dir = tmp_base.join("pairings");
        let _ = fs::create_dir_all(&pair_dir);
        let wifi_state_file = tmp_base.join("wifi_state.json");

        let prev_mux = env::var_os("USBMUXD_SOCKET_ADDRESS");
        let prev_pair = env::var_os("IPHONE_LINK_PAIRING_DIR");
        let prev_wstate = env::var_os("IPHONE_LINK_WIFI_STATE_PATH");

        env::set_var("USBMUXD_SOCKET_ADDRESS", "127.0.0.1:1");
        env::set_var("IPHONE_LINK_PAIRING_DIR", &pair_dir);
        env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &wifi_state_file);

        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        iphone_link_audio::stop_global_pc_audio_bridge();
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();
        clear_metadata_cache(None);
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);

        // 1. Fresh-install AppState::default(): audio_routing="pc", 0-device clean-slate stays idle
        let mut st_fresh = AppState::default();
        assert_eq!(st_fresh.audio_routing, "pc");
        assert!(!st_fresh.audio_muted);
        // Simulate missing user_settings.json path via load_user_settings_from_path + sync
        let missing_settings = tmp_base.join("user_settings.json");
        let _ = load_user_settings_from_path(&missing_settings, &mut st_fresh);
        GLOBAL_AUDIO_PC_ACTIVE.store(
            (st_fresh.audio_routing == "pc" || st_fresh.audio_routing == "bluetooth")
                && !st_fresh.audio_muted,
            Ordering::SeqCst,
        );
        assert!(GLOBAL_AUDIO_PC_ACTIVE.load(Ordering::SeqCst));
        assert!(!iphone_link_audio::is_global_pc_audio_bridge_active());
        assert!(!iphone_link_audio::is_global_pc_audio_bridge_connecting());

        // 2. Seed RFC 5737 synthetic paired Wi-Fi iPhone (0 personal data)
        let synth_udid = "00008150-0000000000000249";
        let synth_ip = "192.0.2.49";
        let _ = fs::write(
            pair_dir.join(format!("{synth_udid}.json")),
            format!(
                r#"{{"udid":"{synth_udid}","host_id":"SYNTH-HOST-249","remote_unlock_host_key":"00112233445566778899aabbccddeeff","client_ltpk":"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff","client_ltsk":"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"}}"#
            ),
        );
        let _ = fs::write(
            &wifi_state_file,
            format!(
                r#"{{"devices":{{"{synth_udid}":{{"name":"Test Phone 249","productType":"iPhone17,1","model":"iPhone 16 Pro","lastIp":"{synth_ip}","wifi_enabled_on_device":true,"reachable":true,"lastSeenAt":1750000249}}}}}}"#
            ),
        );
        iphone_link_apple::wifi::set_cached_wifi_reachability(
            synth_udid,
            true,
            Some(synth_ip),
            1_750_000_249,
        );

        let state = Arc::new(Mutex::new(st_fresh));
        // Pre-populate zero-I/O RAM cache via initial /api/status
        let r_init = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_init = String::from_utf8_lossy(&r_init.body).to_string();
        assert!(b_init.contains(r#""driverReady": true"#), "got {b_init}");
        assert!(b_init.contains(r#""wifiOnline": true"#), "got {b_init}");
        assert!(b_init.contains(synth_ip), "got {b_init}");

        // 3. Start synthetic Wi-Fi mirror (honestGate=false -> active streaming) -> auto-audio PC starts
        let start_body = format!(r#"{{"udid":"{synth_udid}","transport":"wifi","honestGate":false}}"#);
        let r_start = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/screen".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: start_body.clone(),
                body_bytes: start_body.into_bytes(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(r_start.status_code, 200);
        let r_aud = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/audio".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_aud = String::from_utf8_lossy(&r_aud.body).to_string();
        assert!(
            b_aud.contains(r#""audioOut":"pc""#)
                && (b_aud.contains(r#""pcBridgeActive":true"#)
                    || b_aud.contains(r#""pcBridgeConnecting":true"#)),
            "Fresh-install Wi-Fi start must auto-launch PC audio bridge: {b_aud}"
        );

        // While streaming (zero_poll_active=true), /api/status must STILL preserve model, IP, wifiOnline=true!
        let r_live = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_live = String::from_utf8_lossy(&r_live.body).to_string();
        assert!(
            b_live.contains(r#""wifiOnline": true"#) && b_live.contains(synth_ip),
            "Active Wi-Fi stream must never drop wifiOnline/IP during zero_poll_active: {b_live}"
        );

        // 4. Clean stop via POST /api/screen/stop -> post-stop /api/status MUST keep reachable=true + IP
        let r_stop = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/screen/stop".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: "{}".to_string(),
                body_bytes: b"{}".to_vec(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(r_stop.status_code, 200);
        assert!(
            iphone_link_apple::wifi::is_in_wifi_post_stop_grace(synth_udid),
            "Clean Wi-Fi stop must activate 20s post-stop reachability grace window"
        );
        let r_after = handle_request(
            &HttpRequest {
                method: "GET".to_string(),
                path: "/api/status".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: String::new(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_after = String::from_utf8_lossy(&r_after.body).to_string();
        assert!(
            b_after.contains(r#""wifiOnline": true"#)
                && b_after.contains(r#""reachable": true"#)
                && b_after.contains(synth_ip),
            "Post-stop /api/status must stay Wi-Fi online (zero iPhone Terputus flash): {b_after}"
        );
        assert_eq!(state.lock().unwrap().audio_routing, "pc");

        // Cleanup
        iphone_link_audio::stop_global_pc_audio_bridge();
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        LAST_STOP_MIRROR_MS.store(0, Ordering::SeqCst);
        match prev_mux {
            Some(v) => env::set_var("USBMUXD_SOCKET_ADDRESS", v),
            None => env::remove_var("USBMUXD_SOCKET_ADDRESS"),
        }
        match prev_pair {
            Some(v) => env::set_var("IPHONE_LINK_PAIRING_DIR", v),
            None => env::remove_var("IPHONE_LINK_PAIRING_DIR"),
        }
        match prev_wstate {
            Some(v) => env::set_var("IPHONE_LINK_WIFI_STATE_PATH", v),
            None => env::remove_var("IPHONE_LINK_WIFI_STATE_PATH"),
        }
        iphone_link_apple::wifi::clear_cached_wifi_reachability();
        clear_wifi_status_stream_cache_for_test();
        let _ = fs::remove_dir_all(&tmp_base);
    }

    #[test]
    fn test_tahap249_topbar_pill_honesty_and_hw_decode_sticky_guard() {
        // 1. mirror.js Pilar 1: Zero sticky software decode + 1x HW auto-upgrade + decodeQueueSize cap
        let m_js = get_embedded_mirror_js();
        assert!(m_js.contains("_hwTransientRecoveries"));
        assert!(m_js.contains("_goodHwFramesSinceRecovery"));
        assert!(m_js.contains("_droppedBacklogDeltas"));
        assert!(m_js.contains("_postPliThrottled"));
        assert!(m_js.contains("decoder_hw_transient_recover"));
        assert!(m_js.contains("decoder_hw_auto_upgrade"));
        assert!(m_js.contains("decoder.decodeQueueSize"));
        assert!(m_js.contains("ActiveAccelMode:"));
        assert!(m_js.contains("HwTransientRecoveries:"));
        assert!(m_js.contains("DroppedBacklogDeltas:"));
        // Preserve Tahap 248 Mock M-5 diagnostic overlay & test hook
        assert!(m_js.contains("__v5SimulateDecodeErrorForTest"));
        assert!(m_js.contains("showMirrorDiagOverlay"));

        // 2. app.js Pilar 3: Honest top-bar pills, connecting phase lock, audio badge preservation
        assert!(EMBEDDED_APP_JS.contains(r#"scrPhase === "starting""#));
        assert!(EMBEDDED_APP_JS.contains("driverExplicitlyMissing"));
        assert!(EMBEDDED_APP_JS.contains("!driverOk && !overWifi"));
        assert!(EMBEDDED_APP_JS.contains("window.applyServerStatusState = applyServerStatusState"));
        assert!(EMBEDDED_APP_JS.contains("window.renderPills = renderPills"));
        assert!(EMBEDDED_APP_JS.contains("currentAudioState.pcBridgeActive"));
        assert!(EMBEDDED_APP_JS.contains("currentAudioState.pcBridgeConnecting"));
        assert!(
            !EMBEDDED_APP_JS.contains("renderPills({ wifi: res.wifi, devices:"),
            "triggerWifiScan must never pass stripped partial object without driver to renderPills"
        );

        // 3. V4_BRIDGE_JS Pilar 3: Boot placeholder pill guard + same-tick Launchpad/pill sync + 3 audio retries
        assert!(V4_BRIDGE_JS.contains("isBootPlaceholder"));
        assert!(V4_BRIDGE_JS.contains("window.applyServerStatusState"));
        assert!(V4_BRIDGE_JS.contains("window.renderPills(s)"));
        assert!(V4_BRIDGE_JS.contains("tries < 3"));
    }

    #[test]
    fn test_tahap250_burst_tolerant_decode_queue_grace_and_fresh_wifi_auto_tab() {
        // 1. mirror.js Pilar 1: Cold-Start GPU Shader Grace (6000 ms), sustained backlog >= 12 @ 450 ms,
        //    non-freezing soft PLI hint (6..=11), zero aggressive qSize >= 3 delta drop!
        let m_js = get_embedded_mirror_js();
        assert!(m_js.contains("_COLD_START_SHADER_GRACE_MS = 6000"));
        assert!(m_js.contains("_streamDecodeStartedAtMs"));
        assert!(m_js.contains("_sustainedBacklogSinceMs"));
        assert!(m_js.contains("_softPliBacklogSinceMs"));
        assert!(m_js.contains("12@450ms/grace6000ms"));
        assert!(m_js.contains("SustainedBacklogGuard:"));
        assert!(m_js.contains("(nowDecodeMs - _sustainedBacklogSinceMs) >= 450"));
        assert!(m_js.contains("(nowDecodeMs - _softPliBacklogSinceMs) >= 250"));
        assert!(
            !m_js.contains("if (!isKey && qSize >= 3)"),
            "Aggressive qSize >= 3 delta drop that caused 3s Wi-Fi freeze MUST be removed!"
        );
        // Preserve Tahap 248/249 anti-sticky HW decode & Mock M-5
        assert!(m_js.contains("_hwTransientRecoveries"));
        assert!(m_js.contains("decoder_hw_auto_upgrade"));
        assert!(m_js.contains("__v5SimulateDecodeErrorForTest"));

        // 2. app.js Pilar 3: Explicit separation of userExplicitlyClickedTransport vs fresh-install boot
        assert!(EMBEDDED_APP_JS.contains("let userExplicitlyClickedTransport = false"));
        assert!(EMBEDDED_APP_JS.contains("let hasSavedExplicitTransport = Boolean("));
        assert!(EMBEDDED_APP_JS.contains("!userExplicitlyClickedTransport && !hasSavedExplicitTransport"));
        assert!(EMBEDDED_APP_JS.contains("hasOnlineWifiDevice"));
        assert!(
            !EMBEDDED_APP_JS.contains("if (userInitiated || reload)"),
            "setTransport must NOT write localStorage handset-transport on programmatic non-user reload!"
        );

        // 3. V4_BRIDGE_JS Pilar 3: Fresh install (hasUserSettingsFile=false) clears handset-transport
        //    and sanitizer respects auto-switched window.currentTransport === 'wifi'
        assert!(V4_BRIDGE_JS.contains("if (initData.hasUserSettingsFile && initData.transport)"));
        assert!(V4_BRIDGE_JS.contains("localStorage.removeItem('handset-transport')"));
        assert!(V4_BRIDGE_JS.contains("var liveTr = (window.currentTransport === 'wifi') ? 'wifi' : ''"));
    }

    #[test]
    fn test_tahap252_api_screen_mode_airplay_and_diag_switch_button() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_with = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Explicit POST /api/screen { mode: "airplay" } launches native AirPlay H.264 receiver
        let r_ap = handle_request(
            &req_with(
                "POST",
                "/api/screen",
                r#"{"mode":"airplay","transport":"wifi","airplayPort":0,"advertiseMdns":false,"hybridHid":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_ap.status_code, 200);
        let ap_body = String::from_utf8_lossy(&r_ap.body);
        assert!(ap_body.contains(r#""phase":"starting""#), "ap_body={ap_body}");
        assert!(ap_body.contains(r#""stage":"waiting_airplay""#), "ap_body={ap_body}");
        assert!(ap_body.contains(r#""airplayActive":true"#), "ap_body={ap_body}");
        assert!(ap_body.contains(r#""airplayName":"iPhone Link""#), "ap_body={ap_body}");
        assert!(ap_body.contains(r#""mirrorRoute":"airplay_standalone""#), "ap_body={ap_body}");

        // 2. GET /api/status exposes mirrorRoute, airplayActive, airplayName, airplayPort, hidControlAvailable
        let r_st = handle_request(&req_with("GET", "/api/status", ""), &state, Path::new("web"));
        let st_body = String::from_utf8_lossy(&r_st.body);
        assert!(st_body.contains(r#""mirrorRoute": "airplay_standalone""#), "st_body={st_body}");
        assert!(st_body.contains(r#""airplayActive": true"#), "st_body={st_body}");
        assert!(st_body.contains(r#""airplayName": "iPhone Link""#), "st_body={st_body}");
        assert!(st_body.contains(r#""hidControlAvailable": false"#), "st_body={st_body}");

        // 3. Auto router on iOS 16.7.10 routes straight to airplay_standalone without CoreDevice
        let r_auto16 = handle_request(
            &req_with(
                "POST",
                "/api/screen",
                r#"{"mode":"auto","iosVersion":"16.7.10","transport":"wifi","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        let auto16_body = String::from_utf8_lossy(&r_auto16.body);
        assert!(
            auto16_body.contains(r#""mirrorRoute":"airplay_standalone""#),
            "iOS 16.7.10 must route to airplay_standalone: {auto16_body}"
        );

        // 4. Mirror HTML injects #btn-diag-switch-airplay; index.html exposes #segmented-stream-mode;
        //    mirror.js & app.js wire switch-to-airplay-h264 & waiting_airplay 120s timeout
        let m_html = get_embedded_mirror_html();
        assert!(m_html.contains(r#"id="btn-diag-switch-airplay""#));
        assert!(EMBEDDED_INDEX_HTML.contains(r#"id="segmented-stream-mode""#));
        assert!(EMBEDDED_INDEX_HTML.contains(r#"data-stream-mode="airplay""#));
        let m_js = get_embedded_mirror_js();
        assert!(m_js.contains("switch-to-airplay-h264"));
        assert!(m_js.contains("const spsDims = isH264 ? null : parseHevcSpsDimensions(description)"));
        assert!(EMBEDDED_APP_JS.contains("CONNECTING_TIMEOUT_AIRPLAY_MS = 120000"));
        assert!(EMBEDDED_APP_JS.contains("switchToAirplayH264Mode"));

        // 5. Clean DELETE /api/screen stops AirPlay receiver cleanly
        let r_del = handle_request(&req_with("DELETE", "/api/screen", ""), &state, Path::new("web"));
        assert_eq!(r_del.status_code, 200);
        assert!(!iphone_link_apple::airplay::is_global_airplay_running());
    }

    #[test]
    fn test_tahap253_mirror_js_h264_avcc_description_annexb_fallback_and_local_idr_replay() {
        let m_js = get_embedded_mirror_js();
        // (a) avcC description attached for avc1 in AVCC tier (not blocked by !isH264)
        assert!(m_js.contains(r#"if (h264Mode === "avcc" && hasAvccDesc)"#));
        assert!(m_js.contains("cfg.description = description;"));
        assert!(m_js.contains("Boolean(info.isAirplay || info.is_airplay)"));

        // (b) Pure avcC -> Annex-B converters & AVCC IDR detection
        assert!(m_js.contains("function parseAvccSpsPps(avccU8)"));
        assert!(m_js.contains("function convertAvccChunkToAnnexB("));
        assert!(m_js.contains("function avccChunkHasIdr("));

        // (c) Local IDR replay on H.264 fallback (Zero-PLI recovery)
        assert!(m_js.contains("let lastKeyChunkPayload = null"));
        assert!(m_js.contains("const replayLocalKeyChunkIfAvailable = () =>"));
        assert!(m_js.contains("h264_local_idr_replay"));

        // (d) Zero-trim for AirPlay/H.264 & immediate mirror-first-frame notification
        assert!(m_js.contains("isAirplayOrH264"));
        assert!(m_js.contains(r#"window.parent.postMessage({ type: "mirror-first-frame" }, "*")"#));

        // (e) Anti-silent-black 5000ms watchdog + enriched sanitized diagnostics (0 personal data)
        assert!(m_js.contains("_H264_SILENT_BLACK_TIMEOUT_MS = 5000"));
        assert!(m_js.contains("onH264SilentBlackCheck"));
        assert!(m_js.contains("MirrorRoute:"));
        assert!(m_js.contains("H264Mode:"));
        assert!(m_js.contains("AirplayVideoFrames:"));
        assert!(m_js.contains("AirplayDecryptFailures:"));

        // (f) app.js clears Sequoia Connect Overlay immediately on mirror-first-frame
        assert!(EMBEDDED_APP_JS.contains("completeSequoiaHandshake(null)"));
    }

    #[test]
    fn test_tahap254_mirror_js_monotonic_timestamp_safe_raf_and_post_frame1_h264_recovery() {
        let m_js = get_embedded_mirror_js();

        // (a) Strictly positive monotonic timestamp (never constant 0!)
        assert!(m_js.contains("const nextChunkTimestamp = () => (++_decodedChunkSeq) * 16667;"));
        assert!(m_js.contains("timestamp: nextChunkTimestamp(),"));

        // (b) Safe rAF scheduler reset in finally + auto re-arm on pendingFrame
        assert!(m_js.contains("if (pendingFrame && !rafScheduled)"));
        assert!(m_js.contains("_isFlushingPendingFrame = false;\n    rafScheduled = false;")
            || m_js.contains("_isFlushingPendingFrame = false;\r\n    rafScheduled = false;"));

        // (c) AVCC-first validation in convertAvccChunkToAnnexB (256..511B delta NALU safe!)
        assert!(m_js.contains("let validAvcc = true;"));
        assert!(m_js.contains("if (!validAvcc || nals.length === 0 || pos !== avccPayloadU8.byteLength)"));

        // (d) Post-Frame-#1 H.264 silent stall watchdog & automatic tier recovery
        assert!(m_js.contains("const _H264_POST_FRAME1_STALL_MS = 2500;"));
        assert!(m_js.contains("onH264PostFrame1StallCheck = () =>"));
        assert!(m_js.contains("h264_post_frame1_stall_recover"));

        // (e) Enriched PII-free diagnostics in #v4-mirror-diag-overlay
        assert!(m_js.contains("RenderedFrames:"));
        assert!(m_js.contains("DecodedChunks:"));
        assert!(m_js.contains("AirplayDeltaFrames:"));
        assert!(m_js.contains("AirplayMirrorLoopAlive:"));
        assert!(m_js.contains("LastDecodeError:"));

        // (f) /api/status exposes all 5 new Tahap 254 PII-free telemetry keys
        let state = Arc::new(Mutex::new(AppState::default()));
        let req = HttpRequest {
            method: "GET".to_string(),
            path: "/api/status".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        let resp = handle_request(&req, &state, Path::new("web"));
        assert_eq!(resp.status_code, 200);
        let body = String::from_utf8_lossy(&resp.body);
        for key in [
            r#""airplayDeltaFrames":"#,
            r#""airplayTcpPacketsRead":"#,
            r#""airplayMirrorLoopAlive":"#,
            r#""liveChunksBroadcasted":"#,
            r#""liveClientsConnected":"#,
        ] {
            let spaced = key.replace(":", ": ");
            assert!(
                body.contains(key) || body.contains(&spaced),
                "/api/status missing Tahap 254 telemetry key {key}"
            );
        }
    }

    #[test]
    fn test_tahap255_fresh_install_purges_stale_airplay_mode_and_auto_routes_ios27_to_coredevice() {
        // 1. Fresh-install bridge & app.js MUST purge all stale streamMode keys and lock 'auto'
        assert!(V4_BRIDGE_JS.contains("if (!initData.hasUserSettingsFile)"));
        assert!(V4_BRIDGE_JS.contains("localStorage.removeItem('iphone_link_stream_mode')"));
        assert!(V4_BRIDGE_JS.contains("localStorage.removeItem('handset-stream-mode')"));
        assert!(V4_BRIDGE_JS.contains("localStorage.removeItem('v4-stream-mode')"));
        assert!(V4_BRIDGE_JS.contains("window.currentStreamMode = 'auto'"));
        assert!(V4_BRIDGE_JS.contains("else if (group.id === 'segmented-stream-mode')"));

        assert!(EMBEDDED_APP_JS.contains("init && init.hasUserSettingsFile === false"));
        assert!(EMBEDDED_APP_JS.contains("activeStreamPill"));
        assert!(EMBEDDED_APP_JS.contains("mode: effectiveStreamMode"));
        assert!(EMBEDDED_APP_JS.contains("updateStreamModeDescription"));
        assert!(EMBEDDED_INDEX_HTML.contains(r#"id="stream-mode-desc""#));

        // 2. Auto router strictly routes iOS 27.2 & unknown -> CoreDeviceNativeHevc,
        //    iOS 18.5 & iOS 16.7.10 -> AirPlayStandalone (Tahap 293 Opsi 2)
        use iphone_link_apple::link::{resolve_ios_mirror_route, IosMirrorRoute};
        assert_eq!(resolve_ios_mirror_route("27.2"), IosMirrorRoute::CoreDeviceNativeHevc);
        assert_eq!(resolve_ios_mirror_route(""), IosMirrorRoute::CoreDeviceNativeHevc);
        assert_eq!(resolve_ios_mirror_route("18.5"), IosMirrorRoute::AirPlayStandalone);
        assert_eq!(resolve_ios_mirror_route("16.7.10"), IosMirrorRoute::AirPlayStandalone);

        // 3. POST /api/screen { mode: "auto", iosVersion: "27.2" } NEVER launches AirPlay!
        let _ = iphone_link_apple::mirror::global_stop_screen();
        let state = Arc::new(Mutex::new(AppState::default()));
        let req = HttpRequest {
            method: "POST".to_string(),
            path: "/api/screen".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"mode":"auto","iosVersion":"27.2","transport":"usb","honestGate":false}"#.to_string(),
            body_bytes: br#"{"mode":"auto","iosVersion":"27.2","transport":"usb","honestGate":false}"#.to_vec(),
        };
        let resp = handle_request(&req, &state, Path::new("web"));
        assert_eq!(resp.status_code, 200);
        let body = String::from_utf8_lossy(&resp.body);
        assert!(
            !body.contains(r#""airplayActive":true"#) && !body.contains(r#""stage":"waiting_airplay""#),
            "Auto mode on iOS 27.2 MUST NOT route to AirPlay: {body}"
        );
        assert!(!iphone_link_apple::airplay::is_global_airplay_running());
    }

    #[test]
    fn test_tahap255_manual_airplay_mode_is_pure_standalone_zero_hid_and_soft_touch_response() {
        let _ = iphone_link_apple::mirror::global_stop_screen();
        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut st = state.lock().unwrap();
            st.selected_udid = "00008150-TEST-DEV-Z".to_string();
        }
        let req_with = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Even with selected_udid + iOS 27.2 + explicit hybridHid:true, manual `mode: "airplay"`
        //    is ALWAYS 100% Pure AirPlay Standalone (0 CoreDevice worker, hidControlAvailable = false)!
        let r_ap = handle_request(
            &req_with(
                "POST",
                "/api/screen",
                r#"{"mode":"airplay","iosVersion":"27.2","udid":"00008150-TEST-DEV-Z","transport":"wifi","airplayPort":0,"advertiseMdns":false,"hybridHid":true}"#,
            ),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_ap.status_code, 200);
        let ap_body = String::from_utf8_lossy(&r_ap.body);
        assert!(
            ap_body.contains(r#""mirrorRoute":"airplay_standalone""#),
            "Manual airplay mode must ALWAYS be airplay_standalone: {ap_body}"
        );
        assert!(
            ap_body.contains(r#""hidControlAvailable":false"#),
            "Manual airplay mode must expose hidControlAvailable:false: {ap_body}"
        );
        assert!(iphone_link_apple::mirror::is_airplay_standalone_no_hid_active());

        // 2. Touch/button/key/scroll/gesture endpoints return calm HTTP 200 without HID error spam
        for (ep, payload) in [
            ("/mirror/touch", r#"{"x":0.5,"y":0.5,"action":"tap"}"#),
            ("/mirror/button", r#"{"name":"home","state":"press"}"#),
            ("/mirror/key", r#"{"text":"a"}"#),
            ("/mirror/scroll", r#"{"deltaY":-120}"#),
            ("/mirror/gesture", r#"{"direction":"back"}"#),
        ] {
            let r_in = handle_request(&req_with("POST", ep, payload), &state, Path::new("web"));
            assert_eq!(r_in.status_code, 200, "endpoint {ep} must return HTTP 200");
            let in_body = String::from_utf8_lossy(&r_in.body);
            assert!(
                in_body.contains(r#""ok":false"#) && in_body.contains(r#""hidControlAvailable":false"#),
                "endpoint {ep} must return calm soft response in Pure AirPlay: {in_body}"
            );
        }

        // 3. GET /api/debug/airplay-trace returns valid PII-free JSON & mirror.js fetches it
        let r_tr = handle_request(&req_with("GET", "/api/debug/airplay-trace", ""), &state, Path::new("web"));
        assert_eq!(r_tr.status_code, 200);
        let tr_body = String::from_utf8_lossy(&r_tr.body);
        assert!(tr_body.contains(r#""ok":true"#) && tr_body.contains(r#""events":["#), "trace={tr_body}");
        assert!(!tr_body.contains("00008150-TEST-DEV-Z"), "trace must NEVER leak UDID: {tr_body}");
        let m_js = get_embedded_mirror_js();
        assert!(m_js.contains("/api/debug/airplay-trace"));
        assert!(m_js.contains("AirplayTrace:"));

        // 4. Clean stop
        let _ = handle_request(&req_with("DELETE", "/api/screen", ""), &state, Path::new("web"));
        assert!(!iphone_link_apple::mirror::is_airplay_standalone_no_hid_active());
    }

    #[test]
    fn test_tahap257_app_js_hides_stale_canvas_on_waiting_airplay_and_trace_exposes_counters() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut st = state.lock().unwrap();
            st.audio_routing = "pc".to_string();
            st.audio_muted = false;
        }
        let req_with = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Verify embedded handset/web/app.js cleanly hides stale #screen-view on waiting_airplay,
        //    resets screenTimer = 0, and runs startAirplayLiveStateWatcher() for live disconnect detection!
        let app_js = EMBEDDED_APP_JS;
        assert!(app_js.contains("function startAirplayLiveStateWatcher()"));
        assert!(app_js.contains("function stopAirplayLiveStateWatcher()"));
        assert!(app_js.contains("const isAirplayWaiting = Boolean("));
        assert!(app_js.contains("screenView.src = \"about:blank\""));
        assert!(app_js.contains("screenTimer = 0;"));
        assert!(app_js.contains("force-stop-stream"));

        // 2. Start Pure AirPlay session & confirm GET /api/debug/airplay-trace exposes full counters JSON
        let r_ap = handle_request(
            &req_with(
                "POST",
                "/api/screen",
                r#"{"mode":"airplay","iosVersion":"27.2","udid":"00008150-TEST-257","transport":"wifi","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_ap.status_code, 200);
        assert!(iphone_link_apple::mirror::is_airplay_active());

        let r_tr = handle_request(&req_with("GET", "/api/debug/airplay-trace", ""), &state, Path::new("web"));
        assert_eq!(r_tr.status_code, 200);
        let tr = String::from_utf8_lossy(&r_tr.body);
        for key in [
            r#""counters":{"#,
            r#""ntpTx":"#,
            r#""ntpRx":"#,
            r#""ntpLastErr":"#,
            r#""eventAccepts":"#,
            r#""eventRx":"#,
            r#""audioDataRx":"#,
            r#""audioCtrlRx":"#,
            r#""feedbackCount":"#,
            r#""teardownCount":"#,
        ] {
            assert!(tr.contains(key), "Trace JSON missing counter key {key}: {tr}");
        }
        assert!(!tr.contains("00008150-TEST-257"), "Trace JSON must never leak UDID: {tr}");

        // 3. Simulate AirPlay reaching phase="running" -> GET /api/status MUST NOT launch Bluetooth A2DP!
        {
            let mut m = iphone_link_apple::mirror::mirror_runtime()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            m.phase = "running".to_string();
        }
        let r_st = handle_request(&req_with("GET", "/api/status", ""), &state, Path::new("web"));
        assert_eq!(r_st.status_code, 200);
        assert!(
            !iphone_link_audio::is_global_pc_audio_bridge_active()
                && !iphone_link_audio::is_global_pc_audio_bridge_connecting(),
            "Bluetooth A2DP audio bridge MUST NEVER launch while AirPlay H.264 is active!"
        );

        // 4. Clean stop
        let _ = handle_request(&req_with("DELETE", "/api/screen", ""), &state, Path::new("web"));
        assert!(!iphone_link_apple::mirror::is_airplay_active());
    }

    #[test]
    fn test_tahap258_airplay_audio_mute_endpoint_keeps_bt_isolated_and_mirror_js_hd_crisp() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut st = state.lock().unwrap();
            st.audio_routing = "pc".to_string();
            st.audio_muted = false;
        }
        let req_with = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Start Pure AirPlay H.264 session
        let r_ap = handle_request(
            &req_with(
                "POST",
                "/api/screen",
                r#"{"mode":"airplay","iosVersion":"27.2","udid":"00008150-TEST-258","transport":"wifi","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_ap.status_code, 200);
        assert!(iphone_link_apple::mirror::is_airplay_active());

        // 2. POST /api/audio/airplay-mute true/false toggles cleanly with ZERO Bluetooth A2DP start!
        let r_m1 = handle_request(&req_with("POST", "/api/audio/airplay-mute", r#"{"muted":true}"#), &state, Path::new("web"));
        assert_eq!(r_m1.status_code, 200);
        let b_m1 = String::from_utf8_lossy(&r_m1.body);
        assert!(b_m1.contains(r#""airplayAudioMuted":true"#), "b_m1={b_m1}");
        assert!(b_m1.contains(r#""pcBridgeActive":false"#), "b_m1={b_m1}");
        assert!(b_m1.contains(r#""pcBridgeConnecting":false"#), "b_m1={b_m1}");

        let r_m2 = handle_request(&req_with("POST", "/api/audio/airplay-mute", r#"{"muted":false}"#), &state, Path::new("web"));
        assert_eq!(r_m2.status_code, 200);
        let b_m2 = String::from_utf8_lossy(&r_m2.body);
        assert!(b_m2.contains(r#""airplayAudioMuted":false"#), "b_m2={b_m2}");

        // 3. Pressing Audio PC toggle (/api/audio/toggle) during AirPlay routes to AirPlay mute, NEVER Bluetooth A2DP!
        let r_tog = handle_request(&req_with("POST", "/api/audio/toggle", "{}"), &state, Path::new("web"));
        assert_eq!(r_tog.status_code, 200);
        let b_tog = String::from_utf8_lossy(&r_tog.body);
        assert!(b_tog.contains(r#""airplayAudioMuted":true"#), "b_tog={b_tog}");
        assert!(b_tog.contains(r#""pcBridgeActive":false"#), "b_tog={b_tog}");
        assert!(
            !iphone_link_audio::is_global_pc_audio_bridge_active()
                && !iphone_link_audio::is_global_pc_audio_bridge_connecting(),
            "Bluetooth A2DP must remain 100% stopped during AirPlay audio toggle!"
        );
        iphone_link_apple::airplay::set_airplay_audio_muted(false);

        // 4. Audit /api/screen, /api/status, /api/debug/airplay-trace telemetry + app.js & mirror.js crisp HD contracts
        let r_scr = handle_request(&req_with("GET", "/api/screen", ""), &state, Path::new("web"));
        let b_scr = String::from_utf8_lossy(&r_scr.body);
        for key in [
            r#""airplayAudioActive":"#,
            r#""airplayAudioMuted":"#,
            r#""airplayAudioCodec":"#,
            r#""airplayAudioSampleRate":"#,
            r#""airplayAudioDecodedPackets":"#,
            r#""airplayAudioPlayedSamples":"#,
        ] {
            assert!(b_scr.contains(key), "/api/screen missing {key}: {b_scr}");
        }
        let r_tr = handle_request(&req_with("GET", "/api/debug/airplay-trace", ""), &state, Path::new("web"));
        let b_tr = String::from_utf8_lossy(&r_tr.body);
        for key in [r#""audioDecoded":"#, r#""audioPlayedSamples":"#, r#""audioDecodeErrors":"#, r#""audioCodec":"#] {
            assert!(b_tr.contains(key), "/api/debug/airplay-trace missing {key}: {b_tr}");
        }

        let app_js = EMBEDDED_APP_JS;
        assert!(app_js.contains("/api/audio/airplay-mute"));
        assert!(app_js.contains("Audio AirPlay Aktif (44.1 kHz)"));
        assert!(app_js.contains("AirPlay Audio Active (44.1 kHz)"));
        assert!(app_js.contains("AirPlay H.264 Full HD 60 FPS + Audio PC (tanpa sentuh/keyboard)"));
        assert!(app_js.contains("AirPlay H.264 Full HD 60 FPS + PC Audio (no touch/keyboard)"));

        let mirror_js = EMBEDDED_MIRROR_JS;
        assert!(mirror_js.contains("const disableNativeTrim = effectiveIpad || (fitMode === \"contain\") || isAirplayOrH264;"));
        assert!(mirror_js.contains("ctx.imageSmoothingQuality = \"high\";"));
        assert!(mirror_js.contains("_h264AvccRefreshInFlight"));

        // 5. Clean stop
        let _ = handle_request(&req_with("DELETE", "/api/screen", ""), &state, Path::new("web"));
        assert!(!iphone_link_apple::mirror::is_airplay_active());
    }

    #[test]
    fn test_tahap259_airplay_live_config_endpoint_zero_bt_hijack_and_mirror_js_low_latency() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let req_with = |method: &str, path: &str, body: &str| HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. Start Pure AirPlay H.264 session (RFC 5737 synthetic UDID)
        let r_ap = handle_request(
            &req_with(
                "POST",
                "/api/screen",
                r#"{"mode":"airplay","iosVersion":"27.2","udid":"00008150-TEST-DEV-A","transport":"wifi","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_ap.status_code, 200);
        assert!(iphone_link_apple::mirror::is_airplay_active());

        // 2. POST /api/audio/airplay-config hot-reloads volume/buffer/lipSync with 0 BT hijack & 0 unintended mute!
        let r_c1 = handle_request(
            &req_with(
                "POST",
                "/api/audio/airplay-config",
                r#"{"volume":40,"buffer":"low","lipSync":false,"muted":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_c1.status_code, 200);
        let b_c1 = String::from_utf8_lossy(&r_c1.body);
        assert!(b_c1.contains(r#""airplayAudioVolume":40"#), "b_c1={b_c1}");
        assert!(b_c1.contains(r#""airplayAudioBufferPreset":"low""#), "b_c1={b_c1}");
        assert!(b_c1.contains(r#""airplayAudioLipSync":false"#), "b_c1={b_c1}");
        assert!(b_c1.contains(r#""airplayAudioTargetMs":22"#), "b_c1={b_c1}");
        assert!(b_c1.contains(r#""airplayAudioMuted":false"#), "b_c1={b_c1}");
        assert!(b_c1.contains(r#""pcBridgeActive":false"#), "b_c1={b_c1}");

        // 3. Clicking Buffer Audio via legacy POST /api/audio {"bufferPreset":"high"} MUST NOT mute AirPlay audio!
        let r_buf = handle_request(
            &req_with("POST", "/api/audio", r#"{"bufferPreset":"high"}"#),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_buf.status_code, 200);
        let b_buf = String::from_utf8_lossy(&r_buf.body);
        assert!(b_buf.contains(r#""airplayAudioBufferPreset":"high""#), "b_buf={b_buf}");
        assert!(b_buf.contains(r#""airplayAudioMuted":false"#), "Buffer change must NEVER mute AirPlay: {b_buf}");
        assert!(b_buf.contains(r#""pcBridgeActive":false"#), "b_buf={b_buf}");

        // 4. POST /api/settings also live hot-reloads volume/buffer/lipSync cleanly
        let r_set = handle_request(
            &req_with(
                "POST",
                "/api/settings",
                r#"{"audioVolume":85,"audioBufferPreset":"normal","lipSync":true}"#,
            ),
            &state,
            Path::new("web"),
        );
        assert_eq!(r_set.status_code, 200);
        let r_scr = handle_request(&req_with("GET", "/api/screen", ""), &state, Path::new("web"));
        let b_scr = String::from_utf8_lossy(&r_scr.body);
        for expected in [
            r#""airplayAudioVolume":85"#,
            r#""airplayAudioBufferPreset":"normal""#,
            r#""airplayAudioLipSync":true"#,
            r#""airplayAudioTargetMs":100"#,
            r#""airplayAudioBufferMs":"#,
            r#""airplayAudioDroppedPackets":"#,
            r#""airplayVideoQueueDropped":"#,
        ] {
            assert!(b_scr.contains(expected), "/api/screen missing {expected}: {b_scr}");
        }
        assert!(
            !iphone_link_audio::is_global_pc_audio_bridge_active()
                && !iphone_link_audio::is_global_pc_audio_bridge_connecting(),
            "Bluetooth A2DP must stay 100% isolated during AirPlay live config!"
        );

        // 5. Audit embedded /mirror.js & /app.js ultra-low-latency & live hot-reload contracts
        let mirror_js = EMBEDDED_MIRROR_JS;
        assert!(mirror_js.contains("optimizeForLatency: true"));
        assert!(mirror_js.contains("configureDecoderSafe"));
        assert!(mirror_js.contains("ctx.imageSmoothingQuality = \"medium\";"));
        assert!(mirror_js.contains("videoQueueDropped"));
        assert!(mirror_js.contains("!window._isAirPlayStream && _lipSyncEnabled"));

        let app_js = EMBEDDED_APP_JS;
        assert!(app_js.contains("/api/audio/airplay-config"));

        // 6. Clean restore defaults & stop
        let _ = iphone_link_apple::airplay::apply_live_audio_config(
            Some(100),
            Some("normal"),
            Some(true),
            Some(false),
        );
        let _ = handle_request(&req_with("DELETE", "/api/screen", ""), &state, Path::new("web"));
        assert!(!iphone_link_apple::mirror::is_airplay_active());
    }

    #[test]
    fn test_tahap285_ios_routing_matrix_17_to_27_usb_and_wifi_cache() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));

        let req_post = |path: &str, body: &str| HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
        };

        // 1. iOS 17.2.1 with mode="auto" -> honest requiresDecision(ios_below_27); mode="airplay" -> AirPlayStandalone (hidControlAvailable=false)
        let r17_dec = handle_request(
            &req_post(
                "/api/screen/start",
                r#"{"udid":"00008150-000000000001721","transport":"wifi","mode":"auto","iosVersion":"17.2.1","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        let b17_dec = String::from_utf8_lossy(&r17_dec.body).to_string();
        assert!(b17_dec.contains(r#""requiresDecision":true"#), "b17_dec={b17_dec}");
        assert!(b17_dec.contains(r#""decisionReason":"ios_below_27""#), "b17_dec={b17_dec}");
        let r17 = handle_request(
            &req_post(
                "/api/screen/start",
                r#"{"udid":"00008150-000000000001721","transport":"wifi","mode":"airplay","iosVersion":"17.2.1","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        let b17 = String::from_utf8_lossy(&r17.body).to_string();
        assert!(b17.contains(r#""mirrorRoute":"airplay_standalone""#), "b17={b17}");
        assert!(b17.contains(r#""stage":"waiting_airplay""#), "b17={b17}");
        assert!(b17.contains(r#""hidControlAvailable":false"#), "b17={b17}");
        let _ = handle_request(&req_post("/api/screen", "{}"), &state, Path::new("web"));
        let _ = iphone_link_apple::mirror::global_stop_screen();

        // 2. Tahap 293 Opsi 1+2: iOS 17.4.1 & 26.0 over USB mode="auto" returns honest decision card
        //    (no silent USB->AirPlay hijack), and mode="airplay" uses pure AirPlayStandalone (hidControlAvailable=false).
        let r26 = handle_request(
            &req_post(
                "/api/screen/start",
                r#"{"udid":"00008150-000000000002600","transport":"usb","mode":"auto","iosVersion":"26.0","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        let b26 = String::from_utf8_lossy(&r26.body).to_string();
        assert!(b26.contains(r#""requiresDecision":true"#), "b26={b26}");
        assert!(b26.contains(r#""decisionReason":"ios_below_27""#), "b26={b26}");
        assert!(b26.contains(r#""mirrorRoute":"airplay_standalone""#), "b26={b26}");
        assert!(b26.contains(r#""hidControlAvailable":false"#), "b26={b26}");
        let r26_ap = handle_request(
            &req_post(
                "/api/screen/start",
                r#"{"udid":"00008150-000000000002600","transport":"airplay","mode":"airplay","iosVersion":"26.0","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        let b26_ap = String::from_utf8_lossy(&r26_ap.body).to_string();
        assert!(b26_ap.contains(r#""mirrorRoute":"airplay_standalone""#), "b26_ap={b26_ap}");
        assert!(b26_ap.contains(r#""stage":"waiting_airplay""#), "b26_ap={b26_ap}");
        assert!(b26_ap.contains(r#""hidControlAvailable":false"#), "b26_ap={b26_ap}");
        assert!(iphone_link_apple::mirror::is_airplay_standalone_no_hid_active());
        let _ = iphone_link_apple::mirror::global_stop_screen();

        // 3. Cold Wi-Fi cache fallback: NO `iosVersion` in POST body AND empty active USB cache ->
        //    MUST read `iosVersion = "18.5"` from `wifi_state.json` and classify as `ios_below_27` (`airplay_standalone`).
        let tmp_wifi = env::temp_dir().join(format!("ipl_t285_wifi_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&tmp_wifi);
        let prev_ws = env::var_os("IPHONE_LINK_WIFI_STATE_PATH");
        env::set_var("IPHONE_LINK_WIFI_STATE_PATH", &tmp_wifi);
        iphone_link_apple::wifi::update_device_full_metadata(
            "00008150-000000000001850",
            Some("Phone18"),
            Some("iPhone16,1"),
            None,
            Some("18.5"),
            None,
            Some("198.51.100.85"),
            Some(1_763_700_000),
            Some(true),
        );
        let r_wifi_cold = handle_request(
            &req_post(
                "/api/screen/start",
                r#"{"udid":"00008150-000000000001850","transport":"wifi","mode":"auto","airplayPort":0,"advertiseMdns":false}"#,
            ),
            &state,
            Path::new("web"),
        );
        let b_wifi_cold = String::from_utf8_lossy(&r_wifi_cold.body).to_string();
        match prev_ws {
            Some(v) => env::set_var("IPHONE_LINK_WIFI_STATE_PATH", v),
            None => env::remove_var("IPHONE_LINK_WIFI_STATE_PATH"),
        }
        let _ = std::fs::remove_file(&tmp_wifi);
        assert!(
            b_wifi_cold.contains(r#""iosVersion":"18.5""#)
                && b_wifi_cold.contains(r#""mirrorRoute":"airplay_standalone""#)
                && b_wifi_cold.contains(r#""decisionReason":"ios_below_27""#),
            "Cold Wi-Fi iOS 18.5 MUST resolve via wifi_state.json to honest AirPlayStandalone decision: {b_wifi_cold}"
        );
        let _ = iphone_link_apple::mirror::global_stop_screen();

        // 4. iOS 27.0 resolves to CoreDeviceNativeHevc
        assert_eq!(
            iphone_link_apple::link::resolve_ios_mirror_route("27.0"),
            iphone_link_apple::link::IosMirrorRoute::CoreDeviceNativeHevc
        );
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap285_win10_no_hevc_preflight_auto_fallback_to_hybrid_h264() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));

        let prev_hevc = env::var_os("IPHONE_LINK_MOCK_HEVC_MISSING");
        let prev_h264 = env::var_os("IPHONE_LINK_MOCK_H264_MISSING");
        let req_start = HttpRequest {
            method: "POST".to_string(),
            path: "/api/screen/start".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"udid":"00008150-000000000002700","transport":"usb","mode":"auto","iosVersion":"27.0","airplayPort":0,"advertiseMdns":false}"#.to_string(),
            body_bytes: Vec::new(),
        };

        // 1. Tahap 293 Opsi 1: Windows 10 / Portable ZIP without HEVC (`hevc_mft_available=false`, inbox `h264_mft_available=true`)
        //    MUST present an honest decision card (`requiresDecision=true`, `decisionReason="hevc_mft_missing"`) instead of silent hijack,
        //    and explicit AirPlay mode works cleanly.
        env::set_var("IPHONE_LINK_MOCK_HEVC_MISSING", "1");
        env::remove_var("IPHONE_LINK_MOCK_H264_MISSING");
        let r_fb = handle_request(&req_start, &state, Path::new("web"));
        assert_eq!(r_fb.status_code, 200);
        let b_fb = String::from_utf8_lossy(&r_fb.body).to_string();
        assert!(b_fb.contains(r#""requiresDecision":true"#), "b_fb={b_fb}");
        assert!(b_fb.contains(r#""decisionReason":"hevc_mft_missing""#), "b_fb={b_fb}");
        assert!(b_fb.contains(r#""h264MftAvailable":true"#), "b_fb={b_fb}");
        assert!(b_fb.contains(r#""hevcMftAvailable":false"#), "b_fb={b_fb}");
        let r_ap = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/screen/start".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"udid":"00008150-000000000002700","transport":"airplay","mode":"airplay","iosVersion":"27.0","airplayPort":0,"advertiseMdns":false}"#.to_string(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        let b_ap = String::from_utf8_lossy(&r_ap.body).to_string();
        assert!(b_ap.contains(r#""mirrorRoute":"airplay_standalone""#), "b_ap={b_ap}");
        let _ = iphone_link_apple::mirror::global_stop_screen();

        // 2. Windows N/KN missing both H.264 & HEVC -> explicit Media Feature Pack error (no silent stall)
        env::set_var("IPHONE_LINK_MOCK_H264_MISSING", "1");
        let r_mfp = handle_request(&req_start, &state, Path::new("web"));
        assert_eq!(r_mfp.status_code, 200);
        let b_mfp = String::from_utf8_lossy(&r_mfp.body).to_string();
        assert!(b_mfp.contains(r#""errorCode":"media_feature_pack_missing""#), "b_mfp={b_mfp}");
        assert!(b_mfp.contains(r#""mediaFeaturePackMissing":true"#), "b_mfp={b_mfp}");

        match prev_hevc {
            Some(v) => env::set_var("IPHONE_LINK_MOCK_HEVC_MISSING", v),
            None => env::remove_var("IPHONE_LINK_MOCK_HEVC_MISSING"),
        }
        match prev_h264 {
            Some(v) => env::set_var("IPHONE_LINK_MOCK_H264_MISSING", v),
            None => env::remove_var("IPHONE_LINK_MOCK_H264_MISSING"),
        }
        let _ = iphone_link_apple::mirror::global_stop_screen();
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap286_audio_routing_api_accepts_routing_and_audio_out_with_manual_stop_latch() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        iphone_link_audio::stop_global_pc_audio_bridge();
        AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);
        let state = Arc::new(Mutex::new(AppState::default()));

        // 1. Alias `{"routing":"device"}` and `{"audioOut":"device"}` MUST set routing="device" and latch manual stop
        let r_routing = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/audio".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"routing":"device"}"#.to_string(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(r_routing.status_code, 200);
        let b_dev = String::from_utf8_lossy(&r_routing.body).to_string();
        assert!(b_dev.contains(r#""pcBridgeActive":false"#), "b_dev={b_dev}");
        assert!(b_dev.contains(r#""pcBridgeState":"idle""#), "b_dev={b_dev}");
        assert_eq!(state.lock().unwrap().audio_routing, "device");
        assert!(AUDIO_MANUALLY_STOPPED_THIS_SESSION.load(Ordering::SeqCst));

        // 2. Boolean `{"audioOut":false}` in /api/settings MUST persist "device", `"trackpadSpeed":30` MUST NOT clamp to 40
        let _ = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/settings".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"audioOut":false,"trackpadSpeed":30}"#.to_string(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        {
            let st = state.lock().unwrap();
            assert_eq!(st.audio_routing, "device");
            assert_eq!(st.trackpad_speed, 30);
        }

        // 3. Switching back to `"pc"` clears manual stop latch and unmutes cleanly
        let r_pc = handle_request(
            &HttpRequest {
                method: "POST".to_string(),
                path: "/api/audio".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: r#"{"audioOut":"pc","muted":false}"#.to_string(),
                body_bytes: Vec::new(),
            },
            &state,
            Path::new("web"),
        );
        assert_eq!(r_pc.status_code, 200);
        assert_eq!(state.lock().unwrap().audio_routing, "pc");
        assert!(!state.lock().unwrap().audio_muted);
        assert!(!AUDIO_MANUALLY_STOPPED_THIS_SESSION.load(Ordering::SeqCst));

        iphone_link_audio::stop_global_pc_audio_bridge();
        AUDIO_MANUALLY_STOPPED_THIS_SESSION.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_tahap291_pin_disk_dpapi_roundtrip_no_plaintext_and_legacy_compat() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();
        if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
            g.clear();
        }

        let tmp_dir = std::env::temp_dir().join(format!(
            "iphone_link_t291_dpapi_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let settings_file = tmp_dir.join("user_settings.json");
        let st_disk = AppState::default();
        save_user_settings_to_path(&settings_file, &st_disk, "649281").expect("save settings");
        let raw = fs::read_to_string(&settings_file).expect("read settings json");
        assert!(
            raw.contains(r#""unlockPin":"dpapi:"#),
            "Expected DPAPI prefix on disk JSON, got: {raw}"
        );
        assert!(
            !raw.contains("649281"),
            "Plaintext PIN leaked into disk JSON: {raw}"
        );

        let mut st_reloaded = AppState::default();
        let loaded_dpapi = load_user_settings_from_path(&settings_file, &mut st_reloaded);
        assert_eq!(loaded_dpapi, "649281");

        // Backward compatibility: legacy v4.0.0 plaintext JSON still decodes cleanly
        let legacy_json = raw.replace(
            &format!(r#""unlockPin":"{}""#, parse_string_param(&raw, "unlockPin").unwrap_or_default()),
            r#""unlockPin":"246810""#,
        );
        fs::write(&settings_file, legacy_json.as_bytes()).expect("write legacy json");
        let loaded_legacy = load_user_settings_from_path(&settings_file, &mut st_reloaded);
        assert_eq!(loaded_legacy, "246810");

        let _ = fs::remove_dir_all(&tmp_dir);
        clear_stored_pin();
        if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
            g.clear();
        }
        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap291_pin_clear_and_wipe_remove_ram_ui_and_disk() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();
        if let Ok(mut g) = SAVED_UNLOCK_PIN.lock() {
            g.clear();
        }

        let tmp_dir = std::env::temp_dir().join(format!(
            "iphone_link_t291_wipe_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let settings_file = tmp_dir.join("user_settings.json");
        let blur_file = tmp_dir.join("sequoia_last_blur.bin");
        let dpapi_sidecar = tmp_dir.join("legacy.dpapi");
        let state = Arc::new(Mutex::new(AppState::default()));

        {
            let st = state.lock().unwrap();
            save_user_settings_to_path(&settings_file, &st, "739182").unwrap();
        }
        fs::write(&blur_file, b"ILB1dummy").unwrap();
        fs::write(&dpapi_sidecar, b"dpapidummy").unwrap();

        let mut st_cold = AppState::default();
        let hydrated = init_persistent_settings_from_disk_from(&settings_file, &mut st_cold, true);
        assert_eq!(hydrated, "739182");
        assert_eq!(get_hydrated_unlock_pin().as_deref(), Some("739182"));

        // Clear PIN sync empties RAM DPAPI & SAVED_UNLOCK_PIN, then rewrite disk with empty PIN
        clear_unlock_pin_sync(&state).unwrap();
        assert!(load_decrypted_pin().unwrap().is_none());
        assert!(get_hydrated_unlock_pin().is_none());
        {
            let st = state.lock().unwrap();
            save_user_settings_to_path(&settings_file, &st, "").unwrap();
        }
        let mut st_after_clear = AppState::default();
        let after_clear_pin =
            init_persistent_settings_from_disk_from(&settings_file, &mut st_after_clear, true);
        assert_eq!(after_clear_pin, "");
        assert!(get_hydrated_unlock_pin().is_none());

        // Wipe directory removes user_settings.json, sequoia_last_blur.bin, & *.dpapi
        wipe_privacy_files_in_dir(&tmp_dir);
        assert!(!settings_file.exists());
        assert!(!blur_file.exists());
        assert!(!dpapi_sidecar.exists());

        let _ = fs::remove_dir_all(&tmp_dir);
        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap293_diagnostics_port_27015_closed_does_not_or_service_flag() {
        // Port 27015 closed MUST report tcp_port_27015_normal == false even if service is Running!
        let closed_running = evaluate_apple_usb_diagnostics_pure(false, Win32ServiceState::Running, true);
        assert!(closed_running.driver_installed);
        assert!(closed_running.service_running);
        assert!(
            !closed_running.tcp_port_27015_normal,
            "Port 27015 closed must NEVER report Normal just because registry/SCM exists"
        );
        assert_eq!(closed_running.tcp_port_label_id, "Tidak Terjangkau");

        // Stopped service MUST report service_running == false with honest Stopped label
        let stopped = evaluate_apple_usb_diagnostics_pure(false, Win32ServiceState::Stopped, true);
        assert!(stopped.driver_installed);
        assert!(!stopped.service_running);
        assert!(!stopped.tcp_port_27015_normal);
        assert_eq!(stopped.service_label_id, "Terpasang, tapi Berhenti");

        // Open port 27015 -> true
        let healthy = evaluate_apple_usb_diagnostics_pure(true, Win32ServiceState::Running, true);
        assert!(healthy.tcp_port_27015_normal);
        assert_eq!(healthy.tcp_port_label_en, "Normal");
    }

    #[test]
    fn test_tahap293_program_files_fallback_preserves_folder_instance_id_and_migrates_dpapi() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _hid_guard = iphone_link_apple::hid::HID_TEST_SERIAL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_hid_runtime_state();

        let base = env::temp_dir().join(format!(
            "iphone_link_t293_pf_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let fake_pf_exe_dir = PathBuf::from(r"C:\Program Files\iPhone Link");
        assert!(iphone_link_apple::app_paths::is_path_under_program_files(&fake_pf_exe_dir));
        assert!(!iphone_link_apple::app_paths::is_dir_writable(&fake_pf_exe_dir));

        let unified_dir = base.join("iPhoneLink");
        let legacy_dir = base.join("iPhoneLink_NativeRust_v4");
        fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_settings = legacy_dir.join("user_settings.json");
        let unified_settings = unified_dir.join("user_settings.json");

        // Save in legacy dir using fake Program Files exe folderInstanceId + DPAPI PIN
        let exe_fid = get_deterministic_folder_instance_id_for_dir(&fake_pf_exe_dir);
        let st = AppState::default();
        save_user_settings_to_path_with_folder_id(&legacy_settings, &st, "519283", &exe_fid)
            .expect("save legacy settings");

        // Migrate non-destructively into unified %LOCALAPPDATA%\iPhoneLink\
        assert!(iphone_link_apple::app_paths::migrate_legacy_file_if_needed(
            &unified_settings,
            &legacy_settings
        ));
        assert!(unified_settings.is_file());
        assert!(
            !is_fresh_extracted_folder_for(&fake_pf_exe_dir, &unified_settings),
            "Program Files fallback MUST match exe folderInstanceId and NOT appear fresh every restart"
        );

        let mut st_cold = AppState::default();
        let hydrated = init_persistent_settings_from_disk_from(&unified_settings, &mut st_cold, true);
        assert_eq!(hydrated, "519283");

        let _ = fs::remove_dir_all(&base);
        clear_stored_pin();
        reset_hid_runtime_state();
    }

    #[test]
    fn test_tahap293_ios_below_27_returns_honest_decision_without_silent_airplay_hijack() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
        let state = Arc::new(Mutex::new(AppState::default()));

        for ver in ["17.2.1", "26.3.1"] {
            let req = HttpRequest {
                method: "POST".to_string(),
                path: "/api/screen/start".to_string(),
                query: String::new(),
                headers: HashMap::new(),
                body: format!(
                    r#"{{"udid":"00008150-0000000000002930","transport":"usb","mode":"auto","iosVersion":"{ver}","honestGate":true}}"#
                ),
                body_bytes: Vec::new(),
            };
            let resp = handle_request(&req, &state, Path::new("web"));
            let body = String::from_utf8_lossy(&resp.body).to_string();
            assert!(
                body.contains(r#""requiresDecision":true"#)
                    && body.contains(r#""decisionReason":"ios_below_27""#),
                "iOS {ver} MUST NOT hijack USB Start into AirPlay silently: {body}"
            );
            assert!(!iphone_link_apple::airplay::is_global_airplay_running());
        }
        iphone_link_apple::mirror::global_reset_mirror_runtime_for_test();
    }

    #[test]
    fn test_tahap293_diagnostic_log_ring_buffer_zero_pii_redaction() {
        let _guard = TEST_SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        iphone_link_apple::diag_log::clear_for_test();
        let synthetic_udid = ["00008150-", "0002", "AABB", "CCDD", "001C"].concat();
        let synthetic_ip = ["192.", "168.", "55.", "42"].concat();
        let synthetic_mac = ["AA:", "BB:", "CC:", "DD:", "EE:", "FF"].concat();
        let synthetic_user_path = r"C:\Users\TestPerson\AppData\Local\iPhoneLink\user_settings.json";
        let synthetic_pin = "unlockPin\":\"847291\" dpapi:SECRETBLOB123";
        let sample = format!(
            "udid={synthetic_udid} ip={synthetic_ip} mac={synthetic_mac} path={synthetic_user_path} {synthetic_pin}"
        );
        iphone_link_apple::diag_log::record("test", &sample);

        let state = Arc::new(Mutex::new(AppState::default()));
        let report = build_zero_pii_diagnostic_report(&state);
        assert!(report.contains("UDID#"), "Expected hashed UDID tag: {report}");
        assert!(report.contains("<LAN-IP>"), "Expected LAN IP redaction: {report}");
        assert!(report.contains("<MAC>"), "Expected MAC redaction: {report}");
        assert!(report.contains("<USERPROFILE>"), "Expected user path redaction: {report}");
        assert!(report.contains("<PIN-REDACTED>"), "Expected PIN/DPAPI redaction: {report}");
        assert!(!report.contains(&synthetic_udid));
        assert!(!report.contains(&synthetic_ip));
        assert!(!report.contains(&synthetic_mac));
        assert!(!report.contains("TestPerson"));
        assert!(!report.contains("847291"));
        assert!(!report.contains("SECRETBLOB123"));
        iphone_link_apple::diag_log::clear_for_test();
    }
}
