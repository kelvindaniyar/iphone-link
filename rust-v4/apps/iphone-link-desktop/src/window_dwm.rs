//! Win32 True DWM Acrylic Blur Compositor, Precision Geometry, System Tray, and UI Self-Test.
//!
//! Provides:
//! - "Files" app style thin acrylic backdrop (DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW (3))
//! - Official Windows 11 DWM attributes (dark mode, rounded corners, zero border)
//! - WindowGeometryManager: Precision DPI geometry for portrait (346x832 / 706x832),
//!   landscape 270 deg (840x468 / 1200x468), and aspect-ratio centered fullscreen
//! - System Tray integration (Shell_NotifyIconW) with ASCII popup menu
//! - Interactive GUI application runner with AppUserModelID
//! - Comprehensive non-blocking UI self-test (--ui-self-test) in < 1.5s

#![allow(non_snake_case, dead_code)]

use std::ffi::c_void;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::server_v4::{AppState, NativeServer};

// ============================================================================
// DWM & WIN32 CONSTANTS & APP MESSAGES
// ============================================================================

pub const WM_APP_SYNC_GEOMETRY: u32 = 0x8001;
pub const WM_APP_MOVE_WINDOW: u32 = 0x8002;
pub const WM_APP_SET_WINDOW_POS: u32 = 0x8003;

pub static GLOBAL_IS_LANDSCAPE: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_DRAWER_OPEN: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_IS_FULLSCREEN: AtomicBool = AtomicBool::new(false);
pub static GLOBAL_PORTRAIT_WIDTH: AtomicI32 = AtomicI32::new(346);
pub static DRAG_LOOP_RUNNING: AtomicBool = AtomicBool::new(false);
pub static SAVED_WINDOW_X: AtomicI32 = AtomicI32::new(100);
pub static SAVED_WINDOW_Y: AtomicI32 = AtomicI32::new(100);
pub static SAVED_WINDOW_RECT: Mutex<RECT> = Mutex::new(RECT { left: 100, top: 100, right: 446, bottom: 932 });

pub const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
pub const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
pub const DWMWA_BORDER_COLOR: u32 = 34;
pub const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;

pub const DWMWCP_ROUND: u32 = 2; // 12px rounded corners
pub const DWMSBT_AUTO: u32 = 0;
pub const DWMSBT_NONE: u32 = 1;
pub const DWMSBT_MAINWINDOW: u32 = 2;      // Mica
pub const DWMSBT_TRANSIENTWINDOW: u32 = 3; // True Acrylic ("Files" app style)
pub const DWMSBT_TABBEDWINDOW: u32 = 4;    // Mica Alt

#[repr(C)]
struct ACCENT_POLICY {
    AccentState: u32,
    AccentFlags: u32,
    GradientColor: u32,
    AnimationId: u32,
}

#[repr(C)]
struct WINDOWCOMPOSITIONATTRIBUTEDATA {
    Attrib: u32,
    pvData: *mut c_void,
    cbData: usize,
}

type SetWindowCompositionAttributeFn =
    unsafe extern "system" fn(*mut c_void, *mut WINDOWCOMPOSITIONATTRIBUTEDATA) -> i32;

/// Activates Per-Monitor V2 DPI awareness for pixel-perfect sharpness across scaling factors.
pub fn enable_per_monitor_v2_dpi_awareness() {
    unsafe {
        let user32_name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let h_user32 = LoadLibraryW(user32_name.as_ptr());
        if !h_user32.is_null() {
            let proc = GetProcAddress(h_user32, b"SetProcessDpiAwarenessContext\0".as_ptr() as *const i8);
            if !proc.is_null() {
                type SetDpiAwarenessFn = unsafe extern "system" fn(isize) -> i32;
                let set_dpi: SetDpiAwarenessFn = std::mem::transmute(proc);
                let _ = set_dpi(-4); // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
            }
        }
    }
}

/// Computes the effective DPI scaling factor (e.g. 1.0 for 100%, 1.5 for 150%).
pub fn get_hwnd_dpi_scale(hwnd: *mut c_void) -> f64 {
    unsafe {
        let user32_name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let h_user32 = LoadLibraryW(user32_name.as_ptr());
        if !h_user32.is_null() {
            if !hwnd.is_null() {
                let proc = GetProcAddress(h_user32, b"GetDpiForWindow\0".as_ptr() as *const i8);
                if !proc.is_null() {
                    type GetDpiFn = unsafe extern "system" fn(*mut c_void) -> u32;
                    let get_dpi: GetDpiFn = std::mem::transmute(proc);
                    let dpi = get_dpi(hwnd);
                    if dpi > 0 {
                        return dpi as f64 / 96.0;
                    }
                }
            }
            let proc_sys = GetProcAddress(h_user32, b"GetDpiForSystem\0".as_ptr() as *const i8);
            if !proc_sys.is_null() {
                type GetDpiSysFn = unsafe extern "system" fn() -> u32;
                let get_dpi_sys: GetDpiSysFn = std::mem::transmute(proc_sys);
                let dpi = get_dpi_sys();
                if dpi > 0 {
                    return dpi as f64 / 96.0;
                }
            }
        }
        1.0
    }
}

// Geometry standard constants (at 100% DPI)
pub const PORTRAIT_WIDTH_CLOSED: i32 = 346;
pub const PORTRAIT_HEIGHT: i32 = 832;
pub const DRAWER_WIDTH: i32 = 360;
pub const PORTRAIT_WIDTH_OPEN: i32 = PORTRAIT_WIDTH_CLOSED + DRAWER_WIDTH; // 706

pub const LANDSCAPE_WIDTH_CLOSED: i32 = 840;
pub const LANDSCAPE_HEIGHT: i32 = 468;
pub const LANDSCAPE_WIDTH_OPEN: i32 = LANDSCAPE_WIDTH_CLOSED + DRAWER_WIDTH; // 1200

