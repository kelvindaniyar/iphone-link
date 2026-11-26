//! Tahap 264 — Lightweight Win32 Message-Only System Tray Host (`NativeTrayHost`) for `--native-ui`.
//! Owns a hidden `HWND_MESSAGE` window registering `Shell_NotifyIconW` (via `window_dwm::register_tray_icon`)
//! and popping up a native context menu using `TRAY_MENU_ITEMS`, with zero WebView2 dependency.

use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use crate::window_dwm::{
    register_tray_icon, unregister_tray_icon, ID_TRAY_AUDIO, ID_TRAY_CLEAR_CACHE, ID_TRAY_HIDE,
    ID_TRAY_LANDSCAPE, ID_TRAY_QUIT, ID_TRAY_SHOW, ID_TRAY_STARTUP, ID_TRAY_STEALTH,
    ID_TRAY_UPDATE, TRAY_MENU_ITEMS, WM_TRAYICON,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTrayEvent {
    Restore,
    Hide,
    ToggleStartup,
    CheckUpdates,
    ToggleLandscape,
    ToggleStealth,
    ToggleAudioPc,
    ClearCache,
    Quit,
}

static TRAY_EVENT_TX: Mutex<Option<Sender<NativeTrayEvent>>> = Mutex::new(None);
static TRAY_FLAGS: Mutex<TrayMenuFlags> = Mutex::new(TrayMenuFlags {
    startup: false,
    landscape: false,
    stealth: false,
    audio_pc: true,
});

#[derive(Clone, Copy)]
pub struct TrayMenuFlags {
    pub startup: bool,
    pub landscape: bool,
    pub stealth: bool,
    pub audio_pc: bool,
}

#[repr(C)]
struct Pt {
    x: i32,
    y: i32,
}

#[repr(C)]
struct WndClsW {
    cb_size: u32,
    style: u32,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(isize, u32, usize, isize) -> isize>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: isize,
    h_icon: isize,
    h_cursor: isize,
    hbr_background: isize,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
    h_icon_sm: isize,
}

fn to_w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const HWND_MESSAGE: isize = -3;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_CONTEXTMENU: u32 = 0x007B;
const MF_STRING: u32 = 0x0000;
const MF_CHECKED: u32 = 0x0008;
const MF_SEPARATOR: u32 = 0x0800;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_NONOTIFY: u32 = 0x0080;

unsafe fn resolve_proc(module: &str, name: &[u8]) -> Option<*mut c_void> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
        fn GetProcAddress(hModule: *mut c_void, lpProcName: *const i8) -> *mut c_void;
    }
    let mw = to_w(module);
    let hmod = GetModuleHandleW(mw.as_ptr());
    if hmod.is_null() {
        return None;
    }
    let p = GetProcAddress(hmod, name.as_ptr() as *const i8);
    if p.is_null() { None } else { Some(p) }
}

unsafe fn emit_tray_ev(ev: NativeTrayEvent) {
    if let Ok(g) = TRAY_EVENT_TX.lock() {
        if let Some(ref tx) = *g {
            let _ = tx.send(ev);
        }
    }
}

