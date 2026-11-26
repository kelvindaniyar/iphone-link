//! Single-HWND Win32 + 2-Layer DirectComposition (`dcomp.dll`) Compositor.
//!
//! Layer 0 (`video_visual`, bottom): `IDXGISwapChain1` (`FLIP_SEQUENTIAL`, `ALPHA_MODE_IGNORE`)
//!   driven directly by `ID3D11VideoProcessor` (`LiveVideoHwnd::new_for_composition`), positioned
//!   at `ActiveVideoPhysicalRect` (`SetOffsetX`/`SetOffsetY` slots 4/6).
//! Layer 1 (`ui_visual`, top): `IDXGISwapChain1` (`FLIP_SEQUENTIAL`, `ALPHA_MODE_PREMULTIPLIED`)
//!   rendered by Slint's `MinimalSoftwareWindow` (`PremultipliedBgra8` with `background() = (0,0,0,0)`).
//!
//! Because both visuals live inside one parent `HWND` with `WS_EX_NOREDIRECTIONBITMAP` and Windows 11
//! Always-On Acrylic (`DwmExtendFrameIntoClientArea(-1)`, `DWMSBT_NONE = 1` + `SetWindowCompositionAttribute`
//! `WCA_ACCENT_POLICY = 19` / `ACCENT_ENABLE_ACRYLICBLURBEHIND = 4` so focus-loss `WA_INACTIVE` never turns grey),
//! transparent regions of `ui_visual` cleanly expose either the Layer 0 D3D11 video or the native Windows blur!

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use iphone_link_video::d3d11_context::{ComPtr, Guid, LiveD3d11Gpu, DXGI_FORMAT_UNKNOWN};
use iphone_link_video::video_hwnd::{create_composition_swapchain, DXGI_ALPHA_MODE_PREMULTIPLIED};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::platform::{
    Clipboard, EventLoopProxy, Key, Platform, PlatformError, PointerEventButton, WindowAdapter,
    WindowEvent,
};
use slint::{LogicalPosition, PhysicalSize, SharedString};