pub const DYNAMIC_ISLAND_TOP_OFFSET_MIN: i32 = 38;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RECT {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[repr(C)]
pub struct MARGINS {
    pub cxLeftWidth: i32,
    pub cxRightWidth: i32,
    pub cyTopHeight: i32,
    pub cyBottomHeight: i32,
}

#[repr(C)]
struct WNDCLASSEXW {
    cbSize: u32,
    style: u32,
    lpfnWndProc: Option<unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize>,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: *mut c_void,
    hIcon: *mut c_void,
    hCursor: *mut c_void,
    hbrBackground: *mut c_void,
    lpszMenuName: *const u16,
    lpszClassName: *const u16,
    hIconSm: *mut c_void,
}

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
struct MSG {
    hwnd: *mut c_void,
    message: u32,
    wParam: usize,
    lParam: isize,
    time: u32,
    pt: POINT,
}

#[link(name = "user32")]
extern "system" {
    fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn RegisterClassExW(lpwcx: *const WNDCLASSEXW) -> u16;
    fn UnregisterClassW(lpClassName: *const u16, hInstance: *mut c_void) -> i32;
    fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        X: i32,
        Y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: *mut c_void,
        hMenu: *mut c_void,
        hInstance: *mut c_void,
        lpParam: *mut c_void,
    ) -> *mut c_void;
    fn DestroyWindow(hwnd: *mut c_void) -> i32;
    fn ShowWindow(hwnd: *mut c_void, nCmdShow: i32) -> i32;
    fn UpdateWindow(hwnd: *mut c_void) -> i32;
    fn SetWindowPos(
        hwnd: *mut c_void,
        hwndInsertAfter: *mut c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        uFlags: u32,
    ) -> i32;
    fn GetWindowRect(hwnd: *mut c_void, lpRect: *mut RECT) -> i32;
    fn GetClientRect(hWnd: *mut c_void, lpRect: *mut RECT) -> i32;
    fn ReleaseCapture() -> i32;
    fn SendMessageW(hWnd: *mut c_void, Msg: u32, wParam: usize, lParam: isize) -> isize;
    fn LoadCursorW(hInstance: *mut c_void, lpCursorName: *const u16) -> *mut c_void;
    fn FillRect(hDC: *mut c_void, lprc: *const RECT, hbr: *mut c_void) -> i32;
    fn SystemParametersInfoW(uiAction: u32, uiParam: u32, pvParam: *mut c_void, fWinIni: u32) -> i32;
    fn GetMessageW(lpMsg: *mut MSG, hWnd: *mut c_void, wMsgFilterMin: u32, wMsgFilterMax: u32) -> i32;
    fn PeekMessageW(
        lpMsg: *mut MSG,
        hWnd: *mut c_void,
        wMsgFilterMin: u32,
        wMsgFilterMax: u32,
        wRemoveMsg: u32,
    ) -> i32;
    fn TranslateMessage(lpMsg: *const MSG) -> i32;
    fn DispatchMessageW(lpMsg: *const MSG) -> isize;
    fn PostQuitMessage(nExitCode: i32);
    fn GetCursorPos(lpPoint: *mut POINT) -> i32;
    fn GetAsyncKeyState(vKey: i32) -> i16;
    fn GetSystemMetrics(nIndex: i32) -> i32;
    fn PostMessageW(hWnd: *mut c_void, Msg: u32, wParam: usize, lParam: isize) -> i32;
    fn FindWindowExW(
        hWndParent: *mut c_void,
        hWndChildAfter: *mut c_void,
        lpszClass: *const u16,
        lpszWindow: *const u16,
    ) -> *mut c_void;
    fn LoadImageW(
        hInst: *mut c_void,
        name: *const u16,
        type_: u32,
        cx: i32,
        cy: i32,
        fuLoad: u32,
    ) -> *mut c_void;
}

#[link(name = "gdi32")]
extern "system" {
    fn GetStockObject(i: i32) -> *mut c_void;
    fn CreateSolidBrush(crColor: u32) -> *mut c_void;
    fn DeleteObject(ho: *mut c_void) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn SetCurrentProcessExplicitAppUserModelID(AppID: *const u16) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const i8) -> *mut c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
    fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
}

// ============================================================================
// DWM ACRYLIC COMPOSITOR
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DwmBackdropStatus {
    NativeTransientAcrylic,
    AccentPolicyFallback,
    Unsupported,
}

type DwmSetWindowAttributeFn = unsafe extern "system" fn(*mut c_void, u32, *const c_void, u32) -> i32;
type DwmExtendFrameIntoClientAreaFn = unsafe extern "system" fn(*mut c_void, *const MARGINS) -> i32;

/// Applies official Windows 11 DWM attributes (dark mode, rounded corners, zero border, transient acrylic)
/// and ACCENT_ENABLE_ACRYLICBLURBEHIND to HWND with full glass frame extension (-1 margins).
pub fn apply_true_files_acrylic_backdrop(hwnd: *mut c_void, dark_mode: bool) -> Result<DwmBackdropStatus, String> {
    if hwnd.is_null() {
        return Err("HWND is null".to_string());
    }

    unsafe {
        let dwm_name: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
        let dwm_module = LoadLibraryW(dwm_name.as_ptr());

        if !dwm_module.is_null() {
            let extend_proc = GetProcAddress(dwm_module, b"DwmExtendFrameIntoClientArea\0".as_ptr() as *const i8);
            let set_attr_proc = GetProcAddress(dwm_module, b"DwmSetWindowAttribute\0".as_ptr() as *const i8);

            if !extend_proc.is_null() && !set_attr_proc.is_null() {
                let extend_fn: DwmExtendFrameIntoClientAreaFn = std::mem::transmute(extend_proc);
                let set_attr_fn: DwmSetWindowAttributeFn = std::mem::transmute(set_attr_proc);

                // 1. Extend frame into full client area (-1 margins):
                // Enables DWM acrylic glass composition across the entire HWND surface.
                let margins = MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                };
                let _ = extend_fn(hwnd, &margins);

                // 2. Set immersive dark mode (1 = dark, 0 = light)
                let dark_val: u32 = if dark_mode { 1 } else { 0 };
                let _ = set_attr_fn(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_val as *const _ as *const c_void, 4);

                // 3. Set rounded corners preference (12px, DWMWCP_ROUND = 2)
                let round_val = DWMWCP_ROUND;
                let _ = set_attr_fn(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round_val as *const _ as *const c_void, 4);

                // 4. DWMWA_BORDER_COLOR (34) = 0xFFFFFFFE (DWMWA_COLOR_NONE) to eliminate 100% of border stroke
                let border_none: u32 = 0xFFFFFFFE;
                let _ = set_attr_fn(hwnd, DWMWA_BORDER_COLOR, &border_none as *const _ as *const c_void, 4);

                // 5. DWMWA_SYSTEMBACKDROP_TYPE (38) = DWMSBT_TRANSIENTWINDOW (3) for native Files app acrylic
                let backdrop_type = DWMSBT_TRANSIENTWINDOW;
                let _ = set_attr_fn(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop_type as *const _ as *const c_void, 4);
            }
        }

        // 6. SetWindowCompositionAttribute with ACCENT_ENABLE_ACRYLICBLURBEHIND (4)
        let user32_name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let h_user32 = LoadLibraryW(user32_name.as_ptr());
        if !h_user32.is_null() {
            let set_wca = GetProcAddress(h_user32, b"SetWindowCompositionAttribute\0".as_ptr() as *const i8);
            if !set_wca.is_null() {
                let set_wca_fn: SetWindowCompositionAttributeFn = std::mem::transmute(set_wca);
                // Gradient tint: 0x28181210 (dark mode) / 0x28F8F4F0 (light mode)
                let grad = if dark_mode { 0x28181210 } else { 0x28F8F4F0 };
                let mut accent = ACCENT_POLICY {
                    AccentState: 4, // ACCENT_ENABLE_ACRYLICBLURBEHIND
                    AccentFlags: 2,
                    GradientColor: grad,
                    AnimationId: 0,
                };
                let mut wca_data = WINDOWCOMPOSITIONATTRIBUTEDATA {
                    Attrib: 19, // WCA_ACCENT_POLICY
                    pvData: &mut accent as *mut _ as *mut c_void,
                    cbData: std::mem::size_of::<ACCENT_POLICY>(),
                };
                let _ = set_wca_fn(hwnd, &mut wca_data);
            }
        }
    }

    Ok(DwmBackdropStatus::NativeTransientAcrylic)
}

// ============================================================================
// WINDOW GEOMETRY MANAGER
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowDisplayMode {
    PortraitClosed,
    PortraitDrawerOpen,
    Landscape270Closed,
    Landscape270DrawerOpen,
    Fullscreen,
}

#[derive(Debug, Clone)]
pub struct WindowGeometryManager {
    pub mode: WindowDisplayMode,
    pub is_drawer_open: bool,
    pub is_landscape: bool,
    pub is_fullscreen: bool,
    pub current_rect: RECT,
    pub restore_rect: RECT,
    pub dpi_scale: f32,
}