unsafe fn popup_native_tray_menu(hwnd: isize) {
    type CreatePopupMenuFn = unsafe extern "system" fn() -> isize;
    type AppendMenuWFn = unsafe extern "system" fn(isize, u32, usize, *const u16) -> i32;
    type TrackPopupMenuFn = unsafe extern "system" fn(isize, u32, i32, i32, i32, isize, *const c_void) -> i32;
    type DestroyMenuFn = unsafe extern "system" fn(isize) -> i32;
    type GetCursorPosFn = unsafe extern "system" fn(*mut Pt) -> i32;
    type SetForegroundWindowFn = unsafe extern "system" fn(isize) -> i32;

    let (Some(p_create), Some(p_append), Some(p_track), Some(p_destroy), Some(p_cursor), Some(p_fg)) = (
        resolve_proc("user32.dll", b"CreatePopupMenu\0"),
        resolve_proc("user32.dll", b"AppendMenuW\0"),
        resolve_proc("user32.dll", b"TrackPopupMenu\0"),
        resolve_proc("user32.dll", b"DestroyMenu\0"),
        resolve_proc("user32.dll", b"GetCursorPos\0"),
        resolve_proc("user32.dll", b"SetForegroundWindow\0"),
    ) else {
        return;
    };

    let create_menu: CreatePopupMenuFn = std::mem::transmute(p_create);
    let append_menu: AppendMenuWFn = std::mem::transmute(p_append);
    let track_menu: TrackPopupMenuFn = std::mem::transmute(p_track);
    let destroy_menu: DestroyMenuFn = std::mem::transmute(p_destroy);
    let get_cursor: GetCursorPosFn = std::mem::transmute(p_cursor);
    let set_fg: SetForegroundWindowFn = std::mem::transmute(p_fg);

    let hmenu = create_menu();
    if hmenu == 0 {
        return;
    }

    let flags = TRAY_FLAGS.lock().map(|f| *f).unwrap_or(TrayMenuFlags {
        startup: false,
        landscape: false,
        stealth: false,
        audio_pc: true,
    });

    let item_ids = [
        (ID_TRAY_SHOW, false),
        (ID_TRAY_HIDE, false),
        (ID_TRAY_STARTUP, flags.startup),
        (ID_TRAY_UPDATE, false),
        (ID_TRAY_LANDSCAPE, flags.landscape),
        (ID_TRAY_STEALTH, flags.stealth),
        (ID_TRAY_AUDIO, flags.audio_pc),
        (ID_TRAY_CLEAR_CACHE, false),
        (ID_TRAY_QUIT, false),
    ];

    for (idx, (cmd_id, checked)) in item_ids.iter().enumerate() {
        if idx == 2 || idx == 8 {
            let _ = append_menu(hmenu, MF_SEPARATOR, 0, std::ptr::null());
        }
        let text = TRAY_MENU_ITEMS.get(idx).copied().unwrap_or("Action");
        let w = to_w(text);
        let f = MF_STRING | if *checked { MF_CHECKED } else { 0 };
        let _ = append_menu(hmenu, f, *cmd_id, w.as_ptr());
    }

    let mut pt = Pt { x: 0, y: 0 };
    let _ = get_cursor(&mut pt);
    let _ = set_fg(hwnd);
    let chosen = track_menu(
        hmenu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    let _ = destroy_menu(hmenu);

    match chosen as usize {
        ID_TRAY_SHOW => emit_tray_ev(NativeTrayEvent::Restore),
        ID_TRAY_HIDE => emit_tray_ev(NativeTrayEvent::Hide),
        ID_TRAY_STARTUP => emit_tray_ev(NativeTrayEvent::ToggleStartup),
        ID_TRAY_UPDATE => emit_tray_ev(NativeTrayEvent::CheckUpdates),
        ID_TRAY_LANDSCAPE => emit_tray_ev(NativeTrayEvent::ToggleLandscape),
        ID_TRAY_STEALTH => emit_tray_ev(NativeTrayEvent::ToggleStealth),
        ID_TRAY_AUDIO => emit_tray_ev(NativeTrayEvent::ToggleAudioPc),
        ID_TRAY_CLEAR_CACHE => emit_tray_ev(NativeTrayEvent::ClearCache),
        ID_TRAY_QUIT => emit_tray_ev(NativeTrayEvent::Quit),
        _ => {}
    }
}

unsafe extern "system" fn tray_msg_wndproc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_TRAYICON {
        let mouse_msg = (lparam & 0xFFFF) as u32;
        if mouse_msg == WM_LBUTTONDBLCLK {
            emit_tray_ev(NativeTrayEvent::Restore);
            return 0;
        }
        if mouse_msg == WM_RBUTTONUP || mouse_msg == WM_CONTEXTMENU {
            popup_native_tray_menu(hwnd);
            return 0;
        }
    }
    if let Some(p_def) = resolve_proc("user32.dll", b"DefWindowProcW\0") {
        type DefFn = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;
        let def: DefFn = std::mem::transmute(p_def);
        return def(hwnd, msg, wparam, lparam);
    }
    0
}

pub struct NativeTrayHost {
    tray_hwnd: isize,
}

impl NativeTrayHost {
    pub fn new(tx: Sender<NativeTrayEvent>) -> Option<Self> {
        if let Ok(mut g) = TRAY_EVENT_TX.lock() {
            *g = Some(tx);
        }
        unsafe {
            let (Some(p_reg), Some(p_create), Some(p_hmod)) = (
                resolve_proc("user32.dll", b"RegisterClassExW\0"),
                resolve_proc("user32.dll", b"CreateWindowExW\0"),
                resolve_proc("kernel32.dll", b"GetModuleHandleW\0"),
            ) else {
                return None;
            };
            type RegFn = unsafe extern "system" fn(*const WndClsW) -> u16;
            type CwFn = unsafe extern "system" fn(
                u32, *const u16, *const u16, u32, i32, i32, i32, i32, isize, isize, isize, *mut c_void,
            ) -> isize;
            type GmFn = unsafe extern "system" fn(*const u16) -> isize;

            let reg: RegFn = std::mem::transmute(p_reg);
            let cw: CwFn = std::mem::transmute(p_create);
            let gm: GmFn = std::mem::transmute(p_hmod);

            let h_inst = gm(std::ptr::null());
            let cls_w = to_w("iPhoneLinkNativeTrayMsg");
            let title_w = to_w("iPhoneLinkTrayHost");
            let wc = WndClsW {
                cb_size: std::mem::size_of::<WndClsW>() as u32,
                style: 0,
                lpfn_wnd_proc: Some(tray_msg_wndproc),
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: h_inst,
                h_icon: 0,
                h_cursor: 0,
                hbr_background: 0,
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: cls_w.as_ptr(),
                h_icon_sm: 0,
            };
            let _ = reg(&wc);

            let hwnd = cw(
                0,
                cls_w.as_ptr(),
                title_w.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                0,
                h_inst,
                std::ptr::null_mut(),
            );
            if hwnd == 0 {
                return None;
            }
            if !register_tray_icon(hwnd as *mut c_void) {
                return None;
            }
            Some(Self { tray_hwnd: hwnd })
        }
    }

    pub fn update_flags(&self, flags: TrayMenuFlags) {
        if let Ok(mut g) = TRAY_FLAGS.lock() {
            *g = flags;
        }
    }
}

impl Drop for NativeTrayHost {
    fn drop(&mut self) {
        if self.tray_hwnd != 0 {
            unregister_tray_icon(self.tray_hwnd as *mut c_void);
            unsafe {
                if let Some(p_destroy) = resolve_proc("user32.dll", b"DestroyWindow\0") {
                    type DestroyFn = unsafe extern "system" fn(isize) -> i32;
                    let d: DestroyFn = std::mem::transmute(p_destroy);
                    let _ = d(self.tray_hwnd);
                }
            }
            self.tray_hwnd = 0;
        }
    }
}