const IID_IDXGI_DEVICE: Guid = Guid {
    data1: 0x54ec77fa,
    data2: 0x1377,
    data3: 0x44e6,
    data4: [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
};

const IID_IDCOMPOSITION_DEVICE: Guid = Guid {
    data1: 0xc37ea93a,
    data2: 0xe7aa,
    data3: 0x450d,
    data4: [0xb1, 0x6f, 0x97, 0x46, 0xcb, 0x04, 0x07, 0xf3],
};

const IID_ID3D11_TEXTURE2D: Guid = Guid {
    data1: 0x6f15aaf2,
    data2: 0xd208,
    data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

const IID_IDXGI_DEVICE3: Guid = Guid {
    data1: 0x6007896c,
    data2: 0x3244,
    data3: 0x4afd,
    data4: [0xbf, 0x18, 0xa6, 0xd3, 0xbe, 0xda, 0x50, 0x23],
};

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;

const DWMWCP_DONOTROUND: i32 = 1;
const DWMWCP_ROUND: i32 = 2;
/// Official Windows 11 Build >= 22621 hardware thin acrylic system backdrop (`DWMSBT_TRANSIENTWINDOW = 3`).
pub const DWMSBT_TRANSIENTWINDOW: i32 = 3;
pub const DWMSBT_NONE: i32 = 1;
pub const WIN11_SYSTEMBACKDROP_MIN_BUILD: u32 = 22621;
const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_ENABLE_BLURBEHIND: i32 = 3;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: i32 = 4;
const ACCENT_FLAG_ALWAYS_ON: u32 = 2;
pub const SWCA_DARK_ACRYLIC_GRADIENT: u32 = 0x2218_1410;
pub const SWCA_LIGHT_ACRYLIC_GRADIENT: u32 = 0x20FC_F9F6;
const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;

#[repr(C)]
struct RtlOsVersionInfoW {
    dw_os_version_info_size: u32,
    dw_major_version: u32,
    dw_minor_version: u32,
    dw_build_number: u32,
    dw_platform_id: u32,
    sz_csd_version: [u16; 128],
}

/// Queries the real Windows OS build number via `ntdll.dll!RtlGetVersion` (immune to manifest shims).
pub fn query_windows_build_number() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| unsafe {
        let ntdll_w: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        let h_nt = LoadLibraryW(ntdll_w.as_ptr());
        if h_nt.is_null() {
            return WIN11_SYSTEMBACKDROP_MIN_BUILD;
        }
        let p_fn = GetProcAddress(h_nt, b"RtlGetVersion\0".as_ptr() as *const i8);
        if p_fn.is_null() {
            return WIN11_SYSTEMBACKDROP_MIN_BUILD;
        }
        type FnRtlGetVersion = unsafe extern "system" fn(*mut RtlOsVersionInfoW) -> i32;
        let rtl_get_ver: FnRtlGetVersion = std::mem::transmute(p_fn);
        let mut info = RtlOsVersionInfoW {
            dw_os_version_info_size: std::mem::size_of::<RtlOsVersionInfoW>() as u32,
            dw_major_version: 0,
            dw_minor_version: 0,
            dw_build_number: 0,
            dw_platform_id: 0,
            sz_csd_version: [0; 128],
        };
        let status = rtl_get_ver(&mut info);
        if status == 0 && info.dw_build_number > 0 {
            info.dw_build_number
        } else {
            WIN11_SYSTEMBACKDROP_MIN_BUILD
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackdropBackend {
    #[default]
    Win11NativeTransientAcrylic,
    Win10SwcaAcrylicFallback,
}

impl BackdropBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            BackdropBackend::Win11NativeTransientAcrylic => "win11_transient_acrylic",
            BackdropBackend::Win10SwcaAcrylicFallback => "win10_swca_fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackdropPolicyPlan {
    pub backend: BackdropBackend,
    pub dwm_systembackdrop_type: i32,
    pub immersive_dark_mode: i32,
    pub corner_preference: i32,
    pub border_color: u32,
    pub use_swca: bool,
    pub swca_accent_state: i32,
    pub swca_gradient_color: u32,
}

/// Pure OS-build backdrop resolver: Windows 11 (`>= 22621`) uses native hardware
/// `DWMSBT_TRANSIENTWINDOW (3)` + `WM_NCACTIVATE(1,-1)` with zero `SWCA`; Windows 10
/// (`< 22621`) uses `SWCA (4)` + light drag-guard `SWCA (3)` during `WM_ENTERSIZEMOVE`.
pub fn resolve_backdrop_policy_for_build(
    windows_build: u32,
    dark_mode: bool,
    borderless_fullscreen: bool,
    in_sizemove: bool,
) -> BackdropPolicyPlan {
    let corner_preference = if borderless_fullscreen {
        DWMWCP_DONOTROUND
    } else {
        DWMWCP_ROUND
    };
    let border_color = if borderless_fullscreen {
        DWMWA_COLOR_NONE
    } else {
        DWMWA_COLOR_DEFAULT
    };
    let swca_gradient_color = if dark_mode {
        SWCA_DARK_ACRYLIC_GRADIENT
    } else {
        SWCA_LIGHT_ACRYLIC_GRADIENT
    };
    if windows_build >= WIN11_SYSTEMBACKDROP_MIN_BUILD {
        BackdropPolicyPlan {
            backend: BackdropBackend::Win11NativeTransientAcrylic,
            dwm_systembackdrop_type: DWMSBT_TRANSIENTWINDOW,
            // Always lock DWMWA_USE_IMMERSIVE_DARK_MODE = 1 so Win11 never paints an ~90% opaque
            // milky-white luminosity sheet behind Light Mode (Slint Layer 1 provides translucent crystal milk tint).
            immersive_dark_mode: 1,
            corner_preference,
            border_color,
            use_swca: false,
            swca_accent_state: 0,
            swca_gradient_color,
        }
    } else {
        BackdropPolicyPlan {
            backend: BackdropBackend::Win10SwcaAcrylicFallback,
            dwm_systembackdrop_type: DWMSBT_NONE,
            immersive_dark_mode: 1,
            corner_preference,
            border_color,
            use_swca: true,
            swca_accent_state: if in_sizemove {
                ACCENT_ENABLE_BLURBEHIND
            } else {
                ACCENT_ENABLE_ACRYLICBLURBEHIND
            },
            swca_gradient_color,
        }
    }
}

#[repr(C)]
struct Margins {
    cx_left_width: i32,
    cx_right_width: i32,
    cy_top_height: i32,
    cy_bottom_height: i32,
}

#[repr(C)]
struct AccentPolicy {
    accent_state: i32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttribData {
    attrib: u32,
    pv_data: *mut c_void,
    cb_data: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AlwaysOnAcrylicState {
    pub backend: BackdropBackend,
    pub dwm_systembackdrop_type: i32,
    pub swca_applied: bool,
    pub always_on_active: bool,
    pub immersive_dark_mode: i32,
    pub dark_mode: bool,
    pub borderless_fullscreen: bool,
    pub accent_state: i32,
    pub gradient_color: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const i8) -> *mut c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

type FnDwmExtendFrameIntoClientArea =
    unsafe extern "system" fn(hwnd: isize, p_mar_inset: *const Margins) -> i32;
type FnDwmSetWindowAttribute = unsafe extern "system" fn(
    hwnd: isize,
    dw_attribute: u32,
    pv_attribute: *const c_void,
    cb_attribute: u32,
) -> i32;
type FnDwmGetWindowAttribute = unsafe extern "system" fn(
    hwnd: isize,
    dw_attribute: u32,
    pv_attribute: *mut c_void,
    cb_attribute: u32,
) -> i32;
type FnSetWindowCompositionAttribute =
    unsafe extern "system" fn(hwnd: isize, data: *mut WindowCompositionAttribData) -> i32;

struct DwmApiFns {
    extend_frame: Option<FnDwmExtendFrameIntoClientArea>,
    set_attr: Option<FnDwmSetWindowAttribute>,
    get_attr: Option<FnDwmGetWindowAttribute>,
    set_comp_attr: Option<FnSetWindowCompositionAttribute>,
}

unsafe impl Send for DwmApiFns {}
unsafe impl Sync for DwmApiFns {}

fn dwm_api_fns() -> &'static DwmApiFns {
    static FNS: OnceLock<DwmApiFns> = OnceLock::new();
    FNS.get_or_init(|| unsafe {
        let dwm_w: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
        let u32_w: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let h_dwm = LoadLibraryW(dwm_w.as_ptr());
        let h_u32 = LoadLibraryW(u32_w.as_ptr());
        let p_ext = if h_dwm.is_null() {
            std::ptr::null_mut()
        } else {
            GetProcAddress(h_dwm, b"DwmExtendFrameIntoClientArea\0".as_ptr() as *const i8)
        };
        let p_set = if h_dwm.is_null() {
            std::ptr::null_mut()
        } else {
            GetProcAddress(h_dwm, b"DwmSetWindowAttribute\0".as_ptr() as *const i8)
        };
        let p_get = if h_dwm.is_null() {
            std::ptr::null_mut()
        } else {
            GetProcAddress(h_dwm, b"DwmGetWindowAttribute\0".as_ptr() as *const i8)
        };
        let p_swca = if h_u32.is_null() {
            std::ptr::null_mut()
        } else {
            GetProcAddress(h_u32, b"SetWindowCompositionAttribute\0".as_ptr() as *const i8)
        };
        DwmApiFns {
            extend_frame: if p_ext.is_null() { None } else { Some(std::mem::transmute(p_ext)) },
            set_attr: if p_set.is_null() { None } else { Some(std::mem::transmute(p_set)) },
            get_attr: if p_get.is_null() { None } else { Some(std::mem::transmute(p_get)) },
            set_comp_attr: if p_swca.is_null() { None } else { Some(std::mem::transmute(p_swca)) },
        }
    })
}

type FnDCompositionCreateDevice = unsafe extern "system" fn(
    dxgi_device: *mut c_void,
    iid: *const Guid,
    dcomposition_device: *mut *mut c_void,
) -> i32;

/// Premultiplied `B8G8R8A8` pixel compatible with DXGI (`DXGI_FORMAT_B8G8R8A8_UNORM`) +
/// `DXGI_ALPHA_MODE_PREMULTIPLIED`. Notice `background()` MUST be `(0, 0, 0, 0)` so Slint's
/// software renderer clears `MainWindow { background: transparent; }` to true transparent!
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct PremultipliedBgra8 {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

impl TargetPixel for PremultipliedBgra8 {
    #[inline]
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let inv_a = (u8::MAX - color.alpha) as u16;
        let sa = self.a as u16;
        let ca = color.alpha as u16;
        let out_a = (sa + ca - (sa * ca) / 255).min(255) as u8;
        self.r = ((self.r as u16 * inv_a / 255) as u8)
            .saturating_add(color.red)
            .min(out_a);
        self.g = ((self.g as u16 * inv_a / 255) as u8)
            .saturating_add(color.green)
            .min(out_a);
        self.b = ((self.b as u16 * inv_a / 255) as u8)
            .saturating_add(color.blue)
            .min(out_a);
        self.a = out_a;
    }

    #[inline]
    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self {
            b: blue,
            g: green,
            r: red,
            a: 255,
        }
    }

    #[inline]
    fn background() -> Self {
        Self {
            b: 0,
            g: 0,
            r: 0,
            a: 0,
        }
    }
}

/// Queries the live `DWMWA_SYSTEMBACKDROP_TYPE (38)` value on `hwnd`.
pub fn query_dwm_systembackdrop_type(hwnd: isize) -> Option<i32> {
    if hwnd == 0 {
        return None;
    }
    let get_attr = dwm_api_fns().get_attr?;
    let mut val: i32 = -1;
    let hr = unsafe {
        get_attr(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &mut val as *mut i32 as *mut c_void,
            std::mem::size_of::<i32>() as u32,
        )
    };
    if hr >= 0 {
        Some(val)
    } else {
        None
    }
}

/// Applies Always-On Hardware Frosted Thin Acrylic (`DWMSBT_TRANSIENTWINDOW = 3` +
/// `WM_NCACTIVATE(1,-1)` on Windows 11 Build >= 22621; `SWCA` fallback with `WM_ENTERSIZEMOVE`
/// drag-guard on Windows 10 < 22621) while locking `DWMWA_USE_IMMERSIVE_DARK_MODE = 1`.
pub fn apply_always_on_acrylic(
    hwnd: isize,
    dark_mode: bool,
    borderless_fullscreen: bool,
) -> AlwaysOnAcrylicState {
    apply_always_on_acrylic_with_sizemove(hwnd, dark_mode, borderless_fullscreen, false)
}

fn apply_always_on_acrylic_with_sizemove(
    hwnd: isize,
    dark_mode: bool,
    borderless_fullscreen: bool,
    in_sizemove: bool,
) -> AlwaysOnAcrylicState {
    let plan = resolve_backdrop_policy_for_build(
        query_windows_build_number(),
        dark_mode,
        borderless_fullscreen,
        in_sizemove,
    );
    let mut state = AlwaysOnAcrylicState {
        backend: plan.backend,
        dwm_systembackdrop_type: plan.dwm_systembackdrop_type,
        swca_applied: false,
        always_on_active: false,
        immersive_dark_mode: plan.immersive_dark_mode,
        dark_mode,
        borderless_fullscreen,
        accent_state: plan.swca_accent_state,
        gradient_color: plan.swca_gradient_color,
    };
    if hwnd == 0 {
        return state;
    }
    let fns = dwm_api_fns();
    unsafe {
        if !in_sizemove {
            if let Some(extend_fn) = fns.extend_frame {
                let margins = Margins {
                    cx_left_width: -1,
                    cx_right_width: -1,
                    cy_top_height: -1,
                    cy_bottom_height: -1,
                };
                let _ = extend_fn(hwnd, &margins);
            }
            if let Some(set_attr) = fns.set_attr {
                let immersive_dark: i32 = plan.immersive_dark_mode;
                let _ = set_attr(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE,
                    &immersive_dark as *const i32 as *const c_void,
                    std::mem::size_of::<i32>() as u32,
                );

                let corner = plan.corner_preference;
                let _ = set_attr(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &corner as *const i32 as *const c_void,
                    std::mem::size_of::<i32>() as u32,
                );

                let backdrop = plan.dwm_systembackdrop_type;
                let hr_bd = set_attr(
                    hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &backdrop as *const i32 as *const c_void,
                    std::mem::size_of::<i32>() as u32,
                );
                if !plan.use_swca {
                    state.always_on_active = hr_bd >= 0;
                }

                let border = plan.border_color;
                let _ = set_attr(
                    hwnd,
                    DWMWA_BORDER_COLOR,
                    &border as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                );
            }
        }
        if plan.use_swca {
            if let Some(set_comp_attr) = fns.set_comp_attr {
                let mut policy = AccentPolicy {
                    accent_state: plan.swca_accent_state,
                    accent_flags: ACCENT_FLAG_ALWAYS_ON,
                    gradient_color: plan.swca_gradient_color,
                    animation_id: 0,
                };
                let mut data = WindowCompositionAttribData {
                    attrib: WCA_ACCENT_POLICY,
                    pv_data: &mut policy as *mut AccentPolicy as *mut c_void,
                    cb_data: std::mem::size_of::<AccentPolicy>(),
                };
                state.swca_applied = set_comp_attr(hwnd, &mut data) != 0;
                state.always_on_active = state.swca_applied;
            }
        }
    }
    if let Some(queried) = query_dwm_systembackdrop_type(hwnd) {
        state.dwm_systembackdrop_type = queried;
    }
    if let Some(host) = active_host() {
        if host.hwnd == hwnd {
            host.dark_mode.set(dark_mode);
            host.borderless_fullscreen.set(borderless_fullscreen);
            host.last_acrylic_state.set(state);
        }
    }
    state
}

/// Applies the Windows 11 Always-On pure transparent Acrylic backdrop recipe on a DirectComposition HWND.
pub fn apply_dcomp_win11_thin_acrylic(hwnd: isize, is_dark: bool, borderless_fullscreen: bool) {
    let _ = apply_always_on_acrylic(hwnd, is_dark, borderless_fullscreen);
}

/// Returns the active host's last applied Always-On SWCA Acrylic state.
pub fn query_active_always_on_acrylic_state() -> Option<AlwaysOnAcrylicState> {
    let host = active_host()?;
    Some(host.last_acrylic_state.get())
}

/// Manages the DirectComposition device, HWND composition target, root visual tree,
/// Layer 0 dynamic `video_visual`, and Layer 1 Slint `ui_visual` + `IDXGISwapChain1`.
pub struct DcompPresenter {
    gpu: Arc<LiveD3d11Gpu>,
    h_dcomp: *mut c_void,
    dcomp_device: Option<ComPtr>,
    target: Option<ComPtr>,
    root_visual: Option<ComPtr>,
    video_visual: Option<ComPtr>,
    ui_visual: Option<ComPtr>,
    ui_swapchain: Option<ComPtr>,
    ui_width: u32,
    ui_height: u32,
    ui_pixels: Vec<PremultipliedBgra8>,
    video_attached: bool,
    attached_video_sc: usize,
    video_offset_x: f32,
    video_offset_y: f32,
}

unsafe impl Send for DcompPresenter {}

impl DcompPresenter {
    /// Initializes `IDCompositionDevice`, `IDCompositionTarget`, root/video/ui visuals, and the
    /// Layer 1 `DXGI_ALPHA_MODE_PREMULTIPLIED` swapchain, presenting a 100% transparent frame.
    pub fn new(
        hwnd: isize,
        gpu: Arc<LiveD3d11Gpu>,
        ui_width: u32,
        ui_height: u32,
    ) -> Result<Self, String> {
        if hwnd == 0 {
            return Err("DcompPresenter::new requires a valid non-zero HWND".to_string());
        }
        let w = ui_width.max(1);
        let h = ui_height.max(1);

        let dll_w: Vec<u16> = "dcomp.dll\0".encode_utf16().collect();
        let h_dcomp = unsafe { LoadLibraryW(dll_w.as_ptr()) };
        if h_dcomp.is_null() {
            return Err("LoadLibraryW(dcomp.dll) failed".to_string());
        }

        let init_res = (|| -> Result<Self, String> {
            let p_create = unsafe {
                GetProcAddress(
                    h_dcomp,
                    b"DCompositionCreateDevice\0".as_ptr() as *const i8,
                )
            };
            if p_create.is_null() {
                return Err("GetProcAddress(DCompositionCreateDevice) failed".to_string());
            }
            let create_fn: FnDCompositionCreateDevice = unsafe { std::mem::transmute(p_create) };

            let dxgi_dev = gpu
                .device()
                .query_interface(&IID_IDXGI_DEVICE)
                .map_err(|hr| format!("QueryInterface(IDXGIDevice) failed: 0x{:08X}", hr as u32))?;

            let mut p_dcomp: *mut c_void = std::ptr::null_mut();
            let hr = unsafe {
                create_fn(
                    dxgi_dev.as_ptr(),
                    &IID_IDCOMPOSITION_DEVICE,
                    &mut p_dcomp,
                )
            };
            if hr < 0 || p_dcomp.is_null() {
                return Err(format!(
                    "DCompositionCreateDevice failed: HRESULT=0x{:08X}",
                    hr as u32
                ));
            }
            let dcomp_device = unsafe {
                ComPtr::from_raw(p_dcomp)
                    .ok_or_else(|| "null IDCompositionDevice".to_string())?
            };

            let target = Self::create_target_for_hwnd(&dcomp_device, hwnd)?;
            let root_visual = Self::create_visual(&dcomp_device)?;
            let video_visual = Self::create_visual(&dcomp_device)?;
            let ui_visual = Self::create_visual(&dcomp_device)?;

            // IDCompositionTarget::SetRoot = vtable slot 3
            unsafe {
                let vt = target.vtable();
                let set_root: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 =
                    std::mem::transmute(*vt.add(3));
                let hr_r = set_root(target.as_ptr(), root_visual.as_ptr());
                if hr_r < 0 {
                    return Err(format!(
                        "IDCompositionTarget::SetRoot failed: 0x{:08X}",
                        hr_r as u32
                    ));
                }
            }

            let ui_swapchain =
                create_composition_swapchain(&gpu, w, h, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            Self::visual_set_content(&ui_visual, ui_swapchain.as_ptr())?;
            Self::visual_add_child(&root_visual, &ui_visual, false, std::ptr::null_mut())?;

            let mut presenter = Self {
                gpu,
                h_dcomp,
                dcomp_device: Some(dcomp_device),
                target: Some(target),
                root_visual: Some(root_visual),
                video_visual: Some(video_visual),
                ui_visual: Some(ui_visual),
                ui_swapchain: Some(ui_swapchain),
                ui_width: w,
                ui_height: h,
                ui_pixels: vec![PremultipliedBgra8::background(); (w as usize) * (h as usize)],
                video_attached: false,
                attached_video_sc: 0,
                video_offset_x: f32::NAN,
                video_offset_y: f32::NAN,
            };

            presenter.present_transparent_clear()?;
            Ok(presenter)
        })();

        if init_res.is_err() && !h_dcomp.is_null() {
            unsafe {
                let _ = FreeLibrary(h_dcomp);
            }
        }
        init_res
    }

    fn create_target_for_hwnd(dcomp_dev: &ComPtr, hwnd: isize) -> Result<ComPtr, String> {
        // IDCompositionDevice::CreateTargetForHwnd = vtable slot 6
        unsafe {
            let vt = dcomp_dev.vtable();
            let create_target: unsafe extern "system" fn(
                *mut c_void,
                isize,
                i32,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(6));
            let mut p_target: *mut c_void = std::ptr::null_mut();
            let hr = create_target(dcomp_dev.as_ptr(), hwnd, 1, &mut p_target);
            if hr < 0 || p_target.is_null() {
                return Err(format!(
                    "IDCompositionDevice::CreateTargetForHwnd failed: 0x{:08X}",
                    hr as u32
                ));
            }
            ComPtr::from_raw(p_target).ok_or_else(|| "null IDCompositionTarget".to_string())
        }
    }

    fn create_visual(dcomp_dev: &ComPtr) -> Result<ComPtr, String> {
        // IDCompositionDevice::CreateVisual = vtable slot 7
        unsafe {
            let vt = dcomp_dev.vtable();
            let create_vis: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                std::mem::transmute(*vt.add(7));
            let mut p_vis: *mut c_void = std::ptr::null_mut();
            let hr = create_vis(dcomp_dev.as_ptr(), &mut p_vis);
            if hr < 0 || p_vis.is_null() {
                return Err(format!(
                    "IDCompositionDevice::CreateVisual failed: 0x{:08X}",
                    hr as u32
                ));
            }
            ComPtr::from_raw(p_vis).ok_or_else(|| "null IDCompositionVisual".to_string())
        }
    }

    fn visual_set_content(visual: &ComPtr, content: *mut c_void) -> Result<(), String> {
        // IDCompositionVisual::SetContent = vtable slot 15
        unsafe {
            let vt = visual.vtable();
            let set_content: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 =
                std::mem::transmute(*vt.add(15));
            let hr = set_content(visual.as_ptr(), content);
            if hr < 0 {
                return Err(format!(
                    "IDCompositionVisual::SetContent failed: 0x{:08X}",
                    hr as u32
                ));
            }
            Ok(())
        }
    }

    fn visual_set_offset(visual: &ComPtr, x: f32, y: f32) -> Result<(), String> {
        // CRITICAL: On IDCompositionVisual, SetOffsetX(Animation*) is slot 3 and SetOffsetX(f32) is
        // slot 4; SetOffsetY(Animation*) is slot 5 and SetOffsetY(f32) is slot 6!
        unsafe {
            let vt = visual.vtable();
            let set_x: unsafe extern "system" fn(*mut c_void, f32) -> i32 =
                std::mem::transmute(*vt.add(4));
            let set_y: unsafe extern "system" fn(*mut c_void, f32) -> i32 =
                std::mem::transmute(*vt.add(6));
            let hr_x = set_x(visual.as_ptr(), x);
            if hr_x < 0 {
                return Err(format!(
                    "IDCompositionVisual::SetOffsetX({x}) failed: 0x{:08X}",
                    hr_x as u32
                ));
            }
            let hr_y = set_y(visual.as_ptr(), y);
            if hr_y < 0 {
                return Err(format!(
                    "IDCompositionVisual::SetOffsetY({y}) failed: 0x{:08X}",
                    hr_y as u32
                ));
            }
            Ok(())
        }
    }

    fn visual_add_child(
        parent: &ComPtr,
        child: &ComPtr,
        insert_above: bool,
        reference: *mut c_void,
    ) -> Result<(), String> {
        // IDCompositionVisual::AddVisual = vtable slot 16
        unsafe {
            let vt = parent.vtable();
            let add_vis: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                i32,
                *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(16));
            let hr = add_vis(
                parent.as_ptr(),
                child.as_ptr(),
                if insert_above { 1 } else { 0 },
                reference,
            );
            if hr < 0 {
                return Err(format!(
                    "IDCompositionVisual::AddVisual failed: 0x{:08X}",
                    hr as u32
                ));
            }
            Ok(())
        }
    }

    fn visual_remove_child(parent: &ComPtr, child: &ComPtr) {
        // IDCompositionVisual::RemoveVisual = vtable slot 17
        unsafe {
            let vt = parent.vtable();
            let rem_vis: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 =
                std::mem::transmute(*vt.add(17));
            let _ = rem_vis(parent.as_ptr(), child.as_ptr());
        }
    }

    /// Commits pending DirectComposition visual tree mutations atomically.
    pub fn commit(&self) -> Result<(), String> {
        let Some(dev) = self.dcomp_device.as_ref() else {
            return Err("DCompositionDevice dropped".to_string());
        };
        // IDCompositionDevice::Commit = vtable slot 3
        unsafe {
            let vt = dev.vtable();
            let commit: unsafe extern "system" fn(*mut c_void) -> i32 =
                std::mem::transmute(*vt.add(3));
            let hr = commit(dev.as_ptr());
            if hr < 0 {
                return Err(format!(
                    "IDCompositionDevice::Commit failed: 0x{:08X}",
                    hr as u32
                ));
            }
        }
        Ok(())
    }

    /// Attaches, repositions, or detaches Layer 0 (`video_visual`) strictly BELOW Layer 1
    /// (`ui_visual`). Commits automatically whenever tree/content/offset changes.
    pub fn sync_video_layer(
        &mut self,
        video_swapchain: Option<*mut c_void>,
        offset_x: f32,
        offset_y: f32,
    ) -> Result<(), String> {
        let root = self
            .root_visual
            .as_ref()
            .ok_or_else(|| "root_visual missing".to_string())?;
        let v_vis = self
            .video_visual
            .as_ref()
            .ok_or_else(|| "video_visual missing".to_string())?;
        let ui_vis = self
            .ui_visual
            .as_ref()
            .ok_or_else(|| "ui_visual missing".to_string())?;

        let mut dirty = false;
        match video_swapchain {
            Some(sc_ptr) if !sc_ptr.is_null() => {
                let sc_addr = sc_ptr as usize;
                if self.attached_video_sc != sc_addr {
                    Self::visual_set_content(v_vis, sc_ptr)?;
                    self.attached_video_sc = sc_addr;
                    dirty = true;
                }
                if !self.video_attached {
                    // insertAbove = FALSE (0) with reference = ui_visual places Layer 0 behind Layer 1!
                    Self::visual_add_child(root, v_vis, false, ui_vis.as_ptr())?;
                    self.video_attached = true;
                    dirty = true;
                }
                if self.video_offset_x.to_bits() != offset_x.to_bits()
                    || self.video_offset_y.to_bits() != offset_y.to_bits()
                {
                    Self::visual_set_offset(v_vis, offset_x, offset_y)?;
                    self.video_offset_x = offset_x;
                    self.video_offset_y = offset_y;
                    dirty = true;
                }
            }
            _ => {
                if self.video_attached {
                    Self::visual_remove_child(root, v_vis);
                    self.video_attached = false;
                    dirty = true;
                }
                if self.attached_video_sc != 0 {
                    let _ = Self::visual_set_content(v_vis, std::ptr::null_mut());
                    self.attached_video_sc = 0;
                    dirty = true;
                }
                self.video_offset_x = f32::NAN;
                self.video_offset_y = f32::NAN;
            }
        }

        let detached = matches!(video_swapchain, None) || video_swapchain.is_some_and(|p| p.is_null());
        if dirty {
            self.commit()?;
            if detached {
                self.trim_gpu_memory();
            }
        }
        Ok(())
    }

    /// Trims idle DXGI/D3D11 memory via `IDXGIDevice3::Trim()` (vtable slot 17).
    pub fn trim_gpu_memory(&self) {
        if let Ok(dxgi3) = self.gpu.device().query_interface(&IID_IDXGI_DEVICE3) {
            unsafe {
                let vt = dxgi3.vtable();
                // IDXGIDevice3::Trim = vtable slot 17 (IUnknown 0..2, IDXGIObject 3..6, IDXGIDevice 7..11,
                // IDXGIDevice1 12..13, IDXGIDevice2 14..16, IDXGIDevice3::Trim 17)
                let trim_fn: unsafe extern "system" fn(*mut c_void) = std::mem::transmute(*vt.add(17));
                trim_fn(dxgi3.as_ptr());
            }
        }
    }

    /// Updates ONLY Layer 0 (`video_visual`) physical offsets while keeping Layer 0 swapchain
    /// attached (zero flicker during live resize).
    pub fn update_video_offset(&mut self, offset_x: f32, offset_y: f32) -> Result<(), String> {
        if !self.video_attached {
            return Ok(());
        }
        let v_vis = self
            .video_visual
            .as_ref()
            .ok_or_else(|| "video_visual missing".to_string())?;
        if self.video_offset_x.to_bits() != offset_x.to_bits()
            || self.video_offset_y.to_bits() != offset_y.to_bits()
        {
            Self::visual_set_offset(v_vis, offset_x, offset_y)?;
            self.video_offset_x = offset_x;
            self.video_offset_y = offset_y;
            self.commit()?;
        }
        Ok(())
    }

    pub fn ui_size(&self) -> (u32, u32) {
        (self.ui_width, self.ui_height)
    }

    pub fn ui_pixels(&self) -> &[PremultipliedBgra8] {
        &self.ui_pixels
    }

    pub fn ui_pixels_mut(&mut self) -> (&mut [PremultipliedBgra8], u32, u32) {
        (&mut self.ui_pixels, self.ui_width, self.ui_height)
    }

    pub fn is_video_attached(&self) -> bool {
        self.video_attached
    }

    pub fn video_offset(&self) -> (f32, f32) {
        (self.video_offset_x, self.video_offset_y)
    }

    /// Resizes the Layer 1 `ui_swapchain` and clears the CPU buffer to pure transparent `(0,0,0,0)`
    /// so the next full Slint render cleanly populates the new dimensions.
    pub fn resize_ui(&mut self, width: u32, height: u32) -> Result<(), String> {
        let nw = width.max(1);
        let nh = height.max(1);
        if nw == self.ui_width && nh == self.ui_height {
            return Ok(());
        }
        let sc = self
            .ui_swapchain
            .as_ref()
            .ok_or_else(|| "ui_swapchain missing".to_string())?;

        let prev_len = self.ui_pixels.len();
        let new_len = (nw as usize) * (nh as usize);
        self.ui_pixels
            .resize(new_len, PremultipliedBgra8::background());
        if self.ui_pixels.capacity() > new_len.saturating_add(new_len / 2) {
            self.ui_pixels.shrink_to_fit();
        }
        self.ui_pixels.fill(PremultipliedBgra8::background());

        let hr = unsafe {
            let vt = sc.vtable();
            let resize_bufs: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                u32,
                u32,
                u32,
            ) -> i32 = std::mem::transmute(*vt.add(13));
            resize_bufs(sc.as_ptr(), 0, nw, nh, DXGI_FORMAT_UNKNOWN, 0)
        };
        if hr < 0 {
            return Err(format!(
                "ui_swapchain ResizeBuffers({nw}x{nh}) failed: 0x{:08X}",
                hr as u32
            ));
        }
        self.ui_width = nw;
        self.ui_height = nh;
        if new_len < prev_len {
            self.trim_gpu_memory();
        }
        Ok(())
    }

    /// Uploads `ui_pixels` into Layer 1's DXGI backbuffer (`UpdateSubresource` vtable slot 48),
    /// calls `Present`, and commits the DirectComposition tree.
    pub fn upload_and_present_ui(&mut self, vsync: bool) -> Result<(), String> {
        let expected = (self.ui_width as usize) * (self.ui_height as usize);
        if self.ui_pixels.len() < expected || expected == 0 {
            return Ok(());
        }
        let sc = self
            .ui_swapchain
            .as_ref()
            .ok_or_else(|| "ui_swapchain missing".to_string())?;

        let backbuf = unsafe {
            let vt = sc.vtable();
            let get_buf: unsafe extern "system" fn(
                *mut c_void,
                u32,
                *const Guid,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vt.add(9));
            let mut p_bb: *mut c_void = std::ptr::null_mut();
            let hr = get_buf(sc.as_ptr(), 0, &IID_ID3D11_TEXTURE2D, &mut p_bb);
            if hr < 0 || p_bb.is_null() {
                return Err(format!(
                    "ui_swapchain GetBuffer(0) failed: 0x{:08X}",
                    hr as u32
                ));
            }
            ComPtr::from_raw(p_bb).ok_or_else(|| "null ui_swapchain backbuffer".to_string())?
        };

        let row_pitch = self.ui_width * 4;
        unsafe {
            let ctx = self.gpu.context();
            let vt = ctx.vtable();
            // ID3D11DeviceContext::UpdateSubresource = vtable slot 48
            let update_subresource: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                u32,
                *const c_void,
                *const c_void,
                u32,
                u32,
            ) = std::mem::transmute(*vt.add(48));
            update_subresource(
                ctx.as_ptr(),
                backbuf.as_ptr(),
                0,
                std::ptr::null(),
                self.ui_pixels.as_ptr() as *const c_void,
                row_pitch,
                0,
            );
        }
        drop(backbuf);

        let hr_pres = unsafe {
            let vt = sc.vtable();
            let present: unsafe extern "system" fn(*mut c_void, u32, u32) -> i32 =
                std::mem::transmute(*vt.add(8));
            present(sc.as_ptr(), if vsync { 1 } else { 0 }, 0)
        };
        if hr_pres < 0 {
            return Err(format!(
                "ui_swapchain Present failed: 0x{:08X}",
                hr_pres as u32
            ));
        }

        self.commit()
    }

    /// Presents a pure `(0,0,0,0)` transparent UI buffer before `ShowWindow` so no opaque flash
    /// is ever visible on initial window creation.
    pub fn present_transparent_clear(&mut self) -> Result<(), String> {
        self.ui_pixels.fill(PremultipliedBgra8::background());
        self.upload_and_present_ui(false)
    }
}

impl Drop for DcompPresenter {
    fn drop(&mut self) {
        if let Some(root) = self.root_visual.as_ref() {
            unsafe {
                let vt = root.vtable();
                // IDCompositionVisual::RemoveAllVisuals = vtable slot 18
                let rem_all: unsafe extern "system" fn(*mut c_void) -> i32 =
                    std::mem::transmute(*vt.add(18));
                let _ = rem_all(root.as_ptr());
            }
        }
        if let Some(vis) = self.video_visual.as_ref() {
            let _ = Self::visual_set_content(vis, std::ptr::null_mut());
        }
        if let Some(vis) = self.ui_visual.as_ref() {
            let _ = Self::visual_set_content(vis, std::ptr::null_mut());
        }
        if let Some(target) = self.target.as_ref() {
            unsafe {
                let vt = target.vtable();
                let set_root: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 =
                    std::mem::transmute(*vt.add(3));
                let _ = set_root(target.as_ptr(), std::ptr::null_mut());
            }
        }
        let _ = self.commit();

        // Deterministic release order: visuals & target & swapchains BEFORE dcomp_device BEFORE FreeLibrary!
        self.video_visual = None;
        self.ui_visual = None;
        self.root_visual = None;
        self.target = None;
        self.ui_swapchain = None;
        self.dcomp_device = None;

        if !self.h_dcomp.is_null() {
            unsafe {
                let _ = FreeLibrary(self.h_dcomp);
            }
            self.h_dcomp = std::ptr::null_mut();
        }
    }
}

// ============================================================================
// SINGLE-HWND WIN32 + SLINT SOFTWARE PLATFORM (`Win32DcompPlatform`)
// ============================================================================

const WM_APP_WAKE: u32 = 0x8001;
const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_PAINT_MSG: u32 = 0x000F;
const WM_CLOSE_MSG: u32 = 0x0010;
const WM_DESTROY_MSG: u32 = 0x0002;
const WM_SIZE_MSG: u32 = 0x0005;
const WM_ACTIVATE_MSG: u32 = 0x0006;
const WM_ERASEBKGND_MSG: u32 = 0x0014;
const WM_SETCURSOR_MSG: u32 = 0x0020;
const WM_NCCALCSIZE_MSG: u32 = 0x0083;
const WM_NCHITTEST_MSG: u32 = 0x0084;
const WM_NCACTIVATE_MSG: u32 = 0x0086;
const WM_KEYDOWN_MSG: u32 = 0x0100;
const WM_KEYUP_MSG: u32 = 0x0101;
const WM_CHAR_MSG: u32 = 0x0102;
const WM_SYSKEYDOWN_MSG: u32 = 0x0104;
const WM_SYSKEYUP_MSG: u32 = 0x0105;
const WM_SYSCHAR_MSG: u32 = 0x0106;
const WM_MOUSEMOVE_MSG: u32 = 0x0200;
const WM_LBUTTONDOWN_MSG: u32 = 0x0201;
const WM_LBUTTONUP_MSG: u32 = 0x0202;
const WM_MOUSEWHEEL_MSG: u32 = 0x020A;
const WM_MOUSEHWHEEL_MSG: u32 = 0x020E;
const WM_ENTERSIZEMOVE_MSG: u32 = 0x0231;
const WM_EXITSIZEMOVE_MSG: u32 = 0x0232;
const WM_MOUSELEAVE_MSG: u32 = 0x02A3;
const WM_THEMECHANGED_MSG: u32 = 0x031A;

const HTCLIENT_VAL: isize = 1;
const HTLEFT_VAL: isize = 10;
const HTRIGHT_VAL: isize = 11;
const HTTOP_VAL: isize = 12;
const HTTOPLEFT_VAL: isize = 13;
const HTTOPRIGHT_VAL: isize = 14;
const HTBOTTOM_VAL: isize = 15;
const HTBOTTOMLEFT_VAL: isize = 16;
const HTBOTTOMRIGHT_VAL: isize = 17;

const QS_ALLINPUT_MASK: u32 = 0x04FF;
const PM_REMOVE_FLAG: u32 = 0x0001;
const TME_LEAVE_FLAG: u32 = 0x0000_0002;
const WS_EX_NOREDIRECTIONBITMAP: u32 = 0x0020_0000;
const WS_EX_APPWINDOW: u32 = 0x0004_0000;
const WS_POPUP_FLAG: u32 = 0x8000_0000;
const WS_THICKFRAME_FLAG: u32 = 0x0004_0000;
const WS_MINIMIZEBOX_FLAG: u32 = 0x0002_0000;
const WS_SYSMENU_FLAG: u32 = 0x0008_0000;
const SW_SHOW_FLAG: i32 = 5;
const SWP_NOZORDER_FLAG: u32 = 0x0004;
const SWP_NOREDRAW_FLAG: u32 = 0x0008;
const SWP_NOACTIVATE_FLAG: u32 = 0x0010;

#[repr(C)]
#[derive(Default)]
struct MsgWin32 {
    hwnd: *mut c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
    l_private: u32,
}

#[repr(C)]
#[derive(Default)]
struct PtWin32 {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default)]
struct RectWin32 {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct TrackMouseEvtWin32 {
    cb_size: u32,
    dw_flags: u32,
    hwnd_track: *mut c_void,
    dw_hover_time: u32,
}

#[repr(C)]
struct WndClassExHost {
    cb_size: u32,
    style: u32,
    lpfn_wnd_proc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: *mut c_void,
    h_icon: *mut c_void,
    h_cursor: *mut c_void,
    h_br_background: *mut c_void,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
    h_icon_sm: *mut c_void,
}

type FnDcompDefWindowProcW = unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize;
type FnDcompRegisterClassExW = unsafe extern "system" fn(*const WndClassExHost) -> u16;
type FnDcompCreateWindowExW = unsafe extern "system" fn(
    u32,
    *const u16,
    *const u16,
    u32,
    i32,
    i32,
    i32,
    i32,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
) -> *mut c_void;
type FnDcompDestroyWindow = unsafe extern "system" fn(*mut c_void) -> i32;
type FnDcompShowWindow = unsafe extern "system" fn(*mut c_void, i32) -> i32;
type FnDcompPostMessageW = unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> i32;
type FnDcompPeekMessageW =
    unsafe extern "system" fn(*mut MsgWin32, *mut c_void, u32, u32, u32) -> i32;
type FnDcompTranslateMessage = unsafe extern "system" fn(*const MsgWin32) -> i32;
type FnDcompDispatchMessageW = unsafe extern "system" fn(*const MsgWin32) -> isize;
type FnDcompMsgWaitForMultipleObjects =
    unsafe extern "system" fn(u32, *const *mut c_void, i32, u32, u32) -> u32;
type FnDcompGetClientRect = unsafe extern "system" fn(*mut c_void, *mut RectWin32) -> i32;
type FnDcompScreenToClient = unsafe extern "system" fn(*mut c_void, *mut PtWin32) -> i32;
type FnDcompValidateRect = unsafe extern "system" fn(*mut c_void, *const RectWin32) -> i32;
type FnDcompSetCapture = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type FnDcompReleaseCapture = unsafe extern "system" fn() -> i32;
type FnDcompGetKeyState = unsafe extern "system" fn(i32) -> i16;
type FnDcompGetDpiForWindow = unsafe extern "system" fn(*mut c_void) -> u32;
type FnDcompSetProcessDpiAwarenessContext = unsafe extern "system" fn(isize) -> i32;
type FnDcompLoadCursorW = unsafe extern "system" fn(*mut c_void, *const u16) -> *mut c_void;
type FnDcompSetCursor = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type FnDcompTrackMouseEvent = unsafe extern "system" fn(*mut TrackMouseEvtWin32) -> i32;
type FnDcompSetWindowPos =
    unsafe extern "system" fn(*mut c_void, *mut c_void, i32, i32, i32, i32, u32) -> i32;
type FnDcompGetModuleHandleW = unsafe extern "system" fn(*const u16) -> *mut c_void;

struct DcompWin32Fns {
    get_module_handle_w: FnDcompGetModuleHandleW,
    def_window_proc_w: FnDcompDefWindowProcW,
    register_class_ex_w: FnDcompRegisterClassExW,
    create_window_ex_w: FnDcompCreateWindowExW,
    destroy_window: FnDcompDestroyWindow,
    show_window: FnDcompShowWindow,
    post_message_w: FnDcompPostMessageW,
    peek_message_w: FnDcompPeekMessageW,
    translate_message: FnDcompTranslateMessage,
    dispatch_message_w: FnDcompDispatchMessageW,
    msg_wait_for_multiple_objects: FnDcompMsgWaitForMultipleObjects,
    get_client_rect: FnDcompGetClientRect,
    screen_to_client: FnDcompScreenToClient,
    validate_rect: FnDcompValidateRect,
    set_capture: FnDcompSetCapture,
    release_capture: FnDcompReleaseCapture,
    get_key_state: FnDcompGetKeyState,
    get_dpi_for_window: Option<FnDcompGetDpiForWindow>,
    set_process_dpi_awareness_context: Option<FnDcompSetProcessDpiAwarenessContext>,
    load_cursor_w: FnDcompLoadCursorW,
    set_cursor: FnDcompSetCursor,
    track_mouse_event: Option<FnDcompTrackMouseEvent>,
    set_window_pos: FnDcompSetWindowPos,
}

fn dcomp_win32() -> &'static DcompWin32Fns {
    static FNS: OnceLock<DcompWin32Fns> = OnceLock::new();
    FNS.get_or_init(|| unsafe {
        let k32_w: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        let u32_w: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let hk = LoadLibraryW(k32_w.as_ptr());
        let hu = LoadLibraryW(u32_w.as_ptr());
        let gk = |n: &[u8]| GetProcAddress(hk, n.as_ptr() as *const i8);
        let gu = |n: &[u8]| GetProcAddress(hu, n.as_ptr() as *const i8);
        let opt_dpi = gu(b"GetDpiForWindow\0");
        let opt_ctx = gu(b"SetProcessDpiAwarenessContext\0");
        let opt_tme = gu(b"TrackMouseEvent\0");
        DcompWin32Fns {
            get_module_handle_w: std::mem::transmute(gk(b"GetModuleHandleW\0")),
            def_window_proc_w: std::mem::transmute(gu(b"DefWindowProcW\0")),
            register_class_ex_w: std::mem::transmute(gu(b"RegisterClassExW\0")),
            create_window_ex_w: std::mem::transmute(gu(b"CreateWindowExW\0")),
            destroy_window: std::mem::transmute(gu(b"DestroyWindow\0")),
            show_window: std::mem::transmute(gu(b"ShowWindow\0")),
            post_message_w: std::mem::transmute(gu(b"PostMessageW\0")),
            peek_message_w: std::mem::transmute(gu(b"PeekMessageW\0")),
            translate_message: std::mem::transmute(gu(b"TranslateMessage\0")),
            dispatch_message_w: std::mem::transmute(gu(b"DispatchMessageW\0")),
            msg_wait_for_multiple_objects: std::mem::transmute(gu(b"MsgWaitForMultipleObjects\0")),
            get_client_rect: std::mem::transmute(gu(b"GetClientRect\0")),
            screen_to_client: std::mem::transmute(gu(b"ScreenToClient\0")),
            validate_rect: std::mem::transmute(gu(b"ValidateRect\0")),
            set_capture: std::mem::transmute(gu(b"SetCapture\0")),
            release_capture: std::mem::transmute(gu(b"ReleaseCapture\0")),
            get_key_state: std::mem::transmute(gu(b"GetKeyState\0")),
            get_dpi_for_window: if opt_dpi.is_null() {
                None
            } else {
                Some(std::mem::transmute(opt_dpi))
            },
            set_process_dpi_awareness_context: if opt_ctx.is_null() {
                None
            } else {
                Some(std::mem::transmute(opt_ctx))
            },
            load_cursor_w: std::mem::transmute(gu(b"LoadCursorW\0")),
            set_cursor: std::mem::transmute(gu(b"SetCursor\0")),
            track_mouse_event: if opt_tme.is_null() {
                None
            } else {
                Some(std::mem::transmute(opt_tme))
            },
            set_window_pos: std::mem::transmute(gu(b"SetWindowPos\0")),
        }
    })
}