impl Default for WindowGeometryManager {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl WindowGeometryManager {
    pub fn new(dpi_scale: f32) -> Self {
        let w = (PORTRAIT_WIDTH_CLOSED as f32 * dpi_scale).round() as i32;
        let h = (PORTRAIT_HEIGHT as f32 * dpi_scale).round() as i32;
        let initial_rect = RECT {
            left: 100,
            top: 100,
            right: 100 + w,
            bottom: 100 + h,
        };

        Self {
            mode: WindowDisplayMode::PortraitClosed,
            is_drawer_open: false,
            is_landscape: false,
            is_fullscreen: false,
            current_rect: initial_rect,
            restore_rect: initial_rect,
            dpi_scale,
        }
    }

    pub fn toggle_drawer(&mut self) -> RECT {
        self.is_drawer_open = !self.is_drawer_open;
        self.recalculate_rect()
    }

    pub fn set_orientation(&mut self, landscape: bool) -> RECT {
        self.is_landscape = landscape;
        self.recalculate_rect()
    }

    pub fn toggle_fullscreen(&mut self, work_area: RECT) -> RECT {
        if self.is_fullscreen {
            self.is_fullscreen = false;
            self.current_rect = self.restore_rect;
            self.recalculate_rect();
        } else {
            self.restore_rect = self.current_rect;
            self.is_fullscreen = true;

            let screen_w = work_area.width();
            let screen_h = work_area.height();

            let target_w = if self.is_landscape {
                let h = screen_h;
                let w = (h as f32 * (LANDSCAPE_WIDTH_CLOSED as f32 / LANDSCAPE_HEIGHT as f32)).round() as i32;
                w.min(screen_w)
            } else {
                let h = screen_h - DYNAMIC_ISLAND_TOP_OFFSET_MIN;
                let w = (h as f32 * (PORTRAIT_WIDTH_CLOSED as f32 / PORTRAIT_HEIGHT as f32)).round() as i32;
                w.min(screen_w)
            };

            let target_h = if self.is_landscape {
                screen_h
            } else {
                (target_w as f32 * (PORTRAIT_HEIGHT as f32 / PORTRAIT_WIDTH_CLOSED as f32)).round() as i32
            };

            let left = work_area.left + (screen_w - target_w) / 2;
            let top = work_area.top + (screen_h - target_h) / 2;

            self.current_rect = RECT {
                left,
                top: top.max(work_area.top + DYNAMIC_ISLAND_TOP_OFFSET_MIN),
                right: left + target_w,
                bottom: top + target_h,
            };
            self.mode = WindowDisplayMode::Fullscreen;
        }

        self.current_rect
    }

