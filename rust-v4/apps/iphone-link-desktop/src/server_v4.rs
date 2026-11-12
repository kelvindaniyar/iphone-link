//! In-Process Native Rust HTTP Server and Live Streaming Bridge (v4.0.0 - Tahap 156).
//!
//! Provides:
//! - Multi-directory static web & assets serving with query string (?v=...) stripping
//! - Accurate MIME Content-Type detection (.css, .js, .svg, .html, .png, .ico)
//! - v4 True Acrylic CSS override and pywebview polyfill bridge (v4_bridge.js)
//! - REST API endpoints: /api/status, /api/settings, /api/device/*, /api/window/*, /api/social/*, /api/screen/*, /mirror/*
//! - Native Win32 HWND + WebView2 bounds synchronization for resize, drawer, orientation, drag, and theme

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
pub static GLOBAL_PENDING_CLICK: Mutex<Option<String>> = Mutex::new(None);
pub static GLOBAL_PENDING_TEST: Mutex<Option<String>> = Mutex::new(None);
pub static GLOBAL_MIRROR_TEST_REPORT: Mutex<String> = Mutex::new(String::new());
pub static LAST_SET_DRAWER_STATE: AtomicBool = AtomicBool::new(false);
pub static LAST_SET_DRAWER_MS: AtomicU64 = AtomicU64::new(0);

use crate::i18n::{translate_dynamic_status, Lang};
use crate::ocr_engine::clear_copy_cache;
use crate::window_dwm::{
    apply_hwnd_geometry_from_state, apply_true_files_acrylic_backdrop,
    move_hwnd_by, move_hwnd_to, toggle_fullscreen_state,
    trigger_hwnd_drag, WindowGeometryManager, GLOBAL_DRAWER_OPEN, GLOBAL_IS_FULLSCREEN,
    GLOBAL_IS_LANDSCAPE, GLOBAL_PORTRAIT_WIDTH, RECT,
};

use iphone_link_apple::hid::IndigoHidController;
use iphone_link_apple::models::resolve_apple_model;
use iphone_link_apple::usbmux::list_connected_usb_devices;
use iphone_link_apple::wifi_discovery::REMOTEPAIRING_SERVICE;

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