pub struct DcompHostHooks {
    pub is_fullscreen: Box<dyn Fn() -> bool>,
    pub on_nchittest: Box<dyn Fn(i32, i32, i32, i32, f32) -> isize>,
    pub on_pointer_event: Box<dyn Fn(u32, i32, i32) -> bool>,
    pub on_wheel_event: Box<dyn Fn(i32, i32, i32, bool) -> bool>,
    pub on_key_event: Box<dyn Fn(u32, usize, isize) -> bool>,
    pub on_resized: Box<dyn Fn(u32, u32, f32)>,
    pub on_exit_sizemove: Box<dyn Fn()>,
    pub on_caption_dblclick: Box<dyn Fn()>,
    pub on_close_requested: Box<dyn Fn()>,
}

struct ProxyQueueState {
    tasks: Mutex<VecDeque<Box<dyn FnOnce() + Send>>>,
    quit_requested: AtomicBool,
    wake_hwnd: AtomicIsize,
}

fn proxy_state() -> &'static Arc<ProxyQueueState> {
    static STATE: OnceLock<Arc<ProxyQueueState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Arc::new(ProxyQueueState {
            tasks: Mutex::new(VecDeque::new()),
            quit_requested: AtomicBool::new(false),
            wake_hwnd: AtomicIsize::new(0),
        })
    })
}