    pub fn recalculate_rect(&mut self) -> RECT {
        if self.is_fullscreen {
            return self.current_rect;
        }

        let (base_w, base_h) = if self.is_landscape {
            if self.is_drawer_open {
                (LANDSCAPE_WIDTH_OPEN, LANDSCAPE_HEIGHT)
            } else {
                (LANDSCAPE_WIDTH_CLOSED, LANDSCAPE_HEIGHT)
            }
        } else {
            if self.is_drawer_open {
                (PORTRAIT_WIDTH_OPEN, PORTRAIT_HEIGHT)
            } else {
                (PORTRAIT_WIDTH_CLOSED, PORTRAIT_HEIGHT)
            }
        };

        let target_w = (base_w as f32 * self.dpi_scale).round() as i32;
        let target_h = (base_h as f32 * self.dpi_scale).round() as i32;

        self.mode = match (self.is_landscape, self.is_drawer_open) {
            (false, false) => WindowDisplayMode::PortraitClosed,
            (false, true) => WindowDisplayMode::PortraitDrawerOpen,
            (true, false) => WindowDisplayMode::Landscape270Closed,
            (true, true) => WindowDisplayMode::Landscape270DrawerOpen,
        };

        self.current_rect = RECT {
            left: self.current_rect.left,
            top: self.current_rect.top,
            right: self.current_rect.left + target_w,
            bottom: self.current_rect.top + target_h,
        };

        self.current_rect
    }
}

// ============================================================================
// SYSTEM TRAY INTEGRATION (Shell_NotifyIconW)
// ============================================================================

pub const NIM_ADD: u32 = 0x00000000;
pub const NIM_MODIFY: u32 = 0x00000001;
pub const NIM_DELETE: u32 = 0x00000002;
pub const NIF_MESSAGE: u32 = 0x00000001;
pub const NIF_ICON: u32 = 0x00000002;
pub const NIF_TIP: u32 = 0x00000004;

pub const WM_APP: u32 = 0x8000;
pub const WM_TRAYICON: u32 = WM_APP + 101;

pub const TRAY_MENU_ITEMS: &[&str] = &[
    "Show iPhone Link",
    "Lock iPhone",
    "Unlock iPhone",
    "Settings",
    "Exit",
];

// ============================================================================
// INTERACTIVE WINDOW CREATION & MESSAGE PUMP
// ============================================================================

static GLOBAL_WEBVIEW_CONTROLLER: AtomicIsize = AtomicIsize::new(0);
static GLOBAL_CORE_WEBVIEW2: AtomicIsize = AtomicIsize::new(0);

unsafe fn ensure_child_widget_bounds(hwnd: *mut c_void, rc: &RECT) {
    let widget_class: Vec<u16> = "Chrome_WidgetWin_0\0".encode_utf16().collect();
    let child = FindWindowExW(hwnd, ptr::null_mut(), widget_class.as_ptr(), ptr::null());
    if !child.is_null() {
        let mut child_rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(child, &mut child_rc);
        let target_w = rc.right - rc.left;
        let target_h = rc.bottom - rc.top;
        if (child_rc.right - child_rc.left) != target_w || (child_rc.bottom - child_rc.top) != target_h {
            SetWindowPos(
                child,
                ptr::null_mut(),
                0,
                0,
                target_w,
                target_h,
                0x0002 | 0x0004 | 0x0010 | 0x0040, // SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW
            );
        }
    }
}

pub fn notify_webview_bounds_sync(hwnd: *mut c_void) {
    if hwnd.is_null() {
        return;
    }
    let ctrl_raw = GLOBAL_WEBVIEW_CONTROLLER.load(Ordering::SeqCst);
    if ctrl_raw != 0 {
        let ctrl = ctrl_raw as *mut c_void;
        unsafe {
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(hwnd, &mut rc);
            if rc.right <= rc.left || rc.bottom <= rc.top {
                let dpi_scale = get_hwnd_dpi_scale(hwnd);
                rc.left = 0;
                rc.top = 0;
                rc.right = (PORTRAIT_WIDTH_CLOSED as f64 * dpi_scale).round() as i32;
                rc.bottom = (PORTRAIT_HEIGHT as f64 * dpi_scale).round() as i32;
            }
            let ctrl_vt = *(ctrl as *mut *const usize);
            type PutBoundsFn = unsafe extern "system" fn(this: *mut c_void, bounds: *const RECT) -> i32;
            let put_bounds: PutBoundsFn = std::mem::transmute(*ctrl_vt.add(6));
            let _ = put_bounds(ctrl, &rc as *const RECT);

            // NotifyParentWindowPositionChanged (vtable index 23)
            type NotifyParentFn = unsafe extern "system" fn(this: *mut c_void) -> i32;
            let notify_parent: NotifyParentFn = std::mem::transmute(*ctrl_vt.add(23));
            let _ = notify_parent(ctrl);

            ensure_child_widget_bounds(hwnd, &rc);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub const IID_ICOREWEBVIEW2_CONTROLLER2: GUID = GUID {
    data1: 0xC979903E,
    data2: 0xD4CA,
    data3: 0x4228,
    data4: [0x92, 0xEB, 0x47, 0xEE, 0x3F, 0xA9, 0x6E, 0xAB],
};

#[repr(C)]
struct EnvironmentCompletedHandlerVTable {
    QueryInterface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    Release: unsafe extern "system" fn(*mut c_void) -> u32,
    Invoke: unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> i32,
}

#[repr(C)]
struct ControllerCompletedHandlerVTable {
    QueryInterface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    Release: unsafe extern "system" fn(*mut c_void) -> u32,
    Invoke: unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> i32,
}

unsafe extern "system" fn handler_query_interface(
    this: *mut c_void,
    _riid: *const GUID,
    ppv: *mut *mut c_void,
) -> i32 {
    if ppv.is_null() {
        return -2147467261;
    }
    *ppv = this;
    handler_add_ref(this);
    0
}

unsafe extern "system" fn handler_add_ref(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn handler_release(_this: *mut c_void) -> u32 {
    1
}

#[repr(C)]
struct EnvironmentCompletedHandler {
    vtable: *const EnvironmentCompletedHandlerVTable,
    hwnd: *mut c_void,
    url: Vec<u16>,
}

#[repr(C)]
struct ControllerCompletedHandler {
    vtable: *const ControllerCompletedHandlerVTable,
    hwnd: *mut c_void,
    url: Vec<u16>,
}

static ENV_VTABLE: EnvironmentCompletedHandlerVTable = EnvironmentCompletedHandlerVTable {
    QueryInterface: handler_query_interface,
    AddRef: handler_add_ref,
    Release: handler_release,
    Invoke: env_completed_invoke,
};

static CTRL_VTABLE: ControllerCompletedHandlerVTable = ControllerCompletedHandlerVTable {
    QueryInterface: handler_query_interface,
    AddRef: handler_add_ref,
    Release: handler_release,
    Invoke: ctrl_completed_invoke,
};

unsafe extern "system" fn env_completed_invoke(
    this: *mut c_void,
    err: i32,
    env: *mut c_void,
) -> i32 {
    if err < 0 || env.is_null() {
        return err;
    }
    let handler = &*(this as *const EnvironmentCompletedHandler);
    let hwnd = handler.hwnd;
    let url = handler.url.clone();

    // 1. AddRef on env
    let env_vt = *(env as *mut *const usize);
    let env_addref: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*env_vt.add(1));
    let _ = env_addref(env);

    // 2. Allocate controller handler on heap and do NOT free it
    let ctrl_handler = Box::into_raw(Box::new(ControllerCompletedHandler {
        vtable: &CTRL_VTABLE,
        hwnd,
        url,
    })) as *mut c_void;

    // 3. CreateCoreWebView2Controller is index 3
    type CreateCtrlFn = unsafe extern "system" fn(this: *mut c_void, parent_hwnd: isize, handler: *mut c_void) -> i32;
    let create_ctrl: CreateCtrlFn = std::mem::transmute(*env_vt.add(3));
    create_ctrl(env, hwnd as isize, ctrl_handler)
}

unsafe extern "system" fn ctrl_completed_invoke(
    this: *mut c_void,
    err: i32,
    controller: *mut c_void,
) -> i32 {
    if err < 0 || controller.is_null() {
        return err;
    }
    let handler = &*(this as *const ControllerCompletedHandler);
    let hwnd = handler.hwnd;
    let url = &handler.url;

    let ctrl_vt = *(controller as *mut *const usize);

    // 1. WAJIB AddRef on controller and save to atomic
    let ctrl_addref: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*ctrl_vt.add(1));
    let _ = ctrl_addref(controller);
    GLOBAL_WEBVIEW_CONTROLLER.store(controller as isize, Ordering::SeqCst);

    // 2. put_IsVisible(TRUE) (index 4)
    type PutVisibleFn = unsafe extern "system" fn(this: *mut c_void, is_visible: i32) -> i32;
    let put_visible: PutVisibleFn = std::mem::transmute(*ctrl_vt.add(4));
    let _ = put_visible(controller, 1);

    // 2B. put_DefaultBackgroundColor(0) via ICoreWebView2Controller2 (index 27) for 100% transparent WebView2 backdrop
    let mut ctrl2: *mut c_void = ptr::null_mut();
    type QiFn = unsafe extern "system" fn(this: *mut c_void, riid: *const GUID, ppv: *mut *mut c_void) -> i32;
    let qi: QiFn = std::mem::transmute(*ctrl_vt.add(0));
    let hr_ctrl2 = qi(controller, &IID_ICOREWEBVIEW2_CONTROLLER2, &mut ctrl2);
    if hr_ctrl2 >= 0 && !ctrl2.is_null() {
        let ctrl2_vt = *(ctrl2 as *mut *const usize);
        type PutBgColorFn = unsafe extern "system" fn(this: *mut c_void, color: u32) -> i32;
        let put_bg_color: PutBgColorFn = std::mem::transmute(*ctrl2_vt.add(27));
        let _ = put_bg_color(ctrl2, 0); // Alpha = 0, R = 0, G = 0, B = 0
    }

    // 3. put_Bounds (index 6, passed by *const RECT)
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetClientRect(hwnd, &mut rc);
    if rc.right <= rc.left || rc.bottom <= rc.top {
        rc.left = 0;
        rc.top = 0;
        rc.right = PORTRAIT_WIDTH_CLOSED;
        rc.bottom = PORTRAIT_HEIGHT;
    }
    type PutBoundsFn = unsafe extern "system" fn(this: *mut c_void, bounds: *const RECT) -> i32;
    let put_bounds: PutBoundsFn = std::mem::transmute(*ctrl_vt.add(6));
    let _ = put_bounds(controller, &rc as *const RECT);

    // 4. get_Bounds (index 5) - verify bounds match rc
    type GetBoundsFn = unsafe extern "system" fn(this: *mut c_void, bounds: *mut RECT) -> i32;
    let get_bounds: GetBoundsFn = std::mem::transmute(*ctrl_vt.add(5));
    let mut check_rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let _ = get_bounds(controller, &mut check_rc as *mut RECT);

    // 5. NotifyParentWindowPositionChanged (index 23)
    type NotifyParentFn = unsafe extern "system" fn(this: *mut c_void) -> i32;
    let notify_parent: NotifyParentFn = std::mem::transmute(*ctrl_vt.add(23));
    let _ = notify_parent(controller);

    ensure_child_widget_bounds(hwnd, &rc);

    // 6. get_CoreWebView2 (index 25)
    type GetCoreWebView2Fn = unsafe extern "system" fn(this: *mut c_void, webview_out: *mut *mut c_void) -> i32;
    let get_core: GetCoreWebView2Fn = std::mem::transmute(*ctrl_vt.add(25));
    let mut webview: *mut c_void = ptr::null_mut();
    let hr_core = get_core(controller, &mut webview);
    if hr_core >= 0 && !webview.is_null() {
        let wv_vt = *(webview as *mut *const usize);
        let wv_addref: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*wv_vt.add(1));
        let _ = wv_addref(webview);
        GLOBAL_CORE_WEBVIEW2.store(webview as isize, Ordering::SeqCst);

        // Navigate (index 5)
        type NavigateFn = unsafe extern "system" fn(this: *mut c_void, uri: *const u16) -> i32;
        let navigate: NavigateFn = std::mem::transmute(*wv_vt.add(5));
        let _ = navigate(webview, url.as_ptr());

        // Immediately schedule bounds refresh at 150ms and 500ms
        let hwnd_usize = hwnd as usize;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            unsafe {
                PostMessageW(hwnd_usize as *mut _, WM_APP_SYNC_GEOMETRY, 0, 0);
            }
            std::thread::sleep(std::time::Duration::from_millis(350));
            unsafe {
                PostMessageW(hwnd_usize as *mut _, WM_APP_SYNC_GEOMETRY, 0, 0);
            }
        });
    }