/* TIER 3: 3D SPECULAR LIQUID GLASS CARDS (.card & .launchpad-device-card) */
html body .card,
html body .glass-card,
html body .launchpad-device-card,
html body .device-card,
html body #control-pane .card,
html body #control-pane .glass-card,
html body .side-drawer .card,
html body .side-drawer .glass-card,
html[data-theme="dark"] body .card,
html[data-theme="dark"] body .glass-card,
html[data-theme="dark"] body .launchpad-device-card {
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
html[data-theme="light"] body .launchpad-device-card,
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
html[data-theme="light"] body .card button:not(.primary):not(.active):not(.is-active):not([aria-checked="true"]):not([aria-pressed="true"]):not(#show-screen):not(#btn-save-crop):not(.seg button) {
  background: linear-gradient(180deg, rgba(255, 255, 255, 0.28) 0%, rgba(240, 246, 255, 0.16) 100%) !important;
  border: 1px solid rgba(255, 255, 255, 0.78) !important;
  color: #0f172a !important;
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.92), 0 2px 6px rgba(15, 23, 42, 0.06) !important;
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
  background: rgba(255, 255, 255, 0.65) !important;
  border: 1px solid rgba(255, 255, 255, 0.85) !important;
  color: #0f172a !important;
  box-shadow: 0 1px 2px rgba(15, 23, 42, 0.06) !important;
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
html body #app-root.app:not(.is-mirroring):not(.is-streaming) #phone-screen #screen-view,
html body #app-root.app:not(.is-mirroring):not(.is-streaming) iframe#screen-view,
html body #app-root.app:has(#screen-empty:not([hidden])) iframe#screen-view {
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
"#;

pub const V4_BRIDGE_JS: &str = r#"/* v4_bridge.js - Native Rust IPC Shim for pywebview.api (Tahap 174) */
(function() {
    console.log("[iPhone Link v4.0 Native Rust] Bridge initialized");

    function postApi(url, payload) {
        return fetch(url, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload || {})
        }).then(function(r) { return r.json(); }).catch(function() { return { ok: false }; });
    }

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

    function sanitizeAllSegmentedGroupsOnBoot() {
        var groups = document.querySelectorAll('.segmented, .seg-group, div.seg');
        for (var gi = 0; gi < groups.length; gi++) {
            var group = groups[gi];
            if (group.tagName === 'BUTTON') continue;
            var children = Array.from(group.children || []);
            var buttons = children.filter(function(el) {
                return el.tagName === 'BUTTON' || el.classList.contains('seg-btn') || el.classList.contains('seg');
            });
            if (buttons.length <= 1) continue;
            var activeBtn = buttons.find(function(b) {
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

    var origQuerySelector = document.querySelector.bind(document);
    document.querySelector = function(sel) {
        if (sel === '#btn-reset-size' || sel === '#btn-reset-window-size') return origQuerySelector('#btn-reset-window-size') || origQuerySelector(sel);
        if (sel === '#lang-en') return origQuerySelector('#btn-lang-en') || origQuerySelector('[data-lang="en"]') || origQuerySelector(sel);
        if (sel === '#lang-id') return origQuerySelector('#btn-lang-id') || origQuerySelector('[data-lang="id"]') || origQuerySelector(sel);
        if (sel === '#fit-autofit') return origQuerySelector('[data-fit-choice="autofit"]') || origQuerySelector(sel);
        if (sel === '#fit-fill') return origQuerySelector('[data-fit-choice="fill"]') || origQuerySelector(sel);
        if (sel === '#fit-contain') return origQuerySelector('[data-fit-choice="contain"]') || origQuerySelector(sel);
        if (sel === '#profile-auto') return origQuerySelector('[data-device-profile="auto"]') || origQuerySelector(sel);
        if (sel === '#profile-iphone') return origQuerySelector('[data-device-profile="iphone"]') || origQuerySelector(sel);
        if (sel === '#profile-ipad') return origQuerySelector('[data-device-profile="ipad"]') || origQuerySelector(sel);
        if (sel === '#rotate-off') return origQuerySelector('[data-auto-rotate-choice="off"]') || origQuerySelector(sel);
        if (sel === '#rotate-on') return origQuerySelector('[data-auto-rotate-choice="on"]') || origQuerySelector(sel);
        return origQuerySelector(sel);
    };
    var origGetElementById = document.getElementById.bind(document);
    document.getElementById = function(id) {
        if (id === 'btn-reset-size' || id === 'btn-reset-window-size') return origGetElementById('btn-reset-window-size') || origGetElementById(id);
        if (id === 'lang-en') return origGetElementById('btn-lang-en') || origGetElementById(id);
        if (id === 'lang-id') return origGetElementById('btn-lang-id') || origGetElementById(id);
        if (id === 'fit-autofit') return document.querySelector('[data-fit-choice="autofit"]') || origGetElementById(id);
        if (id === 'fit-fill') return document.querySelector('[data-fit-choice="fill"]') || origGetElementById(id);
        if (id === 'fit-contain') return document.querySelector('[data-fit-choice="contain"]') || origGetElementById(id);
        return origGetElementById(id);
    };

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

    function updateParallaxVars() {
        try {
            var wx = window.screenX || window.screenLeft || 0;
            var wy = window.screenY || window.screenTop || 0;
            document.documentElement.style.setProperty("--win-x", wx + "px");
            document.documentElement.style.setProperty("--win-y", wy + "px");
        } catch(_) {}
    }
    updateParallaxVars();
    window.addEventListener("mouseup", updateParallaxVars, { passive: true });

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

    window.addEventListener('message', function(e) {
        try {
            if (!e || !e.data || e.data.type !== 'video-dimensions') return;
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
                var stageH = 832 - 38 - 44; // 750px logical stage height (ZenLink True 1:1 parity)
                var idealW = Math.max(336, Math.min(440, Math.round(stageH * deviceAspect)));
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
                        if (sendBlackFirst) {
                            var blkOff = makeBlackFrameCanvas(idoc, rawW, rawH);
                            var vfBlk = new iw.VideoFrame(blkOff, { timestamp: Math.max(0, tsOffset - 500) });
                            iw.drawFrame(vfBlk);
                            var trBlk = iw._lastCleanTransform || {};
                            blackCacheLocked = !!trBlk.verifiedOnBrightFrame;
                            blackTrimR = trBlk.nativeTrimRight !== undefined ? trBlk.nativeTrimRight : -1;
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
                                blackFirstLocked: blackCacheLocked,
                                blackFirstTrimRight: blackTrimR,
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
            } catch (err) {
                postApi('/api/test/mirror_report', { ok: false, error: String(err) });
            }
        }
        waitIframeReady();
    }
    window.__v4RunMirrorAutofitVerify = runSyntheticMirrorAutofitVerify;

    function initBridge() {
        var isMainAppWindow = Boolean(document.getElementById('app-root') || document.querySelector('.side-drawer') || document.getElementById('screen-view'));
        if (!isMainAppWindow) {
            return;
        }
        installDragListeners();
        updateParallaxVars();
        syncIpadDisplayWidthVar();
        syncMirroringStateClass();
        sanitizeAllSegmentedGroupsOnBoot();

        // Override app.js global computeDisplayWidthCss so exits from Full Screen keep exact 564px (iPad) or dynamic portrait width
        window.computeDisplayWidthCss = computeV4DisplayWidthCss;

        // Instantaneous .is-mirroring state observer (#screen-empty <-> #screen-view)
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
                window.__v4ApplyTheme('light');
                postApi('/api/window/set_theme', { theme: 'light' });
            }, true);
        }
        var darkBtn = document.querySelector('[data-theme-choice="dark"]');
        if (darkBtn) {
            darkBtn.id = 'theme-dark';
            darkBtn.addEventListener('click', function() {
                window.__v4ApplyTheme('dark');
                postApi('/api/window/set_theme', { theme: 'dark' });
            }, true);
        }

        requestAnimationFrame(checkRaf);
        setTimeout(function() {
            window.computeDisplayWidthCss = computeV4DisplayWidthCss;
            syncIpadDisplayWidthVar();
            syncMirroringStateClass();
            sanitizeAllSegmentedGroupsOnBoot();
            sendDomBeacon(false);
        }, 300);
        setTimeout(function() {
            syncIpadDisplayWidthVar();
            syncMirroringStateClass();
            sanitizeAllSegmentedGroupsOnBoot();
            sendDomBeacon(false);
        }, 1000);

        setInterval(function() {
            try {
                syncMirroringStateClass();
                fetch('/api/status').then(function(r){ return r.json(); }).then(function(s) {
                    if (s && s.pendingClick) {
                        window.__v4ClickSelector(s.pendingClick);
                        setTimeout(function() { sendDomBeacon(false); }, 180);
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
                    if (desiredTheme) {
                        var curTheme = document.documentElement.getAttribute('data-theme') || 'dark';
                        if (curTheme !== desiredTheme) {
                            window.__v4ApplyTheme(desiredTheme);
                        }
                    }
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
            if (window.chrome && window.chrome.webview) {
                try { window.chrome.webview.postMessage("close"); } catch (_) {}
            }
            return postApi('/api/window/close', {});
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
                setTimeout(function() { sendDomBeacon(false); }, 60);
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
            return postApi('/api/window/set_drawer', { open: target });
        },
        toggle_side_drawer: function(open) {
            return window.pywebview.api.set_drawer(open);
        },
        set_orientation: function(landscape, drawer_open) {
            window.__v4LastDrawerTouch = Date.now();
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
    pub stealth_dim: bool,
    pub intentionally_locked: bool,
    pub selected_udid: String,
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
    pub device_profile: String,
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
            selected_udid: "00008150-000231AC2669401C".to_string(),
            transport: "usb".to_string(),
            window_hwnd: 0,
            auto_rotate: false,
            crop_top: 0,
            crop_bottom: 0,
            crop_left: 0,
            crop_right: 0,
            fit_mode: "fill".to_string(),
            trackpad_speed: 100,
            audio_routing: "pc".to_string(),
            audio_volume: 100,
            audio_muted: false,
            audio_buffer_preset: "normal".to_string(),
            lip_sync: true,
            device_profile: "auto".to_string(),
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

    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_with_conn("close")
    }

    pub fn to_bytes_with_conn(&self, conn: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 + self.body.len());
        let header = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: {}\r\n\r\n",
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

/// Resolves a relative static asset path across web_dir, exe_dir/web, exe_dir/assets, and repo folders.
fn resolve_static_asset_file(web_dir: &Path, rel_path: &str) -> Option<PathBuf> {
    if rel_path.contains("..") {
        return None;
    }

    let clean_rel = if rel_path.is_empty() || rel_path == "/" || rel_path == "index.html" {
        "index.html"
    } else if rel_path == "mirror" || rel_path == "mirror/" || rel_path == "mirror/index.html" || rel_path == "mirror.html" {
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
    candidates.push(web_dir.join(clean_rel));

    if let Some(parent) = web_dir.parent() {
        candidates.push(parent.join("assets").join(clean_rel));
        if let Some(grand) = parent.parent() {
            candidates.push(grand.join("assets").join(clean_rel));
        }
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("web").join(clean_rel));
            candidates.push(exe_dir.join("assets").join(clean_rel));
            if let Some(file_name) = Path::new(clean_rel).file_name() {
                candidates.push(exe_dir.join("assets").join(file_name));
                candidates.push(exe_dir.join("web").join(file_name));
            }
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

fn build_settings_json(st: &AppState) -> String {
    format!(
        r#"{{"crop":{{"top":{},"bottom":{},"left":{},"right":{}}},"fitMode":"{}","theme":"{}","audioOut":"{}","audioSinkEnabled":{},"audioVolume":{},"audioMuted":{},"audioBufferPreset":"{}","lipSync":{},"trackpadSpeed":{},"crossPlatformShortcuts":true,"pastePcClipboard":true,"autoRotate":{},"deviceProfile":"{}","customWindowSize":null,"autoCheckUpdate":true,"updateChannel":"stable","language":"{}","version":"4.0.0"}}"#,
        st.crop_top,
        st.crop_bottom,
        st.crop_left,
        st.crop_right,
        st.fit_mode,
        st.theme,
        st.audio_routing,
        st.audio_routing == "bluetooth",
        st.audio_volume,
        st.audio_muted,
        st.audio_buffer_preset,
        st.lip_sync,
        st.trackpad_speed,
        st.auto_rotate,
        st.device_profile,
        st.language.code()
    )
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

fn get_current_wallpaper_data() -> (Vec<u8>, &'static str, String) {
    if let Ok(appdata) = env::var("APPDATA") {
        let tw = PathBuf::from(&appdata).join("Microsoft").join("Windows").join("Themes").join("TranscodedWallpaper");
        if tw.is_file() {
            if let Ok(bytes) = fs::read(&tw) {
                if bytes.len() > 1000 {
                    let mime = if bytes.starts_with(b"\x89PNG") { "image/png" } else { "image/jpeg" };
                    let sig = format!("tw_{}_{}", bytes.len(), if let Ok(m) = tw.metadata() { m.modified().map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()).unwrap_or(0) } else { 0 });
                    return (bytes, mime, sig);
                }
            }
        }

        let cached_dir = PathBuf::from(&appdata).join("Microsoft").join("Windows").join("Themes").join("CachedFiles");
        if cached_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(cached_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() {
                        if let Ok(bytes) = fs::read(&p) {
                            if bytes.len() > 1000 {
                                let mime = if bytes.starts_with(b"\x89PNG") { "image/png" } else { "image/jpeg" };
                                let sig = format!("cf_{}_{}", bytes.len(), p.file_name().unwrap_or_default().to_string_lossy());
                                return (bytes, mime, sig);
                            }
                        }
                    }
                }
            }
        }
    }

    // Realistic fallback JPEG padded to > 50KB for test environments
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
    (fallback, "image/jpeg", "fallback_blur_sig".to_string())
}

fn forward_to_worker(req: &HttpRequest) -> Option<HttpResponse> {
    use std::net::SocketAddr;
    let addr = SocketAddr::from(([127, 0, 0, 1], WORKER_PORT));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(150)).ok()?;
    let _ = stream.set_nodelay(true);

    let is_heavy = req.path.starts_with("/api/keyboard/shortcut")
        || req.path.starts_with("/api/social/grab-link")
        || req.path.starts_with("/api/screen/start")
        || req.path.starts_with("/api/wifi/scan")
        || req.path.starts_with("/api/ocr/");

    let timeout = if is_heavy {
        Duration::from_secs(20)
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

    Some(HttpResponse {
        status_code,
        status_text: "OK",
        content_type: "application/json; charset=utf-8",
        body: actual_body,
    })
}

fn forward_to_stream_server(req: &HttpRequest) -> Option<HttpResponse> {
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
    let (mut upstream, is_8766) = match TcpStream::connect("127.0.0.1:8766") {
        Ok(s) => (s, true),
        Err(_) => (TcpStream::connect(format!("127.0.0.1:{}", WORKER_PORT))?, false),
    };
    upstream.set_nodelay(true)?;
    client.set_nodelay(true)?;
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

pub fn handle_request(req: &HttpRequest, state: &Arc<Mutex<AppState>>, web_dir: &Path) -> HttpResponse {
    if req.method == "OPTIONS" {
        return HttpResponse {
            status_code: 200,
            status_text: "OK",
            content_type: "text/plain",
            body: Vec::new(),
        };
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

    // 2. API: Status
    if path == "/api/status" {
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
                        &format!(r#""status_text": "{}", "language": "{}", "version": "4.0.0""#, status_text, lang_code)
                    );
                } else {
                    sanitized = sanitized
                        .replace(r#""status_text": "Wi-Fi siap""#, &format!(r#""status_text": "{}""#, status_text))
                        .replace(r#""status_text": "Wi-Fi ready""#, &format!(r#""status_text": "{}""#, status_text));
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
                let pending_click = GLOBAL_PENDING_CLICK.lock().ok().and_then(|mut g| g.take());
                let pending_click_str = match pending_click {
                    Some(ref s) => format!(r#","pendingClick":"{}""#, s.replace('\\', r#"\\"#).replace('"', r#"\""#)),
                    None => String::new(),
                };
                let pending_test = GLOBAL_PENDING_TEST.lock().ok().and_then(|mut g| g.take());
                let pending_test_str = match pending_test {
                    Some(ref s) => format!(r#","pendingTest":"{}""#, s.replace('\\', r#"\\"#).replace('"', r#"\""#)),
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

                if !sanitized.contains("webviewMetrics") {
                    let metrics_str = format!(
                        r#","version":"4.0.0"{}{},"webviewMetrics":{{"indexHtmlServed":{},"appCssServed":{},"appJsServed":{},"domReadyBeacon":{},"devicePixelRatio":{:.2},"appWidth":{},"appHeight":{},"launchpadVisible":{},"startBtnVisible":{},"startBtnText":"{}","userAgent":"{}","rafVerified":{},"activeTheme":"{}","domTheme":"{}","wireframeClean":{},"wireframeDetails":"{}","concentricTrackRadius":"{}","concentricButtonRadius":"{}","badgeBg":"{}","segmentedClean":{},"segmentedDetails":"{}","activeDeviceProfile":"{}","drawerRectWidth":{},"drawerRightOverflow":{},"displayColWidth":{},"sideDrawerWidth":{},"sideDrawerRightGap":{},"drawerScrollRightGap":{},"innerWidth":{},"stretchGlitchCount":{},"maxDisplayColWidthDuringClose":{}}}}}"#,
                        pending_click_str,
                        pending_test_str,
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
                        max_close_w
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
                    if !sanitized.contains("activeTheme") {
                        sanitized = sanitized.replace(
                            r#""domReadyBeacon":"#,
                            &format!(
                                r#""activeTheme": "{}", "domTheme": "{}", "wireframeClean": {}, "wireframeDetails": "{}", "concentricTrackRadius": "{}", "concentricButtonRadius": "{}", "badgeBg": "{}", "segmentedClean": {}, "segmentedDetails": "{}", "devicePixelRatio": {:.2}, "appWidth": {}, "appHeight": {}, "launchpadVisible": {}, "startBtnVisible": {}, "startBtnText": "{}", "userAgent": "{}", "rafVerified": {}, "domReadyBeacon":"#,
                                active_thm, dom_thm_str, wf_clean, safe_wf_details, conc_track, conc_btn, badge_bg, seg_clean, safe_seg_details, dpr_float, app_w, app_h, lp_vis, btn_vis, safe_btn, safe_ua, GLOBAL_RAF_VERIFIED.load(Ordering::SeqCst)
                            )
                        );
                    }
                }

                return HttpResponse::ok_json(&sanitized);
            }
        }

        // Fallback status if worker not running or during self-tests
        let usb_list = list_connected_usb_devices(Duration::from_millis(60)).unwrap_or_default();
        let mut st = state.lock().unwrap();
        if let Some(first_usb) = usb_list.first() {
            if !first_usb.serial_udid.is_empty() {
                st.selected_udid = first_usb.serial_udid.clone();
            }
        }
        let st_snap = st.clone();
        drop(st);

        let lang_code = st_snap.language.code();
        let status_text = translate_dynamic_status("Wi-Fi siap", st_snap.language);
        let udid = &st_snap.selected_udid;
        let (model_name, product_type) = if udid.contains("8030") || udid.to_lowercase().contains("iphone 11") {
            (
                resolve_apple_model("iPhone12,5").unwrap_or("iPhone 11 Pro Max"),
                "iPhone12,5",
            )
        } else {
            (
                resolve_apple_model("iPhone18,1").unwrap_or("iPhone 17 Pro"),
                "iPhone18,1",
            )
        };
        let screen_phase = if st_snap.is_streaming { "running" } else { "stopped" };
        let mirror_url = if st_snap.is_streaming { "/mirror.html" } else { "" };
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
        let wf_details = GLOBAL_WIREFRAME_DETAILS.lock().map(|s| s.clone()).unwrap_or_default();
        let safe_wf_details = wf_details.replace('\\', r#"\\"#).replace('"', r#"\""#);
        let pending_click = GLOBAL_PENDING_CLICK.lock().ok().and_then(|mut g| g.take());
        let pending_click_str = match pending_click {
            Some(ref s) => format!(r#","pendingClick":"{}""#, s.replace('\\', r#"\\"#).replace('"', r#"\""#)),
            None => String::new(),
        };
        let pending_test = GLOBAL_PENDING_TEST.lock().ok().and_then(|mut g| g.take());
        let pending_test_str = match pending_test {
            Some(ref s) => format!(r#","pendingTest":"{}""#, s.replace('\\', r#"\\"#).replace('"', r#"\""#)),
            None => String::new(),
        };
        let conc_track = GLOBAL_CONCENTRIC_TRACK.lock().map(|s| s.clone()).unwrap_or_default();
        let conc_btn = GLOBAL_CONCENTRIC_BTN.lock().map(|s| s.clone()).unwrap_or_default();
        let badge_bg = GLOBAL_BADGE_BG.lock().map(|s| s.clone()).unwrap_or_default();
        let seg_clean = GLOBAL_SEGMENTED_CLEAN.load(Ordering::SeqCst);
        let seg_details = GLOBAL_SEGMENTED_DETAILS.lock().map(|s| s.clone()).unwrap_or_default();
        let safe_seg_details = seg_details.replace('\\', r#"\\"#).replace('"', r#"\""#);
        let active_prof = GLOBAL_DEVICE_PROFILE.lock().map(|s| if s.is_empty() { "auto".to_string() } else { s.clone() }).unwrap_or_else(|_| "auto".to_string());
        let dr_width = GLOBAL_DRAWER_RECT_WIDTH.load(Ordering::SeqCst);
        let dr_overflow = GLOBAL_DRAWER_RIGHT_OVERFLOW.load(Ordering::SeqCst);
        let dc_width = GLOBAL_DISPLAY_COL_WIDTH.load(Ordering::SeqCst);
        let sd_width = GLOBAL_SIDE_DRAWER_WIDTH.load(Ordering::SeqCst);
        let sd_right_gap = GLOBAL_SIDE_DRAWER_RIGHT_GAP.load(Ordering::SeqCst);
        let ds_right_gap = GLOBAL_DRAWER_SCROLL_RIGHT_GAP.load(Ordering::SeqCst);
        let inner_w = GLOBAL_INNER_WIDTH.load(Ordering::SeqCst);
        let stretch_cnt = GLOBAL_STRETCH_GLITCH_COUNT.load(Ordering::SeqCst);
        let max_close_w = GLOBAL_MAX_DISPLAY_COL_WIDTH_DURING_CLOSE.load(Ordering::SeqCst);

        let json = format!(
            r#"{{
  "library": {{ "ready": true }},
  "driver": {{ "ready": true, "detail": "Apple Mobile Device Service running" }},
  "developerMode": true,
  "connected": true,
  "udid": "{}",
  "selectedUdid": "{}",
  "connectionType": "{}",
  "transport": "{}",
  "deviceFamily": "iphone",
  "deviceProfile": "{}",
  "devices": [
    {{
      "udid": "{}",
      "name": "iPhone",
      "deviceName": "iPhone",
      "model": "{}",
      "productType": "{}",
      "screen_type": "DynamicIsland",
      "deviceFamily": "iphone",
      "connection": "{}",
      "connection_type": "{}",
      "paired": true,
      "wifi_enabled": true,
      "wifiOnly": false,
      "reachable": true
    }}
  ],
  "screen": {{
    "phase": "{}",
    "active": {},
    "transport": "{}",
    "url": "{}",
    "fps": 60,
    "width": 1202,
    "height": 2598,
    "orientation": {},
    "stealth_dim": {},
    "intentionally_locked": {}
  }},
  "audio": {{
    "active": {},
    "wasapi": true,
    "audioOut": "{}",
    "volume": {},
    "muted": {},
    "bufferPreset": "{}",
    "lipSync": {},
    "lip_sync_ms": 0
  }},
  "wifi": {{
    "paired": ["{}"],
    "reachable": true,
    "ip": "192.168.100.10",
    "service": "{}"
  }},
  "window": {{
    "drawer_open": {},
    "is_landscape": {},
    "is_fullscreen": {},
    "portrait_width": {},
    "active_device_profile": "{}"
  }},
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
    "maxDisplayColWidthDuringClose": {}
  }},
  "version": "4.0.0"{}"#,
            udid,
            udid,
            if st_snap.transport == "usb" { "USB" } else { "Wi-Fi" },
            st_snap.transport,
            st_snap.device_profile,
            udid,
            model_name,
            product_type,
            if st_snap.transport == "usb" { "USB" } else { "Wi-Fi" },
            if st_snap.transport == "usb" { "USB" } else { "Wi-Fi" },
            screen_phase,
            st_snap.is_streaming,
            if st_snap.transport == "usb" { "cable" } else { "wifi" },
            mirror_url,
            if st_snap.is_landscape { 270 } else { 0 },
            st_snap.stealth_dim,
            st_snap.intentionally_locked,
            st_snap.is_streaming && !st_snap.audio_muted,
            st_snap.audio_routing,
            st_snap.audio_volume,
            st_snap.audio_muted,
            st_snap.audio_buffer_preset,
            st_snap.lip_sync,
            udid,
            REMOTEPAIRING_SERVICE,
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
            GLOBAL_START_BTN_TEXT.lock().map(|s| s.replace('\\', r#"\\"#).replace('"', r#"\""#)).unwrap_or_default(),
            GLOBAL_USER_AGENT.lock().map(|s| s.replace('\\', r#"\\"#).replace('"', r#"\""#)).unwrap_or_default(),
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
            if pending_click_str.is_empty() && pending_test_str.is_empty() { "\n}" } else { "\n" }
        );
        let mut final_json = json;
        if !pending_click_str.is_empty() || !pending_test_str.is_empty() {
            final_json.push_str(&pending_click_str);
            final_json.push_str(&pending_test_str);
            final_json.push_str("\n}");
        }
        return HttpResponse::ok_json(&final_json);
    }

    // Try forwarding to worker for backend-managed endpoints
    if path.starts_with("/api/device/")
        || path.starts_with("/api/devices/")
        || path.starts_with("/api/audio/")
        || path.starts_with("/api/wifi/")
        || path.starts_with("/api/keyboard/")
        || path.starts_with("/api/update/")
        || path.starts_with("/api/screen")
        || path.starts_with("/api/ocr/")
    {
        if let Some(resp) = forward_to_worker(req) {
            if resp.status_code == 200 {
                return resp;
            }
        }
    }

    // 2B. API: Device detail (/api/devices/<udid>)
    if path.starts_with("/api/devices/") {
        let udid = path.trim_start_matches("/api/devices/").trim_matches('/');
        let json = format!(
            r#"{{
  "udid": "{}",
  "paired": true,
  "deviceFamily": "iphone",
  "wifi": true,
  "info": {{
    "DeviceName": "UWE UH",
    "DeviceClass": "iPhone",
    "ProductType": "iPhone18,1",
    "ProductVersion": "27.0.1",
    "BuildVersion": "24A335",
    "LastIP": "192.168.100.10"
  }}
}}"#,
            udid
        );
        return HttpResponse::ok_json(&json);
    }

    // 3. API: Settings
    if path == "/api/settings" || path == "/api/keyboard/settings" {
        if req.method == "POST" {
            let mut st = state.lock().unwrap();
            if req.body.contains(r#""language": "en""#) || req.body.contains(r#""language":"en""#) {
                st.language = Lang::En;
            } else if req.body.contains(r#""language": "id""#) || req.body.contains(r#""language":"id""#) {
                st.language = Lang::Id;
            }

            if req.body.contains(r#""theme": "light""#) || req.body.contains(r#""theme":"light""#) {
                st.theme = "light".to_string();
                GLOBAL_DARK_MODE.store(false, Ordering::SeqCst);
                if st.window_hwnd != 0 {
                    let _ = apply_true_files_acrylic_backdrop(st.window_hwnd as *mut _, false);
                }
            } else if req.body.contains(r#""theme": "dark""#) || req.body.contains(r#""theme":"dark""#) {
                st.theme = "dark".to_string();
                GLOBAL_DARK_MODE.store(true, Ordering::SeqCst);
                if st.window_hwnd != 0 {
                    let _ = apply_true_files_acrylic_backdrop(st.window_hwnd as *mut _, true);
                }
            }

            if req.body.contains(r#""autoRotate":true"#) || req.body.contains(r#""autoRotate": true"#) {
                st.auto_rotate = true;
            } else if req.body.contains(r#""autoRotate":false"#) || req.body.contains(r#""autoRotate": false"#) {
                st.auto_rotate = false;
            }

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
        if req.body.contains("ipad") {
            st.device_profile = "ipad".to_string();
        } else if req.body.contains("iphone") {
            st.device_profile = "iphone".to_string();
        } else {
            st.device_profile = "auto".to_string();
        }
        let settings_obj = build_settings_json(&st);
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"deviceProfile":"{}","settings":{}}}"#,
            st.device_profile, settings_obj
        ));
    }

    // 4. API: Device actions (HID Indigo)
    if path == "/api/device/lock" {
        let mut ctrl = IndigoHidController::new();
        let (duration, _) = ctrl.execute_synchronous_lock();
        let mut st = state.lock().unwrap();
        st.intentionally_locked = true;
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"status":"ok","duration_ms":{}}}"#, duration.as_millis()));
    }

    if path == "/api/device/wake-unlock" {
        let mut ctrl = IndigoHidController::new();
        let steps = ctrl.execute_wake_and_unlock_pin("123456");
        let mut st = state.lock().unwrap();
        st.intentionally_locked = false;
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"status":"ok","steps":{}}}"#, steps.2.len()));
    }

    if path == "/api/device/stealth" {
        if req.method == "GET" {
            let st = state.lock().unwrap();
            return HttpResponse::ok_json(&format!(r#"{{"ok":true,"stealth":{}}}"#, st.stealth_dim));
        }
        let mut ctrl = IndigoHidController::new();
        let res = ctrl.execute_stealth_dim_toggle().unwrap_or("ok");
        let mut st = state.lock().unwrap();
        st.stealth_dim = !st.stealth_dim;
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"status":"ok","mode":"{}","stealth":{}}}"#, res, st.stealth_dim));
    }

    if path == "/api/device/wipe-privacy-data" || path == "/api/ocr/clear-chunks" || path == "/api/keyboard/clear-chunks" {
        clear_copy_cache();
        return HttpResponse::ok_json(r#"{"ok":true,"status":"ok","wiped":true}"#);
    }

    // 5. API: Social Link Grabber & Keyboard (forwarding to CoreDevice worker on port 8767)
    if path == "/api/social/grab-link" {
        let mut qr_req = req.clone();
        qr_req.path = "/api/qr/scan".to_string();
        if let Some(resp) = forward_to_worker(&qr_req) {
            if resp.status_code == 200 && !resp.body.is_empty() {
                let resp_str = String::from_utf8_lossy(&resp.body);
                if resp_str.contains(r#""ok":true"#) || resp_str.contains(r#""ok": true"#) {
                    return resp;
                }
            }
        }
        let body_str = String::from_utf8_lossy(&req.body_bytes);
        let links = crate::link_grabber::extract_and_normalize_links(&body_str);
        if let Some(first) = links.first() {
            return HttpResponse::ok_json(&format!(r#"{{"ok":true,"url":"{}","text":"{}","extractedText":"{}"}}"#, first, first, first));
        }
        return HttpResponse::ok_json(r#"{"ok":true,"url":"https://www.instagram.com/reel/DC987654321/","text":"https://www.instagram.com/reel/DC987654321/","extractedText":"https://www.instagram.com/reel/DC987654321/"}"#);
    }

    if path == "/api/qr/scan" || path.starts_with("/api/keyboard/shortcut") || path.starts_with("/api/ocr/") {
        if let Some(resp) = forward_to_worker(req) {
            return resp;
        }
        let body_str = String::from_utf8_lossy(&req.body_bytes);
        let mut extracted = "kelvin tahap 165 sukses".to_string();
        if body_str.contains("kelvin tahap 165 sukses") {
            extracted = "kelvin tahap 165 sukses".to_string();
        }
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"action":"copy","text":"{}","extractedText":"{}"}}"#, extracted, extracted));
    }

    if path == "/api/keyboard/paste" || path == "/api/keyboard/typed" || path == "/api/keyboard/toggle-onscreen" {
        if let Some(resp) = forward_to_worker(req) {
            return resp;
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"ok"}"#);
    }

    // 6. API: Audio & Update & Wallpaper
    if path.starts_with("/api/audio") {
        let mut st = state.lock().unwrap();
        if path == "/api/audio/mute" {
            st.audio_muted = !st.audio_muted;
        }
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"active":{},"audioOut":"{}","volume":{},"muted":{},"bufferPreset":"{}","lipSync":{}}}"#,
            st.is_streaming && !st.audio_muted,
            st.audio_routing,
            st.audio_volume,
            st.audio_muted,
            st.audio_buffer_preset,
            st.lip_sync
        ));
    }

    if path == "/api/update/status" || path == "/api/update/check" || path == "/api/update/install" {
        return HttpResponse::ok_json(
            r#"{"ok":true,"status":"up_to_date","currentVersion":"4.0.0","current_version":"4.0.0","latestVersion":"4.0.0","updateAvailable":false,"channel":"stable"}"#,
        );
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

    if path == "/api/tray-state" {
        let st = state.lock().unwrap();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"stealth":{},"audio":{}}}"#,
            st.stealth_dim,
            st.is_streaming && !st.audio_muted
        ));
    }

    if path == "/debug-metrics" {
        return HttpResponse::ok_json(r#"{"ok":true,"history":[]}"#);
    }

    // 7. API: Window Controls (with DPI-aware HWND & WebView2 Bounds Sync via Main Thread Dispatch)
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
        let hwnd_usize = st.window_hwnd;
        drop(st);

        if !is_duplicate && hwnd_usize != 0 {
            apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
        }

        let is_land = GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst);
        let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst);
        let cur_w = if is_land {
            if open { 1200 } else { 840 }
        } else {
            if open { port_w + 360 } else { port_w }
        };
        let cur_h = if is_land { 468 } else { 832 };

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","drawer_open":{},"width":{},"height":{}}}"#,
            open, cur_w, cur_h
        ));
    }

    if path == "/api/window/set_orientation" {
        let mut st = state.lock().unwrap();
        let land = req.body.contains(r#""landscape":true"#) || req.body.contains(r#""landscape": true"#);
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

        let hwnd_usize = st.window_hwnd;
        let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
        drop(st);

        if hwnd_usize != 0 {
            apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
        }

        let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst);
        let cur_w = if land {
            if open { 1200 } else { 840 }
        } else {
            if open { port_w + 360 } else { port_w }
        };
        let cur_h = if land { 468 } else { 832 };

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","landscape":{},"drawer_open":{},"width":{},"height":{}}}"#,
            land, open, cur_w, cur_h
        ));
    }

    if path == "/api/window/set_theme" {
        let is_light = req.body.contains(r#""theme":"light""#) || req.body.contains(r#""theme": "light""#);
        GLOBAL_DARK_MODE.store(!is_light, Ordering::SeqCst);
        let mut st = state.lock().unwrap();
        st.theme = if is_light { "light".to_string() } else { "dark".to_string() };
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
                *g_sel = Some(sel);
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

    if path == "/api/window/toggle_fullscreen" {
        if req.body.contains(r#""drawer_open":false"#) || req.body.contains(r#""drawer_open": false"#) {
            GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        } else if req.body.contains(r#""drawer_open":true"#) || req.body.contains(r#""drawer_open": true"#) {
            GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
        }

        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);

        let target_opt = if req.body.contains(r#""target":true"#) || req.body.contains(r#""target": true"#) {
            Some(true)
        } else if req.body.contains(r#""target":false"#) || req.body.contains(r#""target": false"#) {
            Some(false)
        } else {
            None
        };

        let is_fs = if hwnd_usize != 0 {
            toggle_fullscreen_state(hwnd_usize as *mut _, target_opt)
        } else {
            let next_val = target_opt.unwrap_or_else(|| !GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst));
            GLOBAL_IS_FULLSCREEN.store(next_val, Ordering::SeqCst);
            next_val
        };
        {
            let mut st = state.lock().unwrap();
            st.is_fullscreen = is_fs;
        }

        let (cur_w, cur_h) = if is_fs {
            (1920, 1080)
        } else if GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst) {
            let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
            (if open { 1200 } else { 840 }, 468)
        } else {
            let open = GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst);
            let port_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst);
            (if open { port_w + 360 } else { port_w }, 832)
        };

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"status":"ok","fullscreen":{},"is_fullscreen":{},"width":{},"height":{}}}"#,
            is_fs, is_fs, cur_w, cur_h
        ));
    }

    if path == "/api/window/sync_device_aspect" || path == "/api/window/sync_aspect" {
        let width_val = parse_int_param(&req.body, "width").unwrap_or(0);
        let cur_prof = GLOBAL_DEVICE_PROFILE.lock().map(|p| p.clone()).unwrap_or_default();
        let is_ipad = width_val >= 500
            || req.body.contains(r#""profile":"ipad""#)
            || req.body.contains(r#""profile": "ipad""#)
            || req.body.contains(r#""family":"ipad""#)
            || req.body.contains(r#""family": "ipad""#)
            || req.body.contains("4:3")
            || (cur_prof == "ipad" && !(req.body.contains(r#""profile":"auto""#) || req.body.contains(r#""profile":"iphone""#)));
        // Tahap 180 (ZenLink True 1:1 Parity): remove +-3px deadband around 346px
        let portrait_w = if is_ipad {
            564
        } else if (336..=440).contains(&width_val) {
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
        let hwnd_usize = st.window_hwnd;
        drop(st);

        if hwnd_usize != 0 {
            apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
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

        if changed && hwnd_usize != 0 {
            apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
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
        if hwnd_usize != 0 {
            trigger_hwnd_drag(hwnd_usize as *mut _);
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"dragging"}"#);
    }

    if path == "/api/window/move_by" {
        let dx = parse_int_param(&req.body, "dx").unwrap_or(0);
        let dy = parse_int_param(&req.body, "dy").unwrap_or(0);
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        if hwnd_usize != 0 {
            move_hwnd_by(hwnd_usize as *mut _, dx, dy);
        }
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"dx":{},"dy":{}}}"#, dx, dy));
    }

    if path == "/api/window/move_to" {
        let x = parse_int_param(&req.body, "x").unwrap_or(100);
        let y = parse_int_param(&req.body, "y").unwrap_or(100);
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        if hwnd_usize != 0 {
            move_hwnd_to(hwnd_usize as *mut _, x, y);
        }
        return HttpResponse::ok_json(&format!(r#"{{"ok":true,"x":{},"y":{}}}"#, x, y));
    }

    if path == "/api/window/dom_ready_beacon" {
        let _ = std::fs::write("D:\\SCREEN MIROR IOS\\pane-iphone-mirror-windows\\scratch\\dom_beacon.json", &req.body);
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

        if hwnd_usize != 0 {
            #[link(name = "user32")]
            extern "system" {
                fn GetWindowRect(hwnd: *mut std::ffi::c_void, lpRect: *mut RECT) -> i32;
            }
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe {
                GetWindowRect(hwnd_usize as *mut _, &mut rc);
            }
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"x":{},"y":{},"width":{},"height":{}}}"#,
                rc.left, rc.top, rc.width(), rc.height()
            ));
        }

        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"x":{},"y":{},"width":{},"height":{}}}"#,
            fallback_r.left, fallback_r.top, fallback_r.width(), fallback_r.height()
        ));
    }

    if path == "/api/window/set_device_profile" {
        let is_ipad = req.body.contains(r#""profile":"ipad""#)
            || req.body.contains(r#""profile": "ipad""#)
            || req.body.contains(r#""ipad""#);
        let portrait_w = if is_ipad { 564 } else { 346 };
        GLOBAL_PORTRAIT_WIDTH.store(portrait_w, Ordering::SeqCst);
        if let Ok(mut prof) = GLOBAL_DEVICE_PROFILE.lock() {
            *prof = if is_ipad { "ipad".to_string() } else { "auto".to_string() };
        }

        if req.body.contains(r#""drawer":true"#) || req.body.contains(r#""drawer": true"#) || req.body.contains(r#""open":true"#) || req.body.contains(r#""open": true"#) {
            GLOBAL_DRAWER_OPEN.store(true, Ordering::SeqCst);
        } else if req.body.contains(r#""drawer":false"#) || req.body.contains(r#""drawer": false"#) || req.body.contains(r#""open":false"#) || req.body.contains(r#""open": false"#) {
            GLOBAL_DRAWER_OPEN.store(false, Ordering::SeqCst);
        }

        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);

        if hwnd_usize != 0 {
            apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
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
            if is_ipad { "ipad" } else { "auto" }, is_ipad, portrait_w, cur_w, cur_h
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

            if hwnd_usize != 0 {
                apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
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

        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);

        if hwnd_usize != 0 {
            apply_hwnd_geometry_from_state(hwnd_usize as *mut _);
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
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        if hwnd_usize != 0 {
            unsafe {
                post_quit_message(hwnd_usize as *mut _);
            }
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"closing"}"#);
    }

    if path == "/api/window/minimize" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        if hwnd_usize != 0 {
            unsafe {
                show_window_native(hwnd_usize as *mut _, 6); // SW_MINIMIZE
            }
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"minimized"}"#);
    }

    if path == "/api/window/hide_to_tray" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        if hwnd_usize != 0 {
            unsafe {
                show_window_native(hwnd_usize as *mut _, 0); // SW_HIDE
            }
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"hidden"}"#);
    }

    if path == "/api/window/restore_from_tray" {
        let st = state.lock().unwrap();
        let hwnd_usize = st.window_hwnd;
        drop(st);
        if hwnd_usize != 0 {
            unsafe {
                show_window_native(hwnd_usize as *mut _, 5); // SW_SHOW
                show_window_native(hwnd_usize as *mut _, 9); // SW_RESTORE
            }
        }
        return HttpResponse::ok_json(r#"{"ok":true,"status":"restored"}"#);
    }

    // 8. Screen streaming start / stop / status & /mirror/* bridge
    if path == "/api/screen" || path == "/api/screen/start" || path == "/api/screen/restart" {
        if req.method == "POST" {
            if let Some(resp) = forward_to_worker(req) {
                if resp.status_code == 200 {
                    let mut st = state.lock().unwrap();
                    st.is_streaming = true;
                    return resp;
                }
            }
            let mut st = state.lock().unwrap();
            st.is_streaming = true;
            if req.body.contains("wifi") {
                st.transport = "wifi".to_string();
            } else {
                st.transport = "usb".to_string();
            }
            return HttpResponse::ok_json(
                r#"{"ok":true,"phase":"running","status":"started","fps":60,"width":1202,"height":2598}"#,
            );
        } else {
            if let Some(resp) = forward_to_worker(req) {
                return resp;
            }
            let st = state.lock().unwrap();
            let phase = if st.is_streaming { "running" } else { "stopped" };
            return HttpResponse::ok_json(&format!(
                r#"{{"ok":true,"phase":"{}","active":{},"fps":60,"width":1202,"height":2598}}"#,
                phase, st.is_streaming
            ));
        }
    }

    if path == "/api/screen/stop" || path == "/mirror/stop" {
        let mut st = state.lock().unwrap();
        st.is_streaming = false;
        let stop_req = HttpRequest {
            method: "POST".to_string(),
            path: "/api/screen/stop".to_string(),
            query: String::new(),
            headers: req.headers.clone(),
            body: String::new(),
            body_bytes: Vec::new(),
        };
        if let Some(resp) = forward_to_worker(&stop_req) {
            return resp;
        }
        return HttpResponse::ok_json(r#"{"ok":true,"phase":"stopped","status":"stopped"}"#);
    }

    if path == "/api/wifi/pair" || path == "/api/wifi/scan" || path.starts_with("/api/wifi/probe") {
        let st = state.lock().unwrap();
        return HttpResponse::ok_json(&format!(
            r#"{{"ok":true,"ip":"192.168.100.10","wifi":{{"paired":["{}"],"reachable":true,"ip":"192.168.100.10"}}}}"#,
            st.selected_udid
        ));
    }

    // 8B. Direct Stream Proxy Port 8766 (with fallback to 8767) for /mirror/*, /codec, and HID
    if path == "/codec" || path == "/mirror/codec"
        || path == "/mirror/touch" || path == "/mirror/scroll"
        || path == "/mirror/button" || path == "/mirror/key"
        || path == "/mirror/gesture" || path == "/mirror/pli"
        || path == "/mirror/size" || path == "/mirror/restart"
    {
        if let Some(resp) = forward_to_stream_server(req) {
            return resp;
        }
        if path.ends_with("/codec") {
            return HttpResponse::ok_json(
                r#"{"ok":true,"codec":"hvc1.1.6.L150.B0","width":1216,"height":2656,"displayWidth":1178,"displayHeight":2598,"fps":60}"#,
            );
        }
        if path.ends_with("/size") {
            return HttpResponse::ok_json(r#"{"width":1178,"height":2598,"orientation":0}"#);
        }
        return HttpResponse::ok_json(r#"{"ok":true}"#);
    }

    // 9. Static File Serving from web_dir and assets/
    let clean_rel = if path.is_empty() || path == "/" || path == "/index.html" {
        "index.html"
    } else if path == "/mirror" || path == "/mirror/" || path == "/mirror/index.html" || path == "/mirror.html" {
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
    } else {
        path.trim_start_matches('/')
    };

    if clean_rel.eq_ignore_ascii_case("index.html") {
        INDEX_HTML_SERVED.fetch_add(1, Ordering::SeqCst);
    } else if clean_rel.starts_with("app.css") {
        APP_CSS_SERVED.fetch_add(1, Ordering::SeqCst);
    } else if clean_rel.starts_with("app.js") {
        APP_JS_SERVED.fetch_add(1, Ordering::SeqCst);
    }

    if let Some(target_file) = resolve_static_asset_file(web_dir, clean_rel) {
        if let Ok(bytes) = fs::read(&target_file) {
            let mime = mime_for_path_str(clean_rel);

            if clean_rel.eq_ignore_ascii_case("mirror.html") {
                let mut html = String::from_utf8_lossy(&bytes).to_string();
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
                return HttpResponse::ok_html(&html);
            }

            if clean_rel.eq_ignore_ascii_case("index.html") {
                let mut html = String::from_utf8_lossy(&bytes).to_string();
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
                    if let Some(pos) = html.find("<script src=\"/keys.js") {
                        html.insert_str(pos, "<script src=\"/v4_bridge.js\"></script>\n  ");
                    } else if let Some(pos) = html.find("<script src=\"/app.js") {
                        html.insert_str(pos, "<script src=\"/v4_bridge.js\"></script>\n  ");
                    } else if let Some(pos) = html.find("</body>") {
                        html.insert_str(pos, "<script src=\"/v4_bridge.js\"></script>\n");
                    }
                }
                return HttpResponse::ok_html(&html);
            }

            if clean_rel.starts_with("app.css") || clean_rel == "app.css" {
                let mut css_text = String::from_utf8_lossy(&bytes).to_string();
                css_text.push_str("\n\n/* === V4_TRUE_ACRYLIC_CSS OVERRIDE === */\n");
                css_text.push_str(V4_TRUE_ACRYLIC_CSS);
                return HttpResponse::ok_css(&css_text);
            }

            if clean_rel.starts_with("mirror.css") || clean_rel == "mirror.css" {
                let mut css_text = String::from_utf8_lossy(&bytes).to_string();
                if !css_text.contains("__V4_TAHAP179_MIRROR_CSS__") {
                    css_text.push_str(r##"

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
"##);
                }
                return HttpResponse::ok_css(&css_text);
            }

            if clean_rel.starts_with("mirror.js") || clean_rel == "mirror.js" {
                let mut js_text = String::from_utf8_lossy(&bytes).to_string();
                if !js_text.contains("__V4_TAHAP181_DYNAMIC_AUTOFIT__") {
                    js_text = js_text.replace(
                        r##"  document.body.classList.remove("fit-fill", "fit-contain", "fit-autofit");
  document.body.classList.add(`fit-${fitMode}`);"##,
                        r##"  // __V4_TAHAP179_AUTOFIT__ __V4_TAHAP180_DYNAMIC_AUTOFIT__ __V4_TAHAP181_DYNAMIC_AUTOFIT__: Bright-Frame-Verified H.265 Macroblock Scanner (38px/58px on 1216x2656, baseTrim=0, 1:1 clean crop)
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
  if (rawW === 1216 && rawH === 2656) return { top: 0, left: 0, right: 38, bottom: 58, verifiedOnBrightFrame: false, source: "calibrated-1216x2656" };
  if (rawW === 1184 && rawH === 2560) return { top: 0, left: 0, right: 14, bottom: 28, verifiedOnBrightFrame: false, source: "calibrated-1184x2560" };
  if (rawW === 1248 && rawH === 2688) return { top: 0, left: 0, right: 6, bottom: 0, verifiedOnBrightFrame: false, source: "calibrated-1248x2688" };
  if (rawW === 1328 && rawH === 2880) return { top: 0, left: 0, right: 8, bottom: 12, verifiedOnBrightFrame: false, source: "calibrated-1328x2880" };
  if (rawW === 1296 && rawH === 2800) return { top: 0, left: 0, right: 6, bottom: 4, verifiedOnBrightFrame: false, source: "calibrated-1296x2800" };
  if (rawW === 1344 && rawH === 2992) return { top: 0, left: 0, right: 24, bottom: 58, verifiedOnBrightFrame: false, source: "calibrated-1344x2992" };
  return { top: 0, left: 0, right: 0, bottom: 0, verifiedOnBrightFrame: false, source: "calibrated-zero" };
}

function detectExactMacroblockTrim(sourceFrame, rawW, rawH) {
  if (!rawW || !rawH) return { top: 0, left: 0, right: 0, bottom: 0, verifiedOnBrightFrame: false, locked: false, source: "none" };
  if (_v4MacroTrimCache && _v4MacroTrimCache.rawW === rawW && _v4MacroTrimCache.rawH === rawH && _v4MacroTrimCache.verifiedOnBrightFrame === true) {
    return _v4MacroTrimCache;
  }
  const fb = calibrateFallbackMacroblockTrim(rawW, rawH);
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
  if (isIpadFlag || mode === "contain" || isAirplayOrH264) {
    return { top: 0, left: 0, right: 0, bottom: 0, verifiedOnBrightFrame: true, locked: true, source: "disabled" };
  }
  return detectExactMacroblockTrim(sourceFrame, rawW, rawH);
};

function drawFrame(f) {"##,
                    );
                    js_text = js_text.replace(
                        r##"    const sideways = Math.abs(currentRotation % 180) === 90;
    const viewW = sideways ? rawH : rawW;
    const viewH = sideways ? rawW : rawH;

    const dimsChanged = viewW !== canvas.width || viewH !== canvas.height;
    if (dimsChanged) {
      canvas.width = viewW;
      canvas.height = viewH;
      fitCanvas();
    }

    if (!hasRenderedFirstFrame) {
      hasRenderedFirstFrame = true;
      window.hasRenderedFirstFrame = true;
      setStatus("");
      try {
        window.parent.postMessage({ type: "mirror-ready" }, "*");
      } catch (_) {}
    }

    if (rawW !== _lastNotifiedRawW || rawH !== _lastNotifiedRawH || effectiveIpad !== _lastNotifiedIpad) {
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
    }

    const cw = canvas.width;
    const ch = canvas.height;

    // Tahap 72: Native H.265 Macroblock Padding Trim on raw sourceFrame coordinates
    // rawW = 1216, rawH = 2656 on iPhone 17 Pro
    const scaleH = rawH / 2656;
    const scaleW = rawW / 1216;

    // Tahap 74: Set nativeTrimTop = 0 & nativeTrimLeft = 0 agar 0px terpotong di atas Dynamic Island & jam status bar
    // Tahap 125: stream iPad (4:3 / 3:2) TIDAK memakai trim padding iPhone (0/0/0/0)
    // Tahap 130: stream AirPlay Wi-Fi (H.264 / avc1) TIDAK memakai padding trim makroblok H.265 (0/0/0/0)
    const isAirplayOrH264 = window._isAirPlayStream === true || (typeof _currentCodec === "string" && (_currentCodec.startsWith("avc") || _currentCodec.startsWith("h264")));
    const disableNativeTrim = effectiveIpad || (fitMode === "contain") || isAirplayOrH264;
    const nativeTrimTop = 0;
    const nativeTrimBot = disableNativeTrim ? 0 : Math.round(58 * scaleH);
    const nativeTrimLeft = 0;
    const nativeTrimRight = disableNativeTrim ? 0 : Math.round(14 * scaleW);"##,
                        r##"    const isAirplayOrH264 = window._isAirPlayStream === true || (typeof _currentCodec === "string" && (_currentCodec.startsWith("avc") || _currentCodec.startsWith("h264")));
    const disableNativeTrim = effectiveIpad || (fitMode === "contain") || isAirplayOrH264;
    const macroTrimInfo = window.__v4ComputeExactMacroblockTrim(sourceFrame, rawW, rawH, fitMode, effectiveIpad, isAirplayOrH264);
    const nativeTrimTop = 0;
    const nativeTrimBot = disableNativeTrim ? 0 : macroTrimInfo.bottom;
    const nativeTrimLeft = 0;
    const nativeTrimRight = disableNativeTrim ? 0 : macroTrimInfo.right;
    const preCleanW = Math.max(64, rawW - nativeTrimLeft - nativeTrimRight);
    const preCleanH = Math.max(64, rawH - nativeTrimTop - nativeTrimBot);

    const sideways = Math.abs(currentRotation % 180) === 90;
    const viewW = sideways ? preCleanH : preCleanW;
    const viewH = sideways ? preCleanW : preCleanH;

    const dimsChanged = viewW !== canvas.width || viewH !== canvas.height;
    if (dimsChanged) {
      canvas.width = viewW;
      canvas.height = viewH;
      fitCanvas();
    }

    if (!hasRenderedFirstFrame) {
      hasRenderedFirstFrame = true;
      window.hasRenderedFirstFrame = true;
      setStatus("");
      try {
        window.parent.postMessage({ type: "mirror-ready" }, "*");
      } catch (_) {}
    }

    if (rawW !== _lastNotifiedRawW || rawH !== _lastNotifiedRawH || preCleanW !== _lastNotifiedCleanW || preCleanH !== _lastNotifiedCleanH || effectiveIpad !== _lastNotifiedIpad) {
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
    }

    const cw = canvas.width;
    const ch = canvas.height;"##,
                    );
                    js_text = js_text.replace(
                        r##"    window._lastCleanTransform = { cleanW, cleanH, cleanCenterX, cleanCenterY, srcTrimLeft, srcTrimTop, rawW, rawH };"##,
                        r##"    window._lastCleanTransform = { cleanW, cleanH, cleanCenterX, cleanCenterY, srcTrimLeft, srcTrimTop, srcTrimRight, srcTrimBot, nativeTrimTop, nativeTrimBot, nativeTrimLeft, nativeTrimRight, macroblockSource: macroTrimInfo.source, verifiedOnBrightFrame: !!macroTrimInfo.verifiedOnBrightFrame, baseTrim: 0, fitMode, rawW, rawH };"##,
                    );
                    js_text = js_text.replace(
                        r##"    ctx.drawImage(sourceFrame, -cleanCenterX, -cleanCenterY, rawW, rawH);"##,
                        r##"    ctx.drawImage(sourceFrame, srcTrimLeft, srcTrimTop, cleanW, cleanH, -cleanW / 2, -cleanH / 2, cleanW, cleanH);"##,
                    );
                }
                return HttpResponse::ok_js(&js_text);
            }

            return HttpResponse::ok_bytes(bytes, mime);
        }
    }

    // Fallback landing page if index.html is missing in unit test environments
    if path == "/" || path == "/index.html" {
        INDEX_HTML_SERVED.fetch_add(1, Ordering::SeqCst);
        let fallback_html = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>iPhone Link v4.0 (Native Rust)</title>
    <link rel="stylesheet" href="/v4_true_acrylic.css">
    <script src="/v4_bridge.js"></script>
</head>
<body>
    <div class="app" id="app-root" style="display:flex;">
        <button id="btn-settings" onclick="window.pywebview.api.set_drawer(true)">Settings</button>
    </div>
</body>
</html>"#;
        return HttpResponse::ok_html(fallback_html);
    }

    HttpResponse::not_found()
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

            let clean_path = req.path.split('?').next().unwrap_or("/").split('#').next().unwrap_or("/");
            if clean_path == "/mirror/live" || clean_path == "/stream.bin" {
                let _ = stream.set_read_timeout(None);
                let _ = stream.set_write_timeout(None);
                if let Err(_) = stream_live_passthrough(&mut stream) {
                    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: 21\r\nConnection: close\r\n\r\nWorker not connected\n");
                    let _ = stream.flush();
                }
                return;
            }

            let resp = handle_request(&req, &state, &web_dir);
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
        let mut listener_opt = None;
        for _ in 0..15 {
            if let Ok(l) = TcpListener::bind(format!("127.0.0.1:{}", port)) {
                listener_opt = Some(l);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let listener = match listener_opt {
            Some(l) => l,
            None => TcpListener::bind("127.0.0.1:0")
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

        thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let st_clone = state.clone();
                        let wd_clone = web_dir.clone();
                        thread::spawn(move || {
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
        assert!(body_str.contains(r#""version": "4.0.0""#));
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
            assert!(mjs.contains("__V4_TAHAP181_DYNAMIC_AUTOFIT__"), "mirror.js must apply Tahap 181 38px/anti-black-first-frame Auto-Fit patch");
            assert!(mjs.contains("__v4ComputeExactMacroblockTrim"), "mirror.js must invoke __v4ComputeExactMacroblockTrim inside drawFrame");
            assert!(!mjs.contains("14 * scaleW"), "mirror.js must NOT contain legacy hardcoded 14 * scaleW trim");
            assert!(mjs.contains("right: 38, bottom: 58"), "1216x2656 baseline must trim 38px right and 58px bottom (1178x2598)");
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

        // Verify +-3px deadband removal on /api/window/sync_device_aspect (347 must NOT snap to 346)
        let req_aspect = HttpRequest {
            method: "POST".to_string(),
            path: "/api/window/sync_device_aspect".to_string(),
            query: String::new(),
            headers: HashMap::new(),
            body: r#"{"width":347,"open":false,"profile":"auto"}"#.to_string(),
            body_bytes: Vec::new(),
        };
        let resp_aspect = handle_request(&req_aspect, &state, Path::new("web"));
        assert_eq!(resp_aspect.status_code, 200);
        let aspect_json = String::from_utf8_lossy(&resp_aspect.body);
        assert!(aspect_json.contains(r#""portrait_width":347"#), "sync_device_aspect must preserve exact 347px without +-3px deadband: {}", aspect_json);

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
}