struct Win32DcompEventLoopProxy {
    state: Arc<ProxyQueueState>,
}

impl EventLoopProxy for Win32DcompEventLoopProxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        self.state.quit_requested.store(true, Ordering::SeqCst);
        let h = self.state.wake_hwnd.load(Ordering::Acquire);
        if h != 0 {
            unsafe {
                let _ = (dcomp_win32().post_message_w)(h as *mut c_void, WM_APP_WAKE, 0, 0);
            }
        }
        Ok(())
    }

    fn invoke_from_event_loop(
        &self,
        event: Box<dyn FnOnce() + Send>,
    ) -> Result<(), slint::EventLoopError> {
        let h = self.state.wake_hwnd.load(Ordering::Acquire);
        if cfg!(test) && h == 0 {
            return Err(slint::EventLoopError::EventLoopTerminated);
        }
        if let Ok(mut q) = self.state.tasks.lock() {
            q.push_back(event);
        }
        if h != 0 {
            unsafe {
                let _ = (dcomp_win32().post_message_w)(h as *mut c_void, WM_APP_WAKE, 0, 0);
            }
        }
        Ok(())
    }
}

struct ActiveHostState {
    hwnd: isize,
    sw_window: Rc<MinimalSoftwareWindow>,
    dcomp: RefCell<Option<DcompPresenter>>,
    hooks: RefCell<Option<DcompHostHooks>>,
    mouse_tracked: Cell<bool>,
    scale: Cell<f32>,
    dark_mode: Cell<bool>,
    borderless_fullscreen: Cell<bool>,
    last_acrylic_state: Cell<AlwaysOnAcrylicState>,
    suppress_wm_size_render: Cell<bool>,
}