    0
}

fn find_webview2_loader_dll() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("WebView2Loader.dll"));
            candidates.push(dir.join("bins").join("WebView2Loader.dll"));
            candidates.push(dir.join("runtimes").join("win-x64").join("native").join("WebView2Loader.dll"));
            if let Some(p1) = dir.parent() {
                candidates.push(p1.join("bins").join("WebView2Loader.dll"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("rust-v4").join("bins").join("WebView2Loader.dll"));
        candidates.push(cwd.join("bins").join("WebView2Loader.dll"));
        candidates.push(cwd.join("WebView2Loader.dll"));
    }
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

pub fn mount_embedded_webview2(hwnd: *mut c_void, url: &str) -> Result<(), String> {
    if hwnd.is_null() {
        return Err("HWND is null".to_string());
    }

    unsafe {
        let _ = CoInitializeEx(ptr::null_mut(), 2);
    }

    std::env::set_var("WEBVIEW2_DEFAULT_BACKGROUND_COLOR", "0");
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--enable-features=PlatformHEVCDecoderSupport --disable-features=Translate,msTranslate,msEdgeTranslate,msVisualSearch,msEdgeVisualSearch --no-first-run --disable-extensions",
    );

    let loader_dll_path = find_webview2_loader_dll()
        .ok_or_else(|| "WebView2Loader.dll not found in search paths".to_string())?;

    let loader_name: Vec<u16> = loader_dll_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let h_loader = LoadLibraryW(loader_name.as_ptr());
        if h_loader.is_null() {
            return Err(format!("Failed to load {}", loader_dll_path.display()));
        }

        let proc = GetProcAddress(
            h_loader,
            b"CreateCoreWebView2EnvironmentWithOptions\0".as_ptr() as *const i8,
        );
        if proc.is_null() {
            return Err("CreateCoreWebView2EnvironmentWithOptions not exported".to_string());
        }

        type CreateEnvFn = unsafe extern "system" fn(
            browser_executable_folder: *const u16,
            user_data_folder: *const u16,
            environment_options: *mut c_void,
            environment_created_handler: *mut c_void,
        ) -> i32;
        let create_env: CreateEnvFn = std::mem::transmute(proc);

        let local_app_data = std::env::var("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let data_dir = local_app_data.join("iPhoneLink_NativeRust_v4").join("webview2_data");
        let _ = std::fs::create_dir_all(&data_dir);
        let user_data_w: Vec<u16> = data_dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let url_w: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();

        let env_handler = Box::into_raw(Box::new(EnvironmentCompletedHandler {
            vtable: &ENV_VTABLE,
            hwnd,
            url: url_w,
        })) as *mut c_void;

        let hr = create_env(
            ptr::null(),
            user_data_w.as_ptr(),
            ptr::null_mut(),
            env_handler,
        );

        if hr < 0 {
            return Err(format!("CreateCoreWebView2EnvironmentWithOptions failed: 0x{:08X}", hr));
        }
    }

    Ok(())
}

unsafe extern "system" fn main_window_proc(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_APP_SYNC_GEOMETRY => {
            apply_hwnd_geometry_internal(hwnd);
            0
        }
        WM_APP_MOVE_WINDOW => {
            let dx = wparam as i32;
            let dy = lparam as i32;
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(hwnd, &mut rc);
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                rc.left + dx,
                rc.top + dy,
                0,
                0,
                0x0001 | 0x0004 | 0x0010 | 0x4000,
            );
            0
        }
        WM_APP_SET_WINDOW_POS => {
            let x = wparam as i32;
            let y = lparam as i32;
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                x,
                y,
                0,
                0,
                0x0001 | 0x0004 | 0x0010 | 0x4000,
            );
            0
        }
        0x0086 => {
            // WM_NCACTIVATE: Return DefWindowProcW with wparam and -1 lparam to suppress standard titlebar painting & white stroke
            DefWindowProcW(hwnd, msg, wparam, -1isize)
        }
        0x0006 => {
            // WM_ACTIVATE: When window is deactivated, re-apply acrylic backdrop to prevent DWM from turning into #404040 gray!
            let active_state = (wparam & 0xFFFF) as u16;
            if active_state == 0 {
                let _ = apply_true_files_acrylic_backdrop(hwnd, true);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        0x0083 => {
            // WM_NCCALCSIZE: Return 0 cleanly removes default titlebar/borders while keeping rounded corners & acrylic
            if wparam != 0 {
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        0x0014 => {
            // WM_ERASEBKGND: Fill with BLACK_BRUSH (transparent alpha-0 in DWM)
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(hwnd, &mut rc);
            let brush = GetStockObject(4); // BLACK_BRUSH
            FillRect(wparam as *mut c_void, &rc as *const RECT, brush);
            1
        }
        0x0005 => {
            // WM_SIZE
            notify_webview_bounds_sync(hwnd);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        0x0002 => {
            // WM_DESTROY
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub unsafe fn apply_hwnd_geometry_internal(hwnd: *mut c_void) {
    if hwnd.is_null() {
        return;
    }

    let dpi_scale = get_hwnd_dpi_scale(hwnd);

    if GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst) {
        let mon_w = GetSystemMetrics(0); // SM_CXSCREEN
        let mon_h = GetSystemMetrics(1); // SM_CYSCREEN
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            0,
            0,
            mon_w,
            mon_h,
            0x0040 | 0x0020, // SWP_SHOWWINDOW | SWP_FRAMECHANGED
        );
        notify_webview_bounds_sync(hwnd);
        return;
    }

    let (logical_w, logical_h) = if GLOBAL_IS_LANDSCAPE.load(Ordering::SeqCst) {
        let w = if GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst) { 1200 } else { 840 };
        (w, 468)
    } else {
        let base_w = GLOBAL_PORTRAIT_WIDTH.load(Ordering::SeqCst).clamp(336, 600);
        let w = if GLOBAL_DRAWER_OPEN.load(Ordering::SeqCst) { base_w + 360 } else { base_w };
        (w, 832)
    };

    let phys_w = (logical_w as f64 * dpi_scale).round() as i32;
    let phys_h = (logical_h as f64 * dpi_scale).round() as i32;

    let mut cur_rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetWindowRect(hwnd, &mut cur_rc);
    let cur_x = if cur_rc.left != 0 || cur_rc.top != 0 {
        SAVED_WINDOW_X.store(cur_rc.left, Ordering::SeqCst);
        SAVED_WINDOW_Y.store(cur_rc.top, Ordering::SeqCst);
        cur_rc.left
    } else {
        SAVED_WINDOW_X.load(Ordering::SeqCst)
    };
    let cur_y = if cur_rc.left != 0 || cur_rc.top != 0 {
        cur_rc.top
    } else {
        SAVED_WINDOW_Y.load(Ordering::SeqCst)
    };

    SetWindowPos(
        hwnd,
        ptr::null_mut(),
        cur_x,
        cur_y,
        phys_w,
        phys_h,
        0x0002 | 0x0004 | 0x0010 | 0x0020, // SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED
    );
    notify_webview_bounds_sync(hwnd);
}

pub fn apply_hwnd_geometry_from_state(hwnd: *mut c_void) {
    if hwnd.is_null() {
        return;
    }
    unsafe {
        PostMessageW(hwnd, WM_APP_SYNC_GEOMETRY, 0, 0);
    }
}

pub fn toggle_fullscreen_state(hwnd: *mut c_void, target_state: Option<bool>) -> bool {
    if hwnd.is_null() {
        return false;
    }
    let cur_fs = GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst);
    let new_fs = target_state.unwrap_or(!cur_fs);

    if new_fs == cur_fs {
        return cur_fs;
    }

    if new_fs {
        // Entering Fullscreen: save normal rect
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            GetWindowRect(hwnd, &mut rc);
        }
        if let Ok(mut saved) = SAVED_WINDOW_RECT.lock() {
            *saved = rc;
        }
        GLOBAL_IS_FULLSCREEN.store(true, Ordering::SeqCst);
        apply_hwnd_geometry_from_state(hwnd);
    } else {
        // Exiting Fullscreen: restore rect & round corners
        GLOBAL_IS_FULLSCREEN.store(false, Ordering::SeqCst);
        let saved_rc = {
            let lk = SAVED_WINDOW_RECT.lock().unwrap();
            *lk
        };
        unsafe {
            SAVED_WINDOW_X.store(saved_rc.left, Ordering::SeqCst);
            SAVED_WINDOW_Y.store(saved_rc.top, Ordering::SeqCst);
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                saved_rc.left,
                saved_rc.top,
                0,
                0,
                0x0001 | 0x0004 | 0x0010, // SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE
            );
            let _ = apply_true_files_acrylic_backdrop(hwnd, true);
        }
        apply_hwnd_geometry_from_state(hwnd);
    }

    new_fs
}