thread_local! {
    static LAST_CREATED_SW_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    static ACTIVE_DCOMP_HOST: RefCell<Option<Rc<ActiveHostState>>> = const { RefCell::new(None) };
}

fn active_host() -> Option<Rc<ActiveHostState>> {
    ACTIVE_DCOMP_HOST.with(|c| c.borrow().clone())
}

fn vk_to_slint_text(vk: usize, ctrl: bool, is_key_up: bool) -> Option<SharedString> {
    match vk {
        0x08 => Some(SharedString::from(char::from(Key::Backspace))),
        0x09 => Some(SharedString::from(char::from(Key::Tab))),
        0x0D => Some(SharedString::from(char::from(Key::Return))),
        0x10 | 0xA0 | 0xA1 => Some(SharedString::from(char::from(Key::Shift))),
        0x11 | 0xA2 | 0xA3 => Some(SharedString::from(char::from(Key::Control))),
        0x12 | 0xA4 | 0xA5 => Some(SharedString::from(char::from(Key::Alt))),
        0x1B => Some(SharedString::from(char::from(Key::Escape))),
        0x23 => Some(SharedString::from(char::from(Key::End))),
        0x24 => Some(SharedString::from(char::from(Key::Home))),
        0x25 => Some(SharedString::from(char::from(Key::LeftArrow))),
        0x26 => Some(SharedString::from(char::from(Key::UpArrow))),
        0x27 => Some(SharedString::from(char::from(Key::RightArrow))),
        0x28 => Some(SharedString::from(char::from(Key::DownArrow))),
        0x2E => Some(SharedString::from(char::from(Key::Delete))),
        0x41..=0x5A if ctrl || is_key_up => {
            let lower = ((vk as u8) - b'A' + b'a') as char;
            Some(SharedString::from(lower))
        }
        0x30..=0x39 if is_key_up => {
            let d = (vk as u8) as char;
            Some(SharedString::from(d))
        }
        _ => None,
    }
}

fn dispatch_slint_key_msg(host: &ActiveHostState, msg: u32, wparam: usize, lparam: isize) {
    let fns = dcomp_win32();
    let ctrl = unsafe { ((fns.get_key_state)(0x11) as u16 & 0x8000) != 0 };
    match msg {
        WM_KEYDOWN_MSG | WM_SYSKEYDOWN_MSG => {
            if let Some(text) = vk_to_slint_text(wparam & 0xFFFF, ctrl, false) {
                host.sw_window
                    .dispatch_event(WindowEvent::KeyPressed { text });
            }
        }
        WM_KEYUP_MSG | WM_SYSKEYUP_MSG => {
            if let Some(text) = vk_to_slint_text(wparam & 0xFFFF, ctrl, true) {
                host.sw_window
                    .dispatch_event(WindowEvent::KeyReleased { text });
            }
        }
        WM_CHAR_MSG => {
            let code = (wparam & 0xFFFF) as u32;
            if let Some(ch) = char::from_u32(code) {
                if !ch.is_control() && !ctrl {
                    let mut buf = [0u8; 4];
                    let s = SharedString::from(ch.encode_utf8(&mut buf) as &str);
                    host.sw_window
                        .dispatch_event(WindowEvent::KeyPressed { text: s.clone() });
                    host.sw_window
                        .dispatch_event(WindowEvent::KeyReleased { text: s });
                }
            }
        }
        _ => {
            let _ = lparam;
        }
    }
}

fn render_host_ui_if_needed(host: &ActiveHostState, force_upload: bool) {
    let mut rendered = false;
    host.sw_window.draw_if_needed(|renderer| {
        if let Some(ref mut dc) = *host.dcomp.borrow_mut() {
            let (pixels, w, _) = dc.ui_pixels_mut();
            renderer.render(pixels, w as usize);
            rendered = true;
        }
    });
    if rendered || force_upload {
        if let Some(ref mut dc) = *host.dcomp.borrow_mut() {
            let _ = dc.upload_and_present_ui(false);
        }
    }
}

fn drain_proxy_tasks() -> bool {
    let ps = proxy_state();
    loop {
        let next = ps.tasks.lock().ok().and_then(|mut q| q.pop_front());
        let Some(task) = next else { break };
        task();
    }
    ps.quit_requested.load(Ordering::Relaxed)
}

unsafe extern "system" fn dcomp_host_wnd_proc(
    hwnd_ptr: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let fns = dcomp_win32();
    let def_proc = || (fns.def_window_proc_w)(hwnd_ptr, msg, wparam, lparam);

    let Some(host) = active_host() else {
        return def_proc();
    };
    if host.hwnd != hwnd_ptr as isize {
        return def_proc();
    }

    match msg {
        WM_NCCALCSIZE_MSG => {
            if wparam != 0 {
                return 0;
            }
            def_proc()
        }
        WM_ERASEBKGND_MSG => 1,
        WM_PAINT_MSG => {
            let _ = (fns.validate_rect)(hwnd_ptr, std::ptr::null());
            host.sw_window.request_redraw();
            0
        }
        WM_SETCURSOR_MSG => {
            let hit = (lparam & 0xFFFF) as isize;
            if hit == HTCLIENT_VAL || hit == 2 {
                let arrow = (fns.load_cursor_w)(std::ptr::null_mut(), 32512usize as *const u16);
                if !arrow.is_null() {
                    let _ = (fns.set_cursor)(arrow);
                    return 1;
                }
            }
            def_proc()
        }
        0x00A3 /* WM_NCLBUTTONDBLCLK */ => {
            if wparam == 2 {
                if let Some(ref hooks) = *host.hooks.borrow() {
                    (hooks.on_caption_dblclick)();
                }
                return 0;
            }
            def_proc()
        }
        WM_ENTERSIZEMOVE_MSG => {
            // Only drop Win10 SWCA Acrylic -> BlurBehind during resize/drag; Win11 DWMSBT_TRANSIENTWINDOW needs 0 DWM calls.
            if query_windows_build_number() < WIN11_SYSTEMBACKDROP_MIN_BUILD {
                let _ = apply_always_on_acrylic_with_sizemove(
                    host.hwnd,
                    host.dark_mode.get(),
                    host.borderless_fullscreen.get(),
                    true,
                );
            }
            0
        }
        WM_EXITSIZEMOVE_MSG => {
            let arrow = (fns.load_cursor_w)(std::ptr::null_mut(), 32512usize as *const u16);
            if !arrow.is_null() {
                let _ = (fns.set_cursor)(arrow);
            }
            if query_windows_build_number() < WIN11_SYSTEMBACKDROP_MIN_BUILD {
                let _ = apply_always_on_acrylic_with_sizemove(
                    host.hwnd,
                    host.dark_mode.get(),
                    host.borderless_fullscreen.get(),
                    false,
                );
            }
            if let Some(ref hooks) = *host.hooks.borrow() {
                (hooks.on_exit_sizemove)();
            }
            0
        }
        WM_NCACTIVATE_MSG => {
            // Force wParam = 1 (TRUE), lParam = -1 via DefWindowProcW (exact Tahap 176/259 Always-On Acrylic trick)
            // so Windows 11 DWMSBT_TRANSIENTWINDOW (3) NEVER turns solid grey on focus loss (WA_INACTIVE) with 0 flicker!
            let res = (fns.def_window_proc_w)(hwnd_ptr, msg, 1usize, -1isize);
            if query_windows_build_number() < WIN11_SYSTEMBACKDROP_MIN_BUILD {
                let _ = apply_always_on_acrylic(
                    host.hwnd,
                    host.dark_mode.get(),
                    host.borderless_fullscreen.get(),
                );
            }
            res
        }
        WM_ACTIVATE_MSG => {
            let active = (wparam & 0xFFFF) != 0;
            host.sw_window
                .dispatch_event(WindowEvent::WindowActiveChanged(active));
            if query_windows_build_number() < WIN11_SYSTEMBACKDROP_MIN_BUILD {
                let _ = apply_always_on_acrylic(
                    host.hwnd,
                    host.dark_mode.get(),
                    host.borderless_fullscreen.get(),
                );
            }
            0
        }
        WM_THEMECHANGED_MSG => {
            let _ = apply_always_on_acrylic(
                host.hwnd,
                host.dark_mode.get(),
                host.borderless_fullscreen.get(),
            );
            def_proc()
        }
        WM_NCHITTEST_MSG => {
            let sx = (lparam & 0xFFFF) as i16 as i32;
            let sy = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = PtWin32 { x: sx, y: sy };
            let _ = (fns.screen_to_client)(hwnd_ptr, &mut pt);
            let mut rc = RectWin32::default();
            let _ = (fns.get_client_rect)(hwnd_ptr, &mut rc);
            let cw = rc.right - rc.left;
            let ch = rc.bottom - rc.top;
            let scale = host.scale.get();

            if let Some(ref hooks) = *host.hooks.borrow() {
                if !(hooks.is_fullscreen)() && cw > 80 && ch > 80 {
                    let b = (5.0 * scale).round() as i32;
                    let in_top_right_btns = pt.y < (38.0 * scale).round() as i32
                        && pt.x > cw - (68.0 * scale).round() as i32;
                    let left = pt.x < b;
                    let right = pt.x >= cw - b && !in_top_right_btns;
                    let top = pt.y < b && !in_top_right_btns;
                    let bottom = pt.y >= ch - b;
                    if top && left {
                        return HTTOPLEFT_VAL;
                    }
                    if top && right {
                        return HTTOPRIGHT_VAL;
                    }
                    if bottom && left {
                        return HTBOTTOMLEFT_VAL;
                    }
                    if bottom && right {
                        return HTBOTTOMRIGHT_VAL;
                    }
                    if left {
                        return HTLEFT_VAL;
                    }
                    if right {
                        return HTRIGHT_VAL;
                    }
                    if top {
                        return HTTOP_VAL;
                    }
                    if bottom {
                        return HTBOTTOM_VAL;
                    }
                }
                return (hooks.on_nchittest)(pt.x, pt.y, cw, ch, scale);
            }
            HTCLIENT_VAL
        }
        WM_DPICHANGED_MSG => {
            let new_dpi = (wparam & 0xFFFF) as f32;
            if new_dpi >= 48.0 {
                let s = new_dpi / 96.0;
                host.scale.set(s);
                host.sw_window
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: s });
            }
            let suggested = lparam as *const RectWin32;
            if !suggested.is_null() {
                let r = &*suggested;
                let _ = (fns.set_window_pos)(
                    hwnd_ptr,
                    std::ptr::null_mut(),
                    r.left,
                    r.top,
                    (r.right - r.left).max(320),
                    (r.bottom - r.top).max(400),
                    SWP_NOZORDER_FLAG | SWP_NOACTIVATE_FLAG,
                );
            }
            let _ = apply_always_on_acrylic(
                host.hwnd,
                host.dark_mode.get(),
                host.borderless_fullscreen.get(),
            );
            0
        }
        WM_SIZE_MSG => {
            if wparam != 1 {
                let cw = (lparam & 0xFFFF) as u32;
                let ch = ((lparam >> 16) & 0xFFFF) as u32;
                if cw >= 16 && ch >= 16 {
                    if let Some(f_dpi) = fns.get_dpi_for_window {
                        let dpi = f_dpi(hwnd_ptr);
                        if dpi >= 48 {
                            let s = (dpi as f32) / 96.0;
                            if (s - host.scale.get()).abs() > 0.01 {
                                host.scale.set(s);
                                host.sw_window.dispatch_event(WindowEvent::ScaleFactorChanged {
                                    scale_factor: s,
                                });
                            }
                        }
                    }
                    let already_committed = host.suppress_wm_size_render.get()
                        && host
                            .dcomp
                            .borrow()
                            .as_ref()
                            .is_some_and(|dc| dc.ui_size() == (cw, ch));
                    if !already_committed {
                        {
                            if let Some(ref mut dc) = *host.dcomp.borrow_mut() {
                                let _ = dc.resize_ui(cw, ch);
                            }
                        }
                        host.sw_window.set_size(PhysicalSize::new(cw, ch));
                        if let Some(ref hooks) = *host.hooks.borrow() {
                            (hooks.on_resized)(cw, ch, host.scale.get());
                        }
                        host.sw_window.request_redraw();
                        render_host_ui_if_needed(&host, false);
                    }
                }
            }
            0
        }
        WM_MOUSEMOVE_MSG => {
            if !host.mouse_tracked.get() {
                if let Some(tme) = fns.track_mouse_event {
                    let mut evt = TrackMouseEvtWin32 {
                        cb_size: std::mem::size_of::<TrackMouseEvtWin32>() as u32,
                        dw_flags: TME_LEAVE_FLAG,
                        hwnd_track: hwnd_ptr,
                        dw_hover_time: 0,
                    };
                    if tme(&mut evt) != 0 {
                        host.mouse_tracked.set(true);
                    }
                }
            }
            let cx = (lparam & 0xFFFF) as i16 as i32;
            let cy = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let consumed = host
                .hooks
                .borrow()
                .as_ref()
                .map(|h| (h.on_pointer_event)(msg, cx, cy))
                .unwrap_or(false);
            if !consumed {
                let s = host.scale.get();
                host.sw_window.dispatch_event(WindowEvent::PointerMoved {
                    position: LogicalPosition::new(cx as f32 / s, cy as f32 / s),
                });
            }
            0
        }
        WM_MOUSELEAVE_MSG => {
            host.mouse_tracked.set(false);
            host.sw_window.dispatch_event(WindowEvent::PointerExited);
            0
        }
        WM_LBUTTONDOWN_MSG => {
            let cx = (lparam & 0xFFFF) as i16 as i32;
            let cy = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let _ = (fns.set_capture)(hwnd_ptr);
            let consumed = host
                .hooks
                .borrow()
                .as_ref()
                .map(|h| (h.on_pointer_event)(msg, cx, cy))
                .unwrap_or(false);
            if !consumed {
                let s = host.scale.get();
                let pos = LogicalPosition::new(cx as f32 / s, cy as f32 / s);
                host.sw_window.dispatch_event(WindowEvent::PointerPressed {
                    position: pos,
                    button: PointerEventButton::Left,
                });
            }
            0
        }
        WM_LBUTTONUP_MSG => {
            let cx = (lparam & 0xFFFF) as i16 as i32;
            let cy = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let _ = (fns.release_capture)();
            let consumed = host
                .hooks
                .borrow()
                .as_ref()
                .map(|h| (h.on_pointer_event)(msg, cx, cy))
                .unwrap_or(false);
            if !consumed {
                let s = host.scale.get();
                let pos = LogicalPosition::new(cx as f32 / s, cy as f32 / s);
                host.sw_window.dispatch_event(WindowEvent::PointerReleased {
                    position: pos,
                    button: PointerEventButton::Left,
                });
            }
            0
        }
        WM_MOUSEWHEEL_MSG | WM_MOUSEHWHEEL_MSG => {
            let sx = (lparam & 0xFFFF) as i16 as i32;
            let sy = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = PtWin32 { x: sx, y: sy };
            let _ = (fns.screen_to_client)(hwnd_ptr, &mut pt);
            let delta = ((wparam >> 16) & 0xFFFF) as i16 as i32;
            let is_horiz = msg == WM_MOUSEHWHEEL_MSG;
            let consumed = host
                .hooks
                .borrow()
                .as_ref()
                .map(|h| (h.on_wheel_event)(pt.x, pt.y, delta, is_horiz))
                .unwrap_or(false);
            if !consumed {
                let s = host.scale.get();
                let notch = (delta as f32 / 120.0) * 60.0;
                host.sw_window.dispatch_event(WindowEvent::PointerScrolled {
                    position: LogicalPosition::new(pt.x as f32 / s, pt.y as f32 / s),
                    delta_x: if is_horiz { -notch } else { 0.0 },
                    delta_y: if is_horiz { 0.0 } else { notch },
                });
            }
            0
        }
        WM_SYSCHAR_MSG => 0,
        WM_KEYDOWN_MSG | WM_SYSKEYDOWN_MSG | WM_KEYUP_MSG | WM_SYSKEYUP_MSG | WM_CHAR_MSG => {
            let consumed = host
                .hooks
                .borrow()
                .as_ref()
                .map(|h| (h.on_key_event)(msg, wparam, lparam))
                .unwrap_or(false);
            if !consumed {
                dispatch_slint_key_msg(&host, msg, wparam, lparam);
            } else if msg == WM_KEYDOWN_MSG || msg == WM_SYSKEYDOWN_MSG {
                // Keep Slint modifier tracking healthy while returning 0 to suppress Alt beep
                if matches!(wparam & 0xFFFF, 0x10..=0x12 | 0xA0..=0xA5) {
                    dispatch_slint_key_msg(&host, msg, wparam, lparam);
                }
            }
            if matches!(msg, WM_SYSKEYDOWN_MSG | WM_SYSKEYUP_MSG) && (wparam & 0xFFFF) == 0x73 {
                def_proc()
            } else {
                0
            }
        }
        WM_APP_WAKE => {
            if drain_proxy_tasks() {
                proxy_state().quit_requested.store(true, Ordering::SeqCst);
            }
            0
        }
        WM_CLOSE_MSG => {
            if let Some(ref hooks) = *host.hooks.borrow() {
                (hooks.on_close_requested)();
            } else {
                proxy_state().quit_requested.store(true, Ordering::SeqCst);
            }
            0
        }
        WM_DESTROY_MSG => {
            proxy_state().quit_requested.store(true, Ordering::SeqCst);
            0
        }
        _ => def_proc(),
    }
}

pub struct Win32DcompPlatform {
    start: Instant,
}

impl Win32DcompPlatform {
    fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Platform for Win32DcompPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let win = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        LAST_CREATED_SW_WINDOW.with(|c| {
            *c.borrow_mut() = Some(win.clone());
        });
        Ok(win)
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Win32DcompEventLoopProxy {
            state: proxy_state().clone(),
        }))
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        if matches!(clipboard, Clipboard::DefaultClipboard) {
            crate::ocr_engine::read_win32_clipboard()
        } else {
            None
        }
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        if matches!(clipboard, Clipboard::DefaultClipboard) {
            let _ = crate::ocr_engine::write_win32_clipboard(text);
        }
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let fns = dcomp_win32();
        let ps = proxy_state();
        ps.quit_requested.store(false, Ordering::SeqCst);

        loop {
            if drain_proxy_tasks() || ps.quit_requested.load(Ordering::Relaxed) {
                break;
            }

            let mut msg = MsgWin32::default();
            while unsafe { (fns.peek_message_w)(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE_FLAG) }
                != 0
            {
                unsafe {
                    let _ = (fns.translate_message)(&msg);
                    let _ = (fns.dispatch_message_w)(&msg);
                }
                if drain_proxy_tasks() || ps.quit_requested.load(Ordering::Relaxed) {
                    break;
                }
            }
            if ps.quit_requested.load(Ordering::Relaxed) {
                break;
            }

            slint::platform::update_timers_and_animations();
            if let Some(host) = active_host() {
                render_host_ui_if_needed(&host, false);
            }
            if drain_proxy_tasks() || ps.quit_requested.load(Ordering::Relaxed) {
                break;
            }

            let has_anims = active_host()
                .map(|h| h.sw_window.has_active_animations())
                .unwrap_or(false);
            let mut wait_ms = slint::platform::duration_until_next_timer_update()
                .map(|d| d.as_millis().clamp(1, 5000) as u32)
                .unwrap_or(u32::MAX);
            if has_anims {
                wait_ms = wait_ms.min(16);
            }
            unsafe {
                let _ = (fns.msg_wait_for_multiple_objects)(
                    0,
                    std::ptr::null(),
                    0,
                    wait_ms,
                    QS_ALLINPUT_MASK,
                );
            }
        }
        Ok(())
    }
}

/// Installs `Win32DcompPlatform` once per process and enables Per-Monitor DPI Awareness v2.
pub fn ensure_dcomp_platform_installed() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if let Some(set_ctx) = dcomp_win32().set_process_dpi_awareness_context {
            unsafe {
                let _ = set_ctx(-4isize); // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
            }
        }
        let _ = slint::platform::set_platform(Box::new(Win32DcompPlatform::new()));
    });
}

/// Resets `EventLoopProxy` queue/quit state before starting a Slint component loop.
pub fn reset_dcomp_loop_state() {
    let ps = proxy_state();
    ps.quit_requested.store(false, Ordering::SeqCst);
    ps.wake_hwnd.store(0, Ordering::Release);
    if let Ok(mut q) = ps.tasks.lock() {
        q.clear();
    }
}

/// Creates ONE `WS_EX_NOREDIRECTIONBITMAP` Win32 host `HWND` centered at `(x, y, phys_w, phys_h)`,
/// attaches `DcompPresenter`, renders Frame 0 into `ui_swapchain` while hidden, and ONLY THEN
/// calls `ShowWindow(SW_SHOW)` (`0 ms` white flash!).
pub fn create_single_dcomp_host(
    title: &str,
    gpu: Arc<LiveD3d11Gpu>,
    x: i32,
    y: i32,
    phys_w: u32,
    phys_h: u32,
    dark_mode: bool,
    hooks: DcompHostHooks,
) -> Result<isize, String> {
    let sw_win = LAST_CREATED_SW_WINDOW
        .with(|c| c.borrow().clone())
        .ok_or_else(|| "MinimalSoftwareWindow missing; call ui.show() first".to_string())?;
    let fns = dcomp_win32();
    let class_w: Vec<u16> = "iPhoneLinkDcompMainHost\0".encode_utf16().collect();
    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

    let hwnd_ptr = unsafe {
        let h_inst = (fns.get_module_handle_w)(std::ptr::null());
        let wcx = WndClassExHost {
            cb_size: std::mem::size_of::<WndClassExHost>() as u32,
            style: 0,
            lpfn_wnd_proc: dcomp_host_wnd_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: h_inst,
            h_icon: std::ptr::null_mut(),
            h_cursor: (fns.load_cursor_w)(std::ptr::null_mut(), 32512usize as *const u16),
            h_br_background: std::ptr::null_mut(),
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_w.as_ptr(),
            h_icon_sm: std::ptr::null_mut(),
        };
        let _ = (fns.register_class_ex_w)(&wcx);
        (fns.create_window_ex_w)(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW,
            class_w.as_ptr(),
            title_w.as_ptr(),
            WS_POPUP_FLAG | WS_THICKFRAME_FLAG | WS_MINIMIZEBOX_FLAG | WS_SYSMENU_FLAG,
            x,
            y,
            phys_w.max(320) as i32,
            phys_h.max(400) as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            h_inst,
            std::ptr::null_mut(),
        )
    };
    if hwnd_ptr.is_null() {
        return Err("CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP) failed".to_string());
    }
    let hwnd = hwnd_ptr as isize;
    let initial_acrylic = apply_always_on_acrylic(hwnd, dark_mode, false);

    let mut rc = RectWin32::default();
    unsafe {
        let _ = (fns.get_client_rect)(hwnd_ptr, &mut rc);
    }
    let cw = (rc.right - rc.left).max(16) as u32;
    let ch = (rc.bottom - rc.top).max(16) as u32;
    let scale = if let Some(f_dpi) = fns.get_dpi_for_window {
        let d = unsafe { f_dpi(hwnd_ptr) };
        if d >= 48 { (d as f32) / 96.0 } else { 1.0 }
    } else {
        1.0
    };

    let dcomp = match DcompPresenter::new(hwnd, gpu, cw, ch) {
        Ok(d) => d,
        Err(e) => {
            unsafe {
                let _ = (fns.destroy_window)(hwnd_ptr);
            }
            return Err(e);
        }
    };

    let host = Rc::new(ActiveHostState {
        hwnd,
        sw_window: sw_win.clone(),
        dcomp: RefCell::new(Some(dcomp)),
        hooks: RefCell::new(Some(hooks)),
        mouse_tracked: Cell::new(false),
        scale: Cell::new(scale),
        dark_mode: Cell::new(dark_mode),
        borderless_fullscreen: Cell::new(false),
        last_acrylic_state: Cell::new(initial_acrylic),
        suppress_wm_size_render: Cell::new(false),
    });

    ACTIVE_DCOMP_HOST.with(|c| {
        *c.borrow_mut() = Some(host.clone());
    });
    proxy_state().wake_hwnd.store(hwnd, Ordering::Release);

    sw_win.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    sw_win.set_size(PhysicalSize::new(cw, ch));
    sw_win.request_redraw();
    render_host_ui_if_needed(&host, true);

    unsafe {
        let _ = (fns.show_window)(hwnd_ptr, SW_SHOW_FLAG);
    }
    sw_win.dispatch_event(WindowEvent::WindowActiveChanged(true));
    Ok(hwnd)
}

/// Synchronizes Layer 0 (`video_visual`) swapchain attachment and offset on the active DComp host.
pub fn sync_active_dcomp_video_layer(
    video_swapchain: Option<*mut c_void>,
    offset_x: f32,
    offset_y: f32,
) {
    if let Some(host) = active_host() {
        if let Some(ref mut dc) = *host.dcomp.borrow_mut() {
            let _ = dc.sync_video_layer(video_swapchain, offset_x, offset_y);
        }
    }
}

/// Repositions Layer 0 (`video_visual`) without detaching its active DXGI swapchain.
pub fn update_active_dcomp_video_offset(offset_x: f32, offset_y: f32) {
    if let Some(host) = active_host() {
        if let Some(ref mut dc) = *host.dcomp.borrow_mut() {
            let _ = dc.update_video_offset(offset_x, offset_y);
        }
    }
}