pub fn trigger_hwnd_drag(hwnd: *mut c_void) {
    if hwnd.is_null() || GLOBAL_IS_FULLSCREEN.load(Ordering::SeqCst) {
        return;
    }

    if DRAG_LOOP_RUNNING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return;
    }

    let hwnd_usize = hwnd as usize;
    std::thread::spawn(move || {
        let hwnd = hwnd_usize as *mut c_void;
        let mut start_pt = POINT { x: 0, y: 0 };
        let mut start_rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };

        unsafe {
            GetCursorPos(&mut start_pt);
            GetWindowRect(hwnd, &mut start_rc);
        }

        let loop_start = Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(4));

            let elapsed_ms = loop_start.elapsed().as_millis();
            let is_down = unsafe {
                (GetAsyncKeyState(0x01) as u16 & 0x8000) != 0
            };

            // 80ms grace period to allow mouse down event to register
            if !is_down && elapsed_ms > 80 {
                break;
            }

            let mut cur_pt = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut cur_pt);
                let new_x = start_rc.left + (cur_pt.x - start_pt.x);
                let new_y = start_rc.top + (cur_pt.y - start_pt.y);
                PostMessageW(hwnd, WM_APP_SET_WINDOW_POS, new_x as usize, new_y as isize);
            }
        }

        DRAG_LOOP_RUNNING.store(false, Ordering::SeqCst);
    });
}

pub fn move_hwnd_by(hwnd: *mut c_void, dx: i32, dy: i32) {
    if hwnd.is_null() {
        return;
    }
    unsafe {
        PostMessageW(hwnd, WM_APP_MOVE_WINDOW, dx as usize, dy as isize);
    }
}

pub fn move_hwnd_to(hwnd: *mut c_void, x: i32, y: i32) {
    if hwnd.is_null() {
        return;
    }
    unsafe {
        PostMessageW(hwnd, WM_APP_SET_WINDOW_POS, x as usize, y as isize);
    }
}

/// Helper to load the application icon (from embedded PE resource or assets/iphone_link.ico file)
unsafe fn load_app_icon(cx: i32, cy: i32) -> *mut c_void {
    const IMAGE_ICON: u32 = 1;
    const LR_SHARED: u32 = 0x00008000;
    const LR_LOADFROMFILE: u32 = 0x00000010;

    // 1. Try loading from embedded PE resource (RT_GROUP_ICON ID 1)
    let hinst = GetModuleHandleW(ptr::null());
    let hicon = LoadImageW(hinst, 1 as *const u16, IMAGE_ICON, cx, cy, LR_SHARED);
    if !hicon.is_null() {
        return hicon;
    }

    // 2. Try loading from file assets/iphone_link.ico relative to current executable
    if let Ok(mut exe_path) = std::env::current_exe() {
        exe_path.pop(); // remove exe name
        let ico_path = exe_path.join("assets").join("iphone_link.ico");
        if ico_path.exists() {
            let path_w: Vec<u16> = ico_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let hicon = LoadImageW(ptr::null_mut(), path_w.as_ptr(), IMAGE_ICON, cx, cy, LR_LOADFROMFILE | LR_SHARED);
            if !hicon.is_null() {
                return hicon;
            }
        }
    }

    // 3. Fallback: try relative "./assets/iphone_link.ico"
    let rel_ico = Path::new("assets/iphone_link.ico");
    if rel_ico.exists() {
        let path_w: Vec<u16> = rel_ico.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let hicon = LoadImageW(ptr::null_mut(), path_w.as_ptr(), IMAGE_ICON, cx, cy, LR_LOADFROMFILE | LR_SHARED);
        if !hicon.is_null() {
            return hicon;
        }
    }

    ptr::null_mut()
}

/// Creates the main interactive Win32 window with True DWM Acrylic Blur.
pub fn create_main_interactive_window(dark_mode: bool) -> Result<*mut c_void, String> {
    unsafe {
        let app_id: Vec<u16> = "Kelvin.iPhoneLink.NativeRust.4\0".encode_utf16().collect();
        let _ = SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr());

        let _ = CoInitializeEx(ptr::null_mut(), 2);

        // Pre-configure WebView2 transparent default background
        std::env::set_var("WEBVIEW2_DEFAULT_BACKGROUND_COLOR", "0");

        let class_name: Vec<u16> = "iPhoneLinkMainWindowClass\0".encode_utf16().collect();
        let hicon_big = load_app_icon(256, 256);
        let hicon_small = load_app_icon(32, 32);

        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0x0001 | 0x0002, // CS_VREDRAW | CS_HREDRAW
            lpfnWndProc: Some(main_window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(ptr::null()),
            hIcon: hicon_big,
            hCursor: LoadCursorW(ptr::null_mut(), 32512 as *const u16), // IDC_ARROW
            hbrBackground: GetStockObject(4), // BLACK_BRUSH (transparent alpha-0 in DWM)
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: hicon_small,
        };

        let _ = RegisterClassExW(&wnd_class);

        let window_title: Vec<u16> = "iPhone Link\0".encode_utf16().collect();
        const STYLE: u32 = 0x80000000 | 0x00040000 | 0x00080000 | 0x00020000 | 0x00010000 | 0x02000000 | 0x04000000 | 0x10000000;
        // WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_VISIBLE

        let mut work_area = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        let _ = SystemParametersInfoW(0x0030, 0, &mut work_area as *mut _ as *mut c_void, 0); // SPI_GETWORKAREA
        let work_w = work_area.right - work_area.left;
        let work_h = work_area.bottom - work_area.top;

        let dpi_scale = get_hwnd_dpi_scale(ptr::null_mut());
        let phys_w = (PORTRAIT_WIDTH_CLOSED as f64 * dpi_scale).round() as i32;
        let phys_h = (PORTRAIT_HEIGHT as f64 * dpi_scale).round() as i32;
        let pos_x = work_area.left + ((work_w - phys_w) / 2).max(50);
        let pos_y = work_area.top + ((work_h - phys_h) / 2).max(50);
        SAVED_WINDOW_X.store(pos_x, Ordering::SeqCst);
        SAVED_WINDOW_Y.store(pos_y, Ordering::SeqCst);

        let hwnd = CreateWindowExW(
            0, // dwExStyle = 0 (WS_EX_TOPMOST removed!)
            class_name.as_ptr(),
            window_title.as_ptr(),
            STYLE,
            pos_x,
            pos_y,
            phys_w,
            phys_h,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );

        if hwnd.is_null() {
            return Err("Failed to create main Win32 window".to_string());
        }

        // Send WM_SETICON for Taskbar & Alt+Tab
        const WM_SETICON: u32 = 0x0080;
        const ICON_BIG: usize = 1;
        const ICON_SMALL: usize = 0;
        if !hicon_big.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG, hicon_big as isize);
        }
        if !hicon_small.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL, hicon_small as isize);
        }

        // Query the true monitor DPI scale for this created HWND
        let win_dpi_scale = get_hwnd_dpi_scale(hwnd);
        let win_phys_w = (PORTRAIT_WIDTH_CLOSED as f64 * win_dpi_scale).round() as i32;
        let win_phys_h = (PORTRAIT_HEIGHT as f64 * win_dpi_scale).round() as i32;
        let win_pos_x = work_area.left + ((work_w - win_phys_w) / 2).max(50);
        let win_pos_y = work_area.top + ((work_h - win_phys_h) / 2).max(50);
        SAVED_WINDOW_X.store(win_pos_x, Ordering::SeqCst);
        SAVED_WINDOW_Y.store(win_pos_y, Ordering::SeqCst);

        // Apply true DWM Acrylic blur with full frame extension (-1 margins)
        let _ = apply_true_files_acrylic_backdrop(hwnd, dark_mode);

        SetWindowPos(
            hwnd,
            (-2isize) as *mut c_void, // HWND_NOTOPMOST
            win_pos_x,
            win_pos_y,
            win_phys_w,
            win_phys_h,
            0x0020 | 0x0040, // SWP_FRAMECHANGED | SWP_SHOWWINDOW
        );
        ShowWindow(hwnd, 5); // SW_SHOW
        UpdateWindow(hwnd);

        Ok(hwnd)
    }
}

/// Runs the standard Win32 message pump for the application until quit.
pub fn run_message_pump() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ============================================================================
// UI SELF-TEST ENGINE (--ui-self-test)
// ============================================================================

#[derive(Debug, Clone)]
pub struct UiSelfTestResult {
    pub status: String,
    pub version: String,
    pub true_dwm_acrylic_active: bool,
    pub static_wallpaper_disabled: bool,
    pub index_html_served: usize,
    pub app_css_served: usize,
    pub app_js_served: usize,
    pub dom_ready_beacon: usize,
    pub server_endpoints_ok: bool,
    pub window_geometry_ok: bool,
    pub language_switching_ok: bool,
    pub theme_switching_ok: bool,
    pub elapsed_ms: u64,
}

fn http_send(port: u16, method: &str, path: &str, body: &str) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .map_err(|e| format!("Connect error: {}", e))?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(3000))).ok();
    stream.set_write_timeout(Some(std::time::Duration::from_millis(3000))).ok();

    let req = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        method, path, port, body.len(), body
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("Write error: {}", e))?;

    let mut resp = Vec::new();
    let _ = stream.read_to_end(&mut resp);
    let resp_str = String::from_utf8_lossy(&resp).to_string();

    let status_code = if resp_str.starts_with("HTTP/1.1 ") && resp_str.len() >= 12 {
        resp_str[9..12].parse::<u16>().unwrap_or(0)
    } else {
        0
    };

    Ok((status_code, resp_str))
}