/// Atomically resizes & commits Layer 1 (`ui_swapchain` + Slint) BEFORE invoking Win32
/// `SetWindowPos(SWP_NOREDRAW | SWP_NOZORDER | SWP_NOACTIVATE)` so expanding/shrinking
/// the Side Drawer never shows a transient torn strip.
pub fn atomic_resize_host_window(
    hwnd: isize,
    x: i32,
    y: i32,
    phys_w: u32,
    phys_h: u32,
) -> bool {
    if hwnd == 0 {
        return false;
    }
    let tw = phys_w.max(320);
    let th = phys_h.max(400);
    let fns = dcomp_win32();
    if let Some(host) = active_host() {
        if host.hwnd == hwnd {
            host.suppress_wm_size_render.set(true);
            {
                if let Some(ref mut dc) = *host.dcomp.borrow_mut() {
                    let _ = dc.resize_ui(tw, th);
                }
            }
            host.sw_window.set_size(PhysicalSize::new(tw, th));
            if let Some(ref hooks) = *host.hooks.borrow() {
                (hooks.on_resized)(tw, th, host.scale.get());
            }
            slint::platform::update_timers_and_animations();
            host.sw_window.request_redraw();
            render_host_ui_if_needed(&host, true);
            unsafe {
                let _ = (fns.set_window_pos)(
                    hwnd as *mut c_void,
                    std::ptr::null_mut(),
                    x,
                    y,
                    tw as i32,
                    th as i32,
                    SWP_NOZORDER_FLAG | SWP_NOACTIVATE_FLAG | SWP_NOREDRAW_FLAG,
                );
            }
            host.suppress_wm_size_render.set(false);
            if let Some(ref mut dc) = *host.dcomp.borrow_mut() {
                let _ = dc.commit();
            }
            return true;
        }
    }
    unsafe {
        let _ = (fns.set_window_pos)(
            hwnd as *mut c_void,
            std::ptr::null_mut(),
            x,
            y,
            tw as i32,
            th as i32,
            SWP_NOZORDER_FLAG | SWP_NOACTIVATE_FLAG,
        );
    }
    false
}

/// Asks Windows to trim unreferenced working-set pages for the current process (`-1, -1`).
pub fn trim_process_working_set() {
    unsafe {
        let k32_w: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
        let hk = LoadLibraryW(k32_w.as_ptr());
        if hk.is_null() {
            return;
        }
        let p_cur = GetProcAddress(hk, b"GetCurrentProcess\0".as_ptr() as *const i8);
        let p_trim = GetProcAddress(hk, b"SetProcessWorkingSetSize\0".as_ptr() as *const i8);
        if !p_cur.is_null() && !p_trim.is_null() {
            let get_cur: unsafe extern "system" fn() -> *mut c_void = std::mem::transmute(p_cur);
            let set_ws: unsafe extern "system" fn(*mut c_void, usize, usize) -> i32 =
                std::mem::transmute(p_trim);
            let _ = set_ws(get_cur(), usize::MAX, usize::MAX);
        }
        let _ = FreeLibrary(hk);
    }
}

/// Releases idle DXGI GPU buffers (`IDXGIDevice3::Trim`) and trims process working set.
pub fn trim_active_dcomp_memory() {
    if let Some(host) = active_host() {
        if let Ok(dc_ref) = host.dcomp.try_borrow() {
            if let Some(ref dc) = *dc_ref {
                dc.trim_gpu_memory();
            }
        }
    }
    trim_process_working_set();
}

/// Immediately updates UI pixels & commits DComp if anything in Slint became dirty.
pub fn flush_active_dcomp_ui_frame() {
    if let Some(host) = active_host() {
        slint::platform::update_timers_and_animations();
        render_host_ui_if_needed(&host, false);
    }
}

/// Immediately commits Layer 0 + Layer 1 DirectComposition visual tree after resize or layout swap.
pub fn commit_active_dcomp() {
    if let Some(host) = active_host() {
        if let Ok(mut guard) = host.dcomp.try_borrow_mut() {
            if let Some(ref mut dc) = *guard {
                let _ = dc.commit();
            }
        }
    }
}

/// Reads back raw Premultiplied BGRA8 alpha statistics from Layer 1 (`ui_pixels`) at `(x, y)`
/// for deterministic self-testing of transparent video hole alpha (`a == 0`).
pub fn sample_active_dcomp_ui_alpha(px: u32, py: u32) -> Option<u8> {
    sample_active_dcomp_ui_pixel(px, py).map(|p| p.a)
}

/// Reads back the full `PremultipliedBgra8` pixel from Layer 1 (`ui_pixels`) at `(px, py)`.
pub fn sample_active_dcomp_ui_pixel(px: u32, py: u32) -> Option<PremultipliedBgra8> {
    let host = active_host()?;
    let guard = host.dcomp.borrow();
    let dc = guard.as_ref()?;
    let (w, h) = dc.ui_size();
    if px < w && py < h {
        Some(dc.ui_pixels()[(py as usize) * (w as usize) + (px as usize)])
    } else {
        None
    }
}

/// Extracts the most recently created Slint `MinimalSoftwareWindow` for offscreen pixel unit testing.
pub fn take_last_created_software_window() -> Option<Rc<MinimalSoftwareWindow>> {
    LAST_CREATED_SW_WINDOW.with(|c| c.borrow_mut().take())
}

/// Clears any stored thread-local `MinimalSoftwareWindow` reference after unit tests complete.
pub fn clear_last_created_software_window() {
    LAST_CREATED_SW_WINDOW.with(|c| {
        *c.borrow_mut() = None;
    });
}

/// Returns active DComp host metrics `(ui_w, ui_h, video_attached)` for self-testing.
pub fn query_active_dcomp_state() -> Option<(u32, u32, bool)> {
    let host = active_host()?;
    let guard = host.dcomp.borrow();
    let dc = guard.as_ref()?;
    let (w, h) = dc.ui_size();
    Some((w, h, dc.is_video_attached()))
}