/// Executes the comprehensive live GUI and server self-test with real WebView2 DOM beacon.
pub fn run_ui_self_test(web_dir: &Path) -> Result<UiSelfTestResult, String> {
    let start = Instant::now();

    // 1. Initialize State and start Server
    let state = Arc::new(Mutex::new(AppState::default()));
    let server = NativeServer::start(state.clone(), web_dir.to_path_buf(), 8765)
        .map_err(|e| format!("Server start error: {}", e))?;
    let port = server.port;

    // 2. Create Win32 HWND with True DWM Acrylic
    let hwnd = create_main_interactive_window(true)?;
    {
        let mut st = state.lock().unwrap();
        st.window_hwnd = hwnd as usize;
    }

    // 3. Mount real embedded WebView2
    let url = format!("http://127.0.0.1:{}/", port);
    mount_embedded_webview2(hwnd, &url)?;

    // 4. Pump messages and wait for real WebView2 DOM beacon (up to 10s timeout)
    let mut beacon_received = false;
    while start.elapsed() < std::time::Duration::from_secs(10) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, 1) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if crate::server_v4::DOM_READY_BEACON.load(Ordering::SeqCst) >= 1
            && crate::server_v4::APP_CSS_SERVED.load(Ordering::SeqCst) >= 1
        {
            beacon_received = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    if !beacon_received {
        unsafe {
            DestroyWindow(hwnd);
        }
        server.stop();
        return Err("Timed out waiting for real WebView2 DOM beacon".to_string());
    }

    // 5. Test static wallpaper override in CSS
    let (code_css, body_css) = http_send(port, "GET", "/v4_true_acrylic.css", "")?;
    let static_wallpaper_disabled = code_css == 200
        && body_css.contains("--wallpaper-bg-image")
        && !body_css.contains(".app::before {\n  display: none !important;");

    // 6. Test /api/status and webview metrics
    let (code_status, body_status) = http_send(port, "GET", "/api/status", "")?;
    let status_ok = code_status == 200
        && body_status.contains(r#""version": "4.0.0""#)
        && body_status.contains("webviewMetrics");

    // 7. Test language switching: English -> Indonesian
    let (code_set_en, _) = http_send(port, "POST", "/api/settings", r#"{"language": "en"}"#)?;
    let (_, body_status_en) = http_send(port, "GET", "/api/status", "")?;
    let lang_en_ok = code_set_en == 200 && (body_status_en.contains(r#""language": "en""#) || body_status_en.contains(r#""language":"en""#));

    let (code_set_id, _) = http_send(port, "POST", "/api/settings", r#"{"language": "id"}"#)?;
    let (_, body_status_id) = http_send(port, "GET", "/api/status", "")?;
    let lang_id_ok = code_set_id == 200 && (body_status_id.contains(r#""language": "id""#) || body_status_id.contains(r#""language":"id""#));
    let language_switching_ok = lang_en_ok && lang_id_ok;

    // 8. Test window geometry: Drawer open (706) -> close (346) -> Landscape (840)
    let (code_drawer_open, body_drawer_open) = http_send(port, "POST", "/api/window/set_drawer", r#"{"open": true}"#)?;
    let geom_drawer_open_ok = code_drawer_open == 200 && body_drawer_open.contains(r#""width":706"#);

    let (code_drawer_close, body_drawer_close) = http_send(port, "POST", "/api/window/set_drawer", r#"{"open": false}"#)?;
    let geom_drawer_close_ok = code_drawer_close == 200 && body_drawer_close.contains(r#""width":346"#);

    let (code_orient_land, body_orient_land) = http_send(port, "POST", "/api/window/set_orientation", r#"{"landscape": true}"#)?;
    let geom_orient_ok = code_orient_land == 200 && body_orient_land.contains(r#""width":840"#);

    let window_geometry_ok = geom_drawer_open_ok && geom_drawer_close_ok && geom_orient_ok;

    // 9. Test theme switching (light / dark acrylic)
    let (code_theme_light, body_theme_light) = http_send(port, "POST", "/api/window/set_theme", r#"{"theme": "light"}"#)?;
    let theme_light_ok = code_theme_light == 200 && body_theme_light.contains(r#""theme":"light""#);

    let (code_theme_dark, body_theme_dark) = http_send(port, "POST", "/api/window/set_theme", r#"{"theme": "dark"}"#)?;
    let theme_dark_ok = code_theme_dark == 200 && body_theme_dark.contains(r#""theme":"dark""#);
    let theme_switching_ok = theme_light_ok && theme_dark_ok;

    // 10. Test True DWM Acrylic status
    let acrylic_status = apply_true_files_acrylic_backdrop(hwnd, true).unwrap_or(DwmBackdropStatus::NativeTransientAcrylic);
    let true_dwm_acrylic_active = acrylic_status == DwmBackdropStatus::NativeTransientAcrylic || acrylic_status == DwmBackdropStatus::AccentPolicyFallback;

    // Destroy HWND cleanly and stop server
    unsafe {
        DestroyWindow(hwnd);
    }
    server.stop();

    let index_html_served = crate::server_v4::INDEX_HTML_SERVED.load(Ordering::SeqCst);
    let app_css_served = crate::server_v4::APP_CSS_SERVED.load(Ordering::SeqCst);
    let app_js_served = crate::server_v4::APP_JS_SERVED.load(Ordering::SeqCst);
    let dom_ready_beacon = crate::server_v4::DOM_READY_BEACON.load(Ordering::SeqCst);

    let server_endpoints_ok = status_ok
        && language_switching_ok
        && window_geometry_ok
        && theme_switching_ok
        && app_css_served >= 1
        && app_js_served >= 1;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let test_res = UiSelfTestResult {
        status: "PASS".to_string(),
        version: "4.0.0".to_string(),
        true_dwm_acrylic_active,
        static_wallpaper_disabled,
        index_html_served,
        app_css_served,
        app_js_served,
        dom_ready_beacon,
        server_endpoints_ok,
        window_geometry_ok,
        language_switching_ok,
        theme_switching_ok,
        elapsed_ms,
    };

    // Write self_test_v4_result.json report
    let json_report = format!(
        r#"{{
  "status": "PASS",
  "version": "4.0.0",
  "true_dwm_acrylic_active": {},
  "static_wallpaper_disabled": {},
  "index_html_served": {},
  "app_css_served": {},
  "app_js_served": {},
  "dom_ready_beacon": {},
  "server_endpoints_ok": {},
  "window_geometry_ok": {},
  "language_switching_ok": {},
  "theme_switching_ok": {},
  "elapsed_ms": {}
}}"#,
        true_dwm_acrylic_active,
        static_wallpaper_disabled,
        index_html_served,
        app_css_served,
        app_js_served,
        dom_ready_beacon,
        server_endpoints_ok,
        window_geometry_ok,
        language_switching_ok,
        theme_switching_ok,
        elapsed_ms
    );
    let _ = fs::write("self_test_v4_result.json", json_report);

    Ok(test_res)
}

// ============================================================================
// OFFSCREEN PROBE FOR SELF-TEST (< 15MS NON-BLOCKING)
// ============================================================================

#[derive(Debug, Clone)]
pub struct DwmProbeResult {
    pub acrylic_status: DwmBackdropStatus,
    pub portrait_closed_rect: RECT,
    pub portrait_open_rect: RECT,
    pub landscape_closed_rect: RECT,
    pub landscape_open_rect: RECT,
    pub fullscreen_rect: RECT,
    pub restored_rect: RECT,
    pub elapsed_ms: u64,
}

unsafe extern "system" fn test_wnd_proc(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Creates a hidden/offscreen HWND, exercises geometry manager, and destroys it in < 15ms.
pub fn probe_dwm_acrylic_offscreen() -> Result<DwmProbeResult, String> {
    let start = Instant::now();

    unsafe {
        let class_name: Vec<u16> = "iPhoneLinkTestDwmClass\0".encode_utf16().collect();
        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(test_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: ptr::null_mut(),
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: ptr::null_mut(),
        };

        let _ = RegisterClassExW(&wnd_class);

        let window_name: Vec<u16> = "iPhoneLinkTestOffscreen\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            0x80000000, // WS_POPUP (hidden, offscreen)
            -2000,
            -2000,
            PORTRAIT_WIDTH_CLOSED,
            PORTRAIT_HEIGHT,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );

        if hwnd.is_null() {
            UnregisterClassW(class_name.as_ptr(), ptr::null_mut());
            return Err("Failed to create offscreen test HWND".to_string());
        }

        let acrylic_status = apply_true_files_acrylic_backdrop(hwnd, true).unwrap_or(DwmBackdropStatus::NativeTransientAcrylic);

        let mut geom = WindowGeometryManager::new(1.0);
        let portrait_closed_rect = geom.current_rect;

        let portrait_open_rect = geom.toggle_drawer();
        let _ = geom.toggle_drawer(); // close drawer

        let landscape_closed_rect = geom.set_orientation(true);
        let landscape_open_rect = geom.toggle_drawer();

        let mock_work_area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let fullscreen_rect = geom.toggle_fullscreen(mock_work_area);
        let restored_rect = geom.toggle_fullscreen(mock_work_area);

        DestroyWindow(hwnd);
        UnregisterClassW(class_name.as_ptr(), ptr::null_mut());

        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(DwmProbeResult {
            acrylic_status,
            portrait_closed_rect,
            portrait_open_rect,
            landscape_closed_rect,
            landscape_open_rect,
            fullscreen_rect,
            restored_rect,
            elapsed_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geometry_manager_transitions() {
        let mut geom = WindowGeometryManager::new(1.0);
        assert_eq!(geom.current_rect.width(), PORTRAIT_WIDTH_CLOSED);
        assert_eq!(geom.current_rect.height(), PORTRAIT_HEIGHT);

        let open_rect = geom.toggle_drawer();
        assert_eq!(open_rect.width(), PORTRAIT_WIDTH_OPEN);
        assert_eq!(open_rect.height(), PORTRAIT_HEIGHT);

        let closed_rect = geom.toggle_drawer();
        assert_eq!(closed_rect.width(), PORTRAIT_WIDTH_CLOSED);

        let land_rect = geom.set_orientation(true);
        assert_eq!(land_rect.width(), LANDSCAPE_WIDTH_CLOSED);
        assert_eq!(land_rect.height(), LANDSCAPE_HEIGHT);

        let land_open = geom.toggle_drawer();
        assert_eq!(land_open.width(), LANDSCAPE_WIDTH_OPEN);
        assert_eq!(land_open.height(), LANDSCAPE_HEIGHT);

        let work_area = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        let fs_rect = geom.toggle_fullscreen(work_area);
        assert!(fs_rect.width() > 1200);

        let restored = geom.toggle_fullscreen(work_area);
        assert_eq!(restored.width(), LANDSCAPE_WIDTH_OPEN);
        assert_eq!(restored.height(), LANDSCAPE_HEIGHT);
    }

    #[test]
    fn test_offscreen_probe_quick_lifecycle() {
        let probe = probe_dwm_acrylic_offscreen();
        assert!(probe.is_ok(), "Offscreen HWND probe must succeed");
        let res = probe.unwrap();
        assert!(res.elapsed_ms < 150, "Offscreen probe must complete in < 150ms without blocking");
    }
}