/// Destroys the active DComp host cleanly (detaches visuals BEFORE closing HWND).
pub fn shutdown_active_dcomp_host() {
    proxy_state().wake_hwnd.store(0, Ordering::Release);
    if let Some(host) = ACTIVE_DCOMP_HOST.with(|c| c.borrow_mut().take()) {
        let hwnd = host.hwnd;
        *host.hooks.borrow_mut() = None;
        *host.dcomp.borrow_mut() = None;
        if hwnd != 0 {
            unsafe {
                let _ = (dcomp_win32().destroy_window)(hwnd as *mut c_void);
            }
        }
    }
    LAST_CREATED_SW_WINDOW.with(|c| {
        *c.borrow_mut() = None;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use iphone_link_video::d3d11_context::D3d11Nv12VramRing;
    use iphone_link_video::video_hwnd::LiveVideoHwnd;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(lpModuleName: *const u16) -> *mut c_void;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        lpfn_wnd_proc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: *mut c_void,
        h_icon: *mut c_void,
        h_cursor: *mut c_void,
        h_br_background: *mut c_void,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: *mut c_void,
    }

    unsafe extern "system" fn test_wnd_proc(
        hwnd: *mut c_void,
        msg: u32,
        wp: usize,
        lp: isize,
    ) -> isize {
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    #[test]
    fn test_tahap266_dcomp_two_layer_swapchain_offsets_commit_and_clean_drop() {
        assert_eq!(
            PremultipliedBgra8::background(),
            PremultipliedBgra8 {
                b: 0,
                g: 0,
                r: 0,
                a: 0
            },
            "TargetPixel::background() MUST stay pure transparent (0,0,0,0) for DComp Layer 1"
        );

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu::new"));
        let ring = Arc::new(D3d11Nv12VramRing::new());

        let u32_w: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let h_user32 = unsafe { LoadLibraryW(u32_w.as_ptr()) };
        assert!(!h_user32.is_null(), "LoadLibraryW(user32.dll)");
        type FnRegClassExW = unsafe extern "system" fn(*const WndClassExW) -> u16;
        type FnCreateWinExW = unsafe extern "system" fn(
            u32,
            *const u16,
            *const u16,
            u32,
            i32,
            i32,
            i32,
            i32,
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
        ) -> *mut c_void;
        type FnDestroyWin = unsafe extern "system" fn(*mut c_void) -> i32;

        let class_w: Vec<u16> = "iPhoneLinkDcompUnitHost\0".encode_utf16().collect();
        let title_w: Vec<u16> = "\0".encode_utf16().collect();
        let hwnd_ptr = unsafe {
            let reg_class: FnRegClassExW = std::mem::transmute(GetProcAddress(
                h_user32,
                b"RegisterClassExW\0".as_ptr() as *const i8,
            ));
            let create_win: FnCreateWinExW = std::mem::transmute(GetProcAddress(
                h_user32,
                b"CreateWindowExW\0".as_ptr() as *const i8,
            ));
            let h_inst = GetModuleHandleW(std::ptr::null());
            let wcx = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: test_wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: h_inst,
                h_icon: std::ptr::null_mut(),
                h_cursor: std::ptr::null_mut(),
                h_br_background: std::ptr::null_mut(),
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: class_w.as_ptr(),
                h_icon_sm: std::ptr::null_mut(),
            };
            let _ = reg_class(&wcx);
            // WS_EX_NOREDIRECTIONBITMAP = 0x0020_0000, WS_POPUP = 0x8000_0000
            create_win(
                0x0020_0000,
                class_w.as_ptr(),
                title_w.as_ptr(),
                0x8000_0000,
                -32000,
                -32000,
                706,
                832,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                h_inst,
                std::ptr::null_mut(),
            )
        };
        let hwnd = hwnd_ptr as isize;
        assert_ne!(hwnd, 0, "CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP) must succeed");

        apply_dcomp_win11_thin_acrylic(hwnd, true, false);

        {
            let mut dcomp = DcompPresenter::new(hwnd, gpu.clone(), 706, 832)
                .expect("DcompPresenter::new must succeed");
            assert_eq!(dcomp.ui_size(), (706, 832));

            let mut video = LiveVideoHwnd::new_for_composition(gpu.clone(), ring, 519, 1125)
                .expect("LiveVideoHwnd::new_for_composition must succeed");
            assert_eq!(video.hwnd(), 0);
            assert!(!video.swapchain_ptr().is_null());

            let stats = video.present_latest().expect("video present_latest must succeed");
            assert_eq!(stats.presented_total, 1);

            dcomp
                .sync_video_layer(Some(video.swapchain_ptr()), 120.0, 57.0)
                .expect("sync_video_layer attach & SetOffsetX/Y(4/6) must succeed");
            assert!(dcomp.is_video_attached());
            assert_eq!(dcomp.video_offset(), (120.0, 57.0));

            dcomp.resize_ui(1059, 1248).expect("resize_ui must succeed");
            assert_eq!(dcomp.ui_size(), (1059, 1248));
            dcomp
                .present_transparent_clear()
                .expect("present_transparent_clear must commit cleanly");

            dcomp
                .sync_video_layer(None, 0.0, 0.0)
                .expect("sync_video_layer detach must succeed");
            assert!(!dcomp.is_video_attached());

            drop(dcomp);
            drop(video);
        }

        unsafe {
            let destroy_win: FnDestroyWin = std::mem::transmute(GetProcAddress(
                h_user32,
                b"DestroyWindow\0".as_ptr() as *const i8,
            ));
            let _ = destroy_win(hwnd_ptr);
            let _ = FreeLibrary(h_user32);
        }
    }

    #[test]
    fn test_tahap268_always_on_acrylic_uses_dwmsbt_none_1_and_swca_survives_wa_inactive() {
        let u32_w: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let h_user32 = unsafe { LoadLibraryW(u32_w.as_ptr()) };
        assert!(!h_user32.is_null(), "LoadLibraryW(user32.dll)");
        type FnRegClassExW = unsafe extern "system" fn(*const WndClassExW) -> u16;
        type FnCreateWinExW = unsafe extern "system" fn(
            u32,
            *const u16,
            *const u16,
            u32,
            i32,
            i32,
            i32,
            i32,
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
        ) -> *mut c_void;
        type FnSendMsgW = unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize;
        type FnDestroyWin = unsafe extern "system" fn(*mut c_void) -> i32;

        let class_w: Vec<u16> = "iPhoneLinkAcrylicTestHost268\0".encode_utf16().collect();
        let title_w: Vec<u16> = "\0".encode_utf16().collect();
        let hwnd_ptr = unsafe {
            let reg_class: FnRegClassExW = std::mem::transmute(GetProcAddress(
                h_user32,
                b"RegisterClassExW\0".as_ptr() as *const i8,
            ));
            let create_win: FnCreateWinExW = std::mem::transmute(GetProcAddress(
                h_user32,
                b"CreateWindowExW\0".as_ptr() as *const i8,
            ));
            let h_inst = GetModuleHandleW(std::ptr::null());
            let wcx = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: test_wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: h_inst,
                h_icon: std::ptr::null_mut(),
                h_cursor: std::ptr::null_mut(),
                h_br_background: std::ptr::null_mut(),
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: class_w.as_ptr(),
                h_icon_sm: std::ptr::null_mut(),
            };
            let _ = reg_class(&wcx);
            create_win(
                0x0020_0000, // WS_EX_NOREDIRECTIONBITMAP
                class_w.as_ptr(),
                title_w.as_ptr(),
                0x8000_0000, // WS_POPUP
                -32000,
                -32000,
                346,
                832,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                h_inst,
                std::ptr::null_mut(),
            )
        };
        let hwnd = hwnd_ptr as isize;
        assert_ne!(hwnd, 0);

        // Superseded by Tahap 287: On Windows 11 >= 22621, apply_always_on_acrylic selects native
        // hardware DWMSBT_TRANSIENTWINDOW (3) + DefWindowProcW(WM_NCACTIVATE, 1, -1) with zero SWCA
        // (and falls back cleanly to DWMSBT_NONE=1 + SWCA on Windows 10 < 22621).
        let dark_st = apply_always_on_acrylic(hwnd, true, false);
        assert!(dark_st.always_on_active, "Always-on acrylic backdrop must be active");
        let build = query_windows_build_number();
        let expected_backdrop = if build >= WIN11_SYSTEMBACKDROP_MIN_BUILD {
            DWMSBT_TRANSIENTWINDOW
        } else {
            DWMSBT_NONE
        };
        assert_eq!(dark_st.dwm_systembackdrop_type, expected_backdrop);
        assert_eq!(query_dwm_systembackdrop_type(hwnd), Some(expected_backdrop));
        let win10_plan = resolve_backdrop_policy_for_build(19045, true, false, false);
        assert_eq!(win10_plan.dwm_systembackdrop_type, DWMSBT_NONE);
        assert_eq!(win10_plan.swca_accent_state, ACCENT_ENABLE_ACRYLICBLURBEHIND);
        assert_eq!(win10_plan.swca_gradient_color, SWCA_DARK_ACRYLIC_GRADIENT);

        // Simulate WA_INACTIVE focus loss + WM_NCACTIVATE(0) and verify Always-On Acrylic survives intact
        unsafe {
            let send_msg: FnSendMsgW = std::mem::transmute(GetProcAddress(
                h_user32,
                b"SendMessageW\0".as_ptr() as *const i8,
            ));
            let _ = send_msg(hwnd_ptr, WM_NCACTIVATE_MSG, 0, 0);
            let _ = send_msg(hwnd_ptr, WM_ACTIVATE_MSG, 0, 0);
        }
        let inactive_reassert = apply_always_on_acrylic(hwnd, true, false);
        assert!(inactive_reassert.always_on_active);
        assert_eq!(query_dwm_systembackdrop_type(hwnd), Some(expected_backdrop));

        // Light mode & Borderless Fullscreen states preserve immersive dark = 1 & expected backdrop
        let light_st = apply_always_on_acrylic(hwnd, false, false);
        assert!(light_st.always_on_active);
        assert_eq!(light_st.immersive_dark_mode, 1);
        assert_eq!(light_st.gradient_color, SWCA_LIGHT_ACRYLIC_GRADIENT);
        let fs_st = apply_always_on_acrylic(hwnd, true, true);
        assert!(fs_st.always_on_active);
        assert_eq!(fs_st.dwm_systembackdrop_type, expected_backdrop);

        unsafe {
            let destroy_win: FnDestroyWin = std::mem::transmute(GetProcAddress(
                h_user32,
                b"DestroyWindow\0".as_ptr() as *const i8,
            ));
            let _ = destroy_win(hwnd_ptr);
            let _ = FreeLibrary(h_user32);
        }
    }

    #[test]
    fn test_tahap287_backdrop_policy_win11_transient_acrylic_vs_win10_swca_drag_guard() {
        // Windows 11 Build 22621 & 26340 (Dark, Light, Fullscreen, Drag): DWMSBT_TRANSIENTWINDOW=3, zero SWCA, dark=1
        for &build in &[WIN11_SYSTEMBACKDROP_MIN_BUILD, 26340u32] {
            for &dark in &[true, false] {
                for &fs in &[false, true] {
                    for &drag in &[false, true] {
                        let p = resolve_backdrop_policy_for_build(build, dark, fs, drag);
                        assert_eq!(p.backend, BackdropBackend::Win11NativeTransientAcrylic);
                        assert_eq!(p.dwm_systembackdrop_type, DWMSBT_TRANSIENTWINDOW);
                        assert!(!p.use_swca, "SWCA must NOT run on Win11 >= 22621 DComp HWND");
                        assert_eq!(p.immersive_dark_mode, 1);
                        assert_eq!(p.corner_preference, if fs { DWMWCP_DONOTROUND } else { DWMWCP_ROUND });
                        assert_eq!(p.border_color, if fs { DWMWA_COLOR_NONE } else { DWMWA_COLOR_DEFAULT });
                    }
                }
            }
        }
        // Windows 10 Build 19045 fallback: idle SWCA=4, in_sizemove=true drops to BLURBEHIND=3 (anti-drag-lag)
        let w10_idle = resolve_backdrop_policy_for_build(19045, true, false, false);
        assert_eq!(w10_idle.backend, BackdropBackend::Win10SwcaAcrylicFallback);
        assert_eq!(w10_idle.dwm_systembackdrop_type, DWMSBT_NONE);
        assert!(w10_idle.use_swca);
        assert_eq!(w10_idle.swca_accent_state, ACCENT_ENABLE_ACRYLICBLURBEHIND);
        let w10_drag = resolve_backdrop_policy_for_build(19045, true, false, true);
        assert_eq!(w10_drag.swca_accent_state, ACCENT_ENABLE_BLURBEHIND);
        assert!(query_windows_build_number() > 0);
    }

    #[test]
    fn test_tahap287_live_hwnd_dwmsbt_transientwindow_3_survives_wa_inactive_and_fullscreen_toggle() {
        let u32_w: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let h_user32 = unsafe { LoadLibraryW(u32_w.as_ptr()) };
        assert!(!h_user32.is_null());
        type FnRegClassExW = unsafe extern "system" fn(*const WndClassExW) -> u16;
        type FnCreateWinExW = unsafe extern "system" fn(
            u32, *const u16, *const u16, u32, i32, i32, i32, i32,
            *mut c_void, *mut c_void, *mut c_void, *mut c_void,
        ) -> *mut c_void;
        type FnSendMsgW = unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize;
        type FnDestroyWin = unsafe extern "system" fn(*mut c_void) -> i32;

        let class_w: Vec<u16> = "iPhoneLinkAcrylicTestHost287\0".encode_utf16().collect();
        let title_w: Vec<u16> = "\0".encode_utf16().collect();
        let hwnd_ptr = unsafe {
            let reg_class: FnRegClassExW = std::mem::transmute(GetProcAddress(
                h_user32, b"RegisterClassExW\0".as_ptr() as *const i8,
            ));
            let create_win: FnCreateWinExW = std::mem::transmute(GetProcAddress(
                h_user32, b"CreateWindowExW\0".as_ptr() as *const i8,
            ));
            let h_inst = GetModuleHandleW(std::ptr::null());
            let wcx = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: test_wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: h_inst,
                h_icon: std::ptr::null_mut(),
                h_cursor: std::ptr::null_mut(),
                h_br_background: std::ptr::null_mut(),
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: class_w.as_ptr(),
                h_icon_sm: std::ptr::null_mut(),
            };
            let _ = reg_class(&wcx);
            create_win(
                WS_EX_NOREDIRECTIONBITMAP,
                class_w.as_ptr(),
                title_w.as_ptr(),
                WS_POPUP_FLAG,
                -32000, -32000, 346, 832,
                std::ptr::null_mut(), std::ptr::null_mut(), h_inst, std::ptr::null_mut(),
            )
        };
        let hwnd = hwnd_ptr as isize;
        assert_ne!(hwnd, 0);

        let build = query_windows_build_number();
        let st_dark = apply_always_on_acrylic(hwnd, true, false);
        assert!(st_dark.always_on_active);
        if build >= WIN11_SYSTEMBACKDROP_MIN_BUILD {
            assert_eq!(st_dark.backend, BackdropBackend::Win11NativeTransientAcrylic);
            assert_eq!(st_dark.dwm_systembackdrop_type, DWMSBT_TRANSIENTWINDOW);
            assert!(!st_dark.swca_applied);
            assert_eq!(query_dwm_systembackdrop_type(hwnd), Some(DWMSBT_TRANSIENTWINDOW));
        }

        // Simulate WA_INACTIVE + WM_NCACTIVATE(0) + Light Mode + Fullscreen transitions
        unsafe {
            let send_msg: FnSendMsgW = std::mem::transmute(GetProcAddress(
                h_user32, b"SendMessageW\0".as_ptr() as *const i8,
            ));
            let _ = send_msg(hwnd_ptr, WM_NCACTIVATE_MSG, 0, 0);
            let _ = send_msg(hwnd_ptr, WM_ACTIVATE_MSG, 0, 0);
        }
        let st_light = apply_always_on_acrylic(hwnd, false, false);
        let st_fs = apply_always_on_acrylic(hwnd, false, true);
        let st_restored = apply_always_on_acrylic(hwnd, true, false);
        for st in &[st_light, st_fs, st_restored] {
            assert!(st.always_on_active);
            assert_eq!(st.immersive_dark_mode, 1);
            if build >= WIN11_SYSTEMBACKDROP_MIN_BUILD {
                assert_eq!(st.dwm_systembackdrop_type, DWMSBT_TRANSIENTWINDOW);
                assert_eq!(query_dwm_systembackdrop_type(hwnd), Some(DWMSBT_TRANSIENTWINDOW));
            }
        }
        println!(
            "[TAHAP287 LIVE HWND] build={} backend={} dwmsbt={:?} always_on={} dark_imm={}",
            build,
            st_restored.backend.as_str(),
            query_dwm_systembackdrop_type(hwnd),
            st_restored.always_on_active,
            st_restored.immersive_dark_mode
        );

        unsafe {
            let destroy_win: FnDestroyWin = std::mem::transmute(GetProcAddress(
                h_user32, b"DestroyWindow\0".as_ptr() as *const i8,
            ));
            let _ = destroy_win(hwnd_ptr);
            let _ = FreeLibrary(h_user32);
        }
    }

    #[test]
    fn test_tahap287_premultiplied_bgra8_blend_invariant_no_halo_and_true_alpha_hole() {
        assert_eq!(PremultipliedBgra8::background(), PremultipliedBgra8 { b: 0, g: 0, r: 0, a: 0 });
        let premul = |r: u8, g: u8, b: u8, a: u8| -> PremultipliedRgbaColor {
            PremultipliedRgbaColor {
                red: ((u16::from(r) * u16::from(a) + 127) / 255) as u8,
                green: ((u16::from(g) * u16::from(a) + 127) / 255) as u8,
                blue: ((u16::from(b) * u16::from(a) + 127) / 255) as u8,
                alpha: a,
            }
        };

        // 1. Test 200+ semi-transparent combinations for strict premultiplied invariant r,g,b <= a
        let alphas = [0u8, 16, 20, 56, 72, 89, 104, 110, 125, 191, 255];
        for &a1 in &alphas {
            for &a2 in &alphas {
                for &(r, g, b) in &[(255, 255, 255), (12, 16, 26), (244, 247, 252), (56, 189, 248)] {
                    let mut px = PremultipliedBgra8::background();
                    px.blend(premul(r, g, b, a1));
                    assert!(px.r <= px.a && px.g <= px.a && px.b <= px.a, "px={px:?}");
                    px.blend(premul(255 - r, 128, b, a2));
                    assert!(px.r <= px.a && px.g <= px.a && px.b <= px.a, "px={px:?}");
                }
            }
        }

        // 2. Verify Dark Smoky & Light Milky 3-layer Drawer + Card + Accordion stacking stays < 230
        let mut dark_3layer = PremultipliedBgra8::background();
        dark_3layer.blend(premul(0x0E, 0x13, 0x1F, 0x48)); // surface-chrome Dark (72)
        dark_3layer.blend(premul(0xFF, 0xFF, 0xFF, 0x14)); // DrawerCard Dark (20)
        dark_3layer.blend(premul(0xFF, 0xFF, 0xFF, 0x10)); // DrawerAccordion Dark (16)
        assert!(dark_3layer.a > 60 && dark_3layer.a < 130, "dark_3layer.a={}", dark_3layer.a);

        let mut light_3layer = PremultipliedBgra8::background();
        light_3layer.blend(premul(0xF5, 0xF8, 0xFD, 0x68)); // surface-chrome Light (104)
        light_3layer.blend(premul(0xFF, 0xFF, 0xFF, 0x7D)); // DrawerCard Light (125)
        light_3layer.blend(premul(0xFF, 0xFF, 0xFF, 0x59)); // DrawerAccordion Light (89)
        assert!(light_3layer.a > 160 && light_3layer.a < 230, "light_3layer.a={}", light_3layer.a);
    }
}
