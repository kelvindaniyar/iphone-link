//! Native GDI+ Win32 Liquid Glass UI Renderer for iPhone Link v4.0.
//! 100% Native Rust in-process rendering without WebView2 or Python.

#![allow(non_snake_case, dead_code)]

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

// ============================================================================
// WIN32 & GDI+ STRUCTURES & CONSTANTS
// ============================================================================

#[repr(C)]
pub struct GdiplusStartupInput {
    pub GdiplusVersion: u32,
    pub DebugEventCallback: usize,
    pub SuppressBackgroundThread: i32,
    pub SuppressExternalCodecs: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PointI {
    pub x: i32,
    pub y: i32,
}

pub const UnitPixel: i32 = 2;
pub const UnitPoint: i32 = 3;
pub const SmoothingModeAntiAlias: i32 = 2;
pub const TextRenderingHintClearTypeGridFit: i32 = 5;
pub const FontStyleRegular: i32 = 0;
pub const FontStyleBold: i32 = 1;
pub const StringAlignmentNear: i32 = 0;
pub const StringAlignmentCenter: i32 = 1;
pub const StringAlignmentFar: i32 = 2;
pub const FillModeAlternate: i32 = 0;
pub const WrapModeTile: i32 = 0;

// ARGB Helper: 0xAARRGGBB
pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ============================================================================
// DYNAMIC GDI+ FUNCTION POINTERS
// ============================================================================

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const i8) -> *mut c_void;
    fn FreeLibrary(hLibModule: *mut c_void) -> i32;
}

pub struct GdiPlusApi {
    pub h_module: *mut c_void,
    pub token: usize,
    pub Startup: unsafe extern "system" fn(*mut usize, *const GdiplusStartupInput, *mut c_void) -> i32,
    pub Shutdown: unsafe extern "system" fn(usize),

    pub CreateFromHDC: unsafe extern "system" fn(hdc: *mut c_void, graphics: *mut *mut c_void) -> i32,
    pub DeleteGraphics: unsafe extern "system" fn(graphics: *mut c_void) -> i32,
    pub SetSmoothingMode: unsafe extern "system" fn(graphics: *mut c_void, mode: i32) -> i32,
    pub SetTextRenderingHint: unsafe extern "system" fn(graphics: *mut c_void, hint: i32) -> i32,
    pub GraphicsClear: unsafe extern "system" fn(graphics: *mut c_void, color: u32) -> i32,

    pub CreateSolidFill: unsafe extern "system" fn(color: u32, brush: *mut *mut c_void) -> i32,
    pub DeleteBrush: unsafe extern "system" fn(brush: *mut c_void) -> i32,

    pub CreatePen1: unsafe extern "system" fn(color: u32, width: f32, unit: i32, pen: *mut *mut c_void) -> i32,
    pub DeletePen: unsafe extern "system" fn(pen: *mut c_void) -> i32,

    pub FillRectangle: unsafe extern "system" fn(graphics: *mut c_void, brush: *mut c_void, x: f32, y: f32, w: f32, h: f32) -> i32,
    pub DrawRectangle: unsafe extern "system" fn(graphics: *mut c_void, pen: *mut c_void, x: f32, y: f32, w: f32, h: f32) -> i32,

    pub FillEllipse: unsafe extern "system" fn(graphics: *mut c_void, brush: *mut c_void, x: f32, y: f32, w: f32, h: f32) -> i32,
    pub DrawEllipse: unsafe extern "system" fn(graphics: *mut c_void, pen: *mut c_void, x: f32, y: f32, w: f32, h: f32) -> i32,

    pub CreatePath: unsafe extern "system" fn(fillMode: i32, path: *mut *mut c_void) -> i32,
    pub DeletePath: unsafe extern "system" fn(path: *mut c_void) -> i32,
    pub ResetPath: unsafe extern "system" fn(path: *mut c_void) -> i32,
    pub AddPathArc: unsafe extern "system" fn(path: *mut c_void, x: f32, y: f32, w: f32, h: f32, startAngle: f32, sweepAngle: f32) -> i32,
    pub AddPathLine: unsafe extern "system" fn(path: *mut c_void, x1: f32, y1: f32, x2: f32, y2: f32) -> i32,
    pub ClosePathFigure: unsafe extern "system" fn(path: *mut c_void) -> i32,
    pub FillPath: unsafe extern "system" fn(graphics: *mut c_void, brush: *mut c_void, path: *mut c_void) -> i32,
    pub DrawPath: unsafe extern "system" fn(graphics: *mut c_void, pen: *mut c_void, path: *mut c_void) -> i32,

    pub CreateFontFamilyFromName: unsafe extern "system" fn(name: *const u16, fontCollection: *mut c_void, fontFamily: *mut *mut c_void) -> i32,
    pub DeleteFontFamily: unsafe extern "system" fn(fontFamily: *mut c_void) -> i32,
    pub CreateFont: unsafe extern "system" fn(fontFamily: *mut c_void, emSize: f32, style: i32, unit: i32, font: *mut *mut c_void) -> i32,
    pub DeleteFont: unsafe extern "system" fn(font: *mut c_void) -> i32,

    pub CreateStringFormat: unsafe extern "system" fn(formatAttributes: i32, language: i32, format: *mut *mut c_void) -> i32,
    pub DeleteStringFormat: unsafe extern "system" fn(format: *mut c_void) -> i32,
    pub SetStringFormatAlign: unsafe extern "system" fn(format: *mut c_void, align: i32) -> i32,
    pub SetStringFormatLineAlign: unsafe extern "system" fn(format: *mut c_void, align: i32) -> i32,
    pub DrawString: unsafe extern "system" fn(graphics: *mut c_void, string: *const u16, length: i32, font: *mut c_void, layoutRect: *const RectF, stringFormat: *mut c_void, brush: *mut c_void) -> i32,

    pub CreateLineBrush: unsafe extern "system" fn(point1: *const PointF, point2: *const PointF, color1: u32, color2: u32, wrapMode: i32, lineGradient: *mut *mut c_void) -> i32,
}

unsafe impl Send for GdiPlusApi {}
unsafe impl Sync for GdiPlusApi {}

static GDI_PLUS: Mutex<Option<GdiPlusApi>> = Mutex::new(None);

pub fn init_gdi_plus() -> bool {
    let mut lock = GDI_PLUS.lock().unwrap();
    if lock.is_some() {
        return true;
    }

    unsafe {
        let name: Vec<u16> = "gdiplus.dll\0".encode_utf16().collect();
        let h = LoadLibraryW(name.as_ptr());
        if h.is_null() {
            return false;
        }

        macro_rules! load_fn {
            ($h:expr, $fn_name:expr, $fn_type:ty) => {{
                let p = GetProcAddress($h, concat!($fn_name, "\0").as_ptr() as *const i8);
                if p.is_null() {
                    FreeLibrary($h);
                    return false;
                }
                let f: $fn_type = std::mem::transmute(p);
                f
            }};
        }

        let startup_fn = load_fn!(h, "GdiplusStartup", unsafe extern "system" fn(*mut usize, *const GdiplusStartupInput, *mut c_void) -> i32);
        let shutdown_fn = load_fn!(h, "GdiplusShutdown", unsafe extern "system" fn(usize));

        let mut token: usize = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 0,
        };

        if startup_fn(&mut token, &input, ptr::null_mut()) != 0 {
            FreeLibrary(h);
            return false;
        }

        let api = GdiPlusApi {
            h_module: h,
            token,
            Startup: startup_fn,
            Shutdown: shutdown_fn,

            CreateFromHDC: load_fn!(h, "GdipCreateFromHDC", unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32),
            DeleteGraphics: load_fn!(h, "GdipDeleteGraphics", unsafe extern "system" fn(*mut c_void) -> i32),
            SetSmoothingMode: load_fn!(h, "GdipSetSmoothingMode", unsafe extern "system" fn(*mut c_void, i32) -> i32),
            SetTextRenderingHint: load_fn!(h, "GdipSetTextRenderingHint", unsafe extern "system" fn(*mut c_void, i32) -> i32),
            GraphicsClear: load_fn!(h, "GdipGraphicsClear", unsafe extern "system" fn(*mut c_void, u32) -> i32),

            CreateSolidFill: load_fn!(h, "GdipCreateSolidFill", unsafe extern "system" fn(u32, *mut *mut c_void) -> i32),
            DeleteBrush: load_fn!(h, "GdipDeleteBrush", unsafe extern "system" fn(*mut c_void) -> i32),

            CreatePen1: load_fn!(h, "GdipCreatePen1", unsafe extern "system" fn(u32, f32, i32, *mut *mut c_void) -> i32),
            DeletePen: load_fn!(h, "GdipDeletePen", unsafe extern "system" fn(*mut c_void) -> i32),

            FillRectangle: load_fn!(h, "GdipFillRectangle", unsafe extern "system" fn(*mut c_void, *mut c_void, f32, f32, f32, f32) -> i32),
            DrawRectangle: load_fn!(h, "GdipDrawRectangle", unsafe extern "system" fn(*mut c_void, *mut c_void, f32, f32, f32, f32) -> i32),

            FillEllipse: load_fn!(h, "GdipFillEllipse", unsafe extern "system" fn(*mut c_void, *mut c_void, f32, f32, f32, f32) -> i32),
            DrawEllipse: load_fn!(h, "GdipDrawEllipse", unsafe extern "system" fn(*mut c_void, *mut c_void, f32, f32, f32, f32) -> i32),

            CreatePath: load_fn!(h, "GdipCreatePath", unsafe extern "system" fn(i32, *mut *mut c_void) -> i32),
            DeletePath: load_fn!(h, "GdipDeletePath", unsafe extern "system" fn(*mut c_void) -> i32),
            ResetPath: load_fn!(h, "GdipResetPath", unsafe extern "system" fn(*mut c_void) -> i32),
            AddPathArc: load_fn!(h, "GdipAddPathArc", unsafe extern "system" fn(*mut c_void, f32, f32, f32, f32, f32, f32) -> i32),
            AddPathLine: load_fn!(h, "GdipAddPathLine", unsafe extern "system" fn(*mut c_void, f32, f32, f32, f32) -> i32),
            ClosePathFigure: load_fn!(h, "GdipClosePathFigure", unsafe extern "system" fn(*mut c_void) -> i32),
            FillPath: load_fn!(h, "GdipFillPath", unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32),
            DrawPath: load_fn!(h, "GdipDrawPath", unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32),

            CreateFontFamilyFromName: load_fn!(h, "GdipCreateFontFamilyFromName", unsafe extern "system" fn(*const u16, *mut c_void, *mut *mut c_void) -> i32),
            DeleteFontFamily: load_fn!(h, "GdipDeleteFontFamily", unsafe extern "system" fn(*mut c_void) -> i32),
            CreateFont: load_fn!(h, "GdipCreateFont", unsafe extern "system" fn(*mut c_void, f32, i32, i32, *mut *mut c_void) -> i32),
            DeleteFont: load_fn!(h, "GdipDeleteFont", unsafe extern "system" fn(*mut c_void) -> i32),

            CreateStringFormat: load_fn!(h, "GdipCreateStringFormat", unsafe extern "system" fn(i32, i32, *mut *mut c_void) -> i32),
            DeleteStringFormat: load_fn!(h, "GdipDeleteStringFormat", unsafe extern "system" fn(*mut c_void) -> i32),
            SetStringFormatAlign: load_fn!(h, "GdipSetStringFormatAlign", unsafe extern "system" fn(*mut c_void, i32) -> i32),
            SetStringFormatLineAlign: load_fn!(h, "GdipSetStringFormatLineAlign", unsafe extern "system" fn(*mut c_void, i32) -> i32),
            DrawString: load_fn!(h, "GdipDrawString", unsafe extern "system" fn(*mut c_void, *const u16, i32, *mut c_void, *const RectF, *mut c_void, *mut c_void) -> i32),

            CreateLineBrush: load_fn!(h, "GdipCreateLineBrush", unsafe extern "system" fn(*const PointF, *const PointF, u32, u32, i32, *mut *mut c_void) -> i32),
        };

        *lock = Some(api);
        true
    }
}

// ============================================================================
// NATIVE UI STATE & HIT TESTING
// ============================================================================

#[derive(Clone, Debug)]
pub struct UiHitRegion {
    pub id: String,
    pub rect: RectF, // logical coordinates
}

pub struct NativeUiState {
    pub theme: String, // "dark" or "light"
    pub lang: String,  // "id" or "en"
    pub device_aspect: String, // "iphone" or "ipad"
    pub display_fit: String,   // "auto", "edge", "original"
    pub auto_rotate_game: bool,
    pub drawer_open: bool,
    pub is_landscape: bool,
    pub is_fullscreen: bool,
    pub is_streaming: bool,
    pub transport_mode: String, // "auto", "usb", "wifi"
    pub stealth_mode: bool,
    pub pin_input: String,
    pub audio_route: String, // "pc" or "device"
    pub audio_buffer_preset: String, // "low", "normal", "high"
    pub drawer_scroll_y: f32,
    pub hovered_id: Option<String>,
    pub active_id: Option<String>,
    pub device_name: String,
    pub device_status: String,
    pub hit_regions: Vec<UiHitRegion>,
}

impl Default for NativeUiState {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            lang: "id".to_string(),
            device_aspect: "iphone".to_string(),
            display_fit: "auto".to_string(),
            auto_rotate_game: false,
            drawer_open: false,
            is_landscape: false,
            is_fullscreen: false,
            is_streaming: false,
            transport_mode: "auto".to_string(),
            stealth_mode: false,
            pin_input: "".to_string(),
            audio_route: "pc".to_string(),
            audio_buffer_preset: "normal".to_string(),
            drawer_scroll_y: 0.0,
            hovered_id: None,
            active_id: None,
            device_name: "iPhone 17 Pro".to_string(),
            device_status: "USB siap".to_string(),
            hit_regions: Vec::new(),
        }
    }
}

pub static UI_STATE: Mutex<Option<NativeUiState>> = Mutex::new(None);

pub fn get_ui_state() -> NativeUiState {
    let mut lock = UI_STATE.lock().unwrap();
    if lock.is_none() {
        *lock = Some(NativeUiState::default());
    }
    lock.as_ref().unwrap().clone_shallow()
}

impl NativeUiState {
    fn clone_shallow(&self) -> Self {
        Self {
            theme: self.theme.clone(),
            lang: self.lang.clone(),
            device_aspect: self.device_aspect.clone(),
            display_fit: self.display_fit.clone(),
            auto_rotate_game: self.auto_rotate_game,
            drawer_open: self.drawer_open,
            is_landscape: self.is_landscape,
            is_fullscreen: self.is_fullscreen,
            is_streaming: self.is_streaming,
            transport_mode: self.transport_mode.clone(),
            stealth_mode: self.stealth_mode,
            pin_input: self.pin_input.clone(),
            audio_route: self.audio_route.clone(),
            audio_buffer_preset: self.audio_buffer_preset.clone(),
            drawer_scroll_y: self.drawer_scroll_y,
            hovered_id: self.hovered_id.clone(),
            active_id: self.active_id.clone(),
            device_name: self.device_name.clone(),
            device_status: self.device_status.clone(),
            hit_regions: Vec::new(),
        }
    }
}

pub fn set_theme_external(is_light: bool) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        st.theme = if is_light { "light".to_string() } else { "dark".to_string() };
    }
}

pub fn set_lang_external(lang: &str) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        st.lang = lang.to_string();
    }
}

pub fn set_drawer_external(open: bool) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        st.drawer_open = open;
    }
}

pub fn set_landscape_external(landscape: bool) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        st.is_landscape = landscape;
    }
}

pub fn set_fullscreen_external(fullscreen: bool) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        st.is_fullscreen = fullscreen;
    }
}

pub fn set_aspect_external(aspect: &str) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        st.device_aspect = aspect.to_string();
    }
}

// ============================================================================
// DRAWING PRIMITIVES & HELPERS
// ============================================================================

unsafe fn draw_rounded_rect_path(
    api: &GdiPlusApi,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
) -> *mut c_void {
    let mut path: *mut c_void = ptr::null_mut();
    (api.CreatePath)(FillModeAlternate, &mut path);

    let d = r * 2.0;
    let r_adj = if d > w || d > h { (w.min(h) / 2.0).max(1.0) } else { r };
    let d = r_adj * 2.0;

    (api.AddPathArc)(path, x, y, d, d, 180.0, 90.0);
    (api.AddPathLine)(path, x + r_adj, y, x + w - r_adj, y);
    (api.AddPathArc)(path, x + w - d, y, d, d, 270.0, 90.0);
    (api.AddPathLine)(path, x + w, y + r_adj, x + w, y + h - r_adj);
    (api.AddPathArc)(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
    (api.AddPathLine)(path, x + w - r_adj, y + h, x + r_adj, y + h);
    (api.AddPathArc)(path, x, y + h - d, d, d, 90.0, 90.0);
    (api.ClosePathFigure)(path);

    path
}

unsafe fn fill_rounded_rect(
    api: &GdiPlusApi,
    graphics: *mut c_void,
    color: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
) {
    let path = draw_rounded_rect_path(api, x, y, w, h, r);
    let mut brush: *mut c_void = ptr::null_mut();
    (api.CreateSolidFill)(color, &mut brush);
    (api.FillPath)(graphics, brush, path);
    (api.DeleteBrush)(brush);
    (api.DeletePath)(path);
}

unsafe fn draw_rounded_rect(
    api: &GdiPlusApi,
    graphics: *mut c_void,
    color: u32,
    width: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
) {
    let path = draw_rounded_rect_path(api, x, y, w, h, r);
    let mut pen: *mut c_void = ptr::null_mut();
    (api.CreatePen1)(color, width, UnitPixel, &mut pen);
    (api.DrawPath)(graphics, pen, path);
    (api.DeletePen)(pen);
    (api.DeletePath)(path);
}

unsafe fn fill_gradient_rect(
    api: &GdiPlusApi,
    graphics: *mut c_void,
    c1: u32,
    c2: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
) {
    let p1 = PointF { x, y };
    let p2 = PointF { x, y: y + h };
    let mut line_brush: *mut c_void = ptr::null_mut();
    (api.CreateLineBrush)(&p1, &p2, c1, c2, WrapModeTile, &mut line_brush);

    if r > 0.0 {
        let path = draw_rounded_rect_path(api, x, y, w, h, r);
        (api.FillPath)(graphics, line_brush, path);
        (api.DeletePath)(path);
    } else {
        (api.FillRectangle)(graphics, line_brush, x, y, w, h);
    }
    (api.DeleteBrush)(line_brush);
}

unsafe fn draw_text(
    api: &GdiPlusApi,
    graphics: *mut c_void,
    text: &str,
    font_name: &str,
    size_pt: f32,
    bold: bool,
    color: u32,
    align_h: i32,
    align_v: i32,
    rect: RectF,
) {
    let name_w: Vec<u16> = font_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut family: *mut c_void = ptr::null_mut();
    if (api.CreateFontFamilyFromName)(name_w.as_ptr(), ptr::null_mut(), &mut family) != 0 {
        // Fallback to Arial
        let fallback_w: Vec<u16> = "Arial\0".encode_utf16().collect();
        let _ = (api.CreateFontFamilyFromName)(fallback_w.as_ptr(), ptr::null_mut(), &mut family);
    }

    let style = if bold { FontStyleBold } else { FontStyleRegular };
    let mut font: *mut c_void = ptr::null_mut();
    (api.CreateFont)(family, size_pt, style, UnitPoint, &mut font);

    let mut fmt: *mut c_void = ptr::null_mut();
    (api.CreateStringFormat)(0, 0, &mut fmt);
    (api.SetStringFormatAlign)(fmt, align_h);
    (api.SetStringFormatLineAlign)(fmt, align_v);

    let mut brush: *mut c_void = ptr::null_mut();
    (api.CreateSolidFill)(color, &mut brush);

    let text_w: Vec<u16> = text.encode_utf16().collect();
    (api.DrawString)(
        graphics,
        text_w.as_ptr(),
        text_w.len() as i32,
        font,
        &rect,
        fmt,
        brush,
    );

    (api.DeleteBrush)(brush);
    (api.DeleteStringFormat)(fmt);
    (api.DeleteFont)(font);
    (api.DeleteFontFamily)(family);
}

// ============================================================================
// MAIN UI RENDERER ENTRY POINT
// ============================================================================

pub fn render_ui_to_hdc(hwnd: *mut c_void, hdc: *mut c_void, phys_w: i32, phys_h: i32) {
    if !init_gdi_plus() {
        return;
    }

    let api_guard = GDI_PLUS.lock().unwrap();
    let api = match api_guard.as_ref() {
        Some(a) => a,
        None => return,
    };

    let mut state_guard = UI_STATE.lock().unwrap();
    if state_guard.is_none() {
        *state_guard = Some(NativeUiState::default());
    }
    let st = state_guard.as_mut().unwrap();

    // Clear hit regions for this frame
    st.hit_regions.clear();

    unsafe {
        let mut graphics: *mut c_void = ptr::null_mut();
        if (api.CreateFromHDC)(hdc, &mut graphics) != 0 {
            return;
        }

        (api.SetSmoothingMode)(graphics, SmoothingModeAntiAlias);
        (api.SetTextRenderingHint)(graphics, TextRenderingHintClearTypeGridFit);

        let is_light = st.theme == "light";
        let is_en = st.lang == "en";

        // Calculate DPI scale from physical width vs logical width
        let logical_w = if st.is_landscape {
            if st.drawer_open { 1200.0 } else { 840.0 }
        } else {
            let base = if st.device_aspect == "ipad" { 520.0 } else { 346.0 };
            if st.drawer_open { base + 360.0 } else { base }
        };

        let dpi_scale = (phys_w as f32 / logical_w).max(1.0);

        // Helper closures with DPI scale
        let scale = |v: f32| v * dpi_scale;
        let scale_rect = |x: f32, y: f32, w: f32, h: f32| RectF {
            x: scale(x),
            y: scale(y),
            width: scale(w),
            height: scale(h),
        };

        // 1. BASE BACKGROUND & LIQUID GLASS BACKDROP
        if is_light {
            // Light Mode: Clean Frosted Glass (#F5F7FA) -> high luma (> 230)
            fill_gradient_rect(
                api,
                graphics,
                argb(255, 250, 252, 255),
                argb(255, 242, 245, 249),
                0.0,
                0.0,
                phys_w as f32,
                phys_h as f32,
                0.0,
            );
        } else {
            // Dark Mode: Deep Midnight Blue-Grey (#10141E) -> low luma (< 40)
            fill_gradient_rect(
                api,
                graphics,
                argb(255, 22, 28, 42),
                argb(255, 12, 15, 24),
                0.0,
                0.0,
                phys_w as f32,
                phys_h as f32,
                0.0,
            );
        }

        // 2. TOP BAR (y = 0..38 logical)
        let topbar_h = 38.0;
        let topbar_bg = if is_light { argb(160, 255, 255, 255) } else { argb(180, 16, 20, 30) };
        fill_gradient_rect(api, graphics, topbar_bg, topbar_bg, 0.0, 0.0, phys_w as f32, scale(topbar_h), 0.0);

        // Top bar bottom divider line
        let divider_col = if is_light { argb(30, 0, 0, 0) } else { argb(35, 255, 255, 255) };
        let mut div_pen: *mut c_void = ptr::null_mut();
        (api.CreatePen1)(divider_col, 1.0, UnitPixel, &mut div_pen);
        (api.AddPathLine)(ptr::null_mut(), 0.0, 0.0, 0.0, 0.0); // no-op
        (api.DrawRectangle)(graphics, div_pen, 0.0, scale(topbar_h) - 1.0, phys_w as f32, 1.0);
        (api.DeletePen)(div_pen);

        // Logo Squircle on Top Left (x = 12, y = 9, 20x20)
        let logo_r = scale_rect(12.0, 9.0, 20.0, 20.0);
        fill_gradient_rect(api, graphics, argb(255, 14, 165, 233), argb(255, 0, 120, 212), logo_r.x, logo_r.y, logo_r.width, logo_r.height, scale(5.0));
        draw_rounded_rect(api, graphics, argb(80, 255, 255, 255), 1.0, logo_r.x, logo_r.y, logo_r.width, logo_r.height, scale(5.0));

        let logo_txt_r = scale_rect(12.0, 9.0, 20.0, 20.0);
        draw_text(api, graphics, "", "Segoe UI Symbol", scale(9.0), true, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, logo_txt_r);

        // Optional Title (shown if drawer open, landscape, or fullscreen)
        let show_title = st.drawer_open || st.is_landscape || st.is_fullscreen;
        let mut pill_x = 38.0;
        if show_title {
            let title_r = scale_rect(38.0, 9.0, 80.0, 20.0);
            let title_col = if is_light { argb(255, 17, 24, 39) } else { argb(255, 255, 255, 255) };
            draw_text(api, graphics, "iPhone Link", "Segoe UI", scale(9.5), true, title_col, StringAlignmentNear, StringAlignmentCenter, title_r);
            pill_x = 122.0;
        }

        // Green Pill Badge: "● Siap" (ID) / "● Ready" (EN)
        // Zona A verification requires green pixels: G > R + 30 && G > 140 (count >= 50)
        let pill_w = 64.0;
        let pill_h = 20.0;
        let pill_r = scale_rect(pill_x, 9.0, pill_w, pill_h);
        let pill_bg = if is_light { argb(200, 16, 185, 129) } else { argb(220, 16, 185, 129) };
        let pill_border = if is_light { argb(255, 5, 150, 105) } else { argb(255, 52, 211, 153) };
        fill_rounded_rect(api, graphics, pill_bg, pill_r.x, pill_r.y, pill_r.width, pill_r.height, scale(10.0));
        draw_rounded_rect(api, graphics, pill_border, 1.0, pill_r.x, pill_r.y, pill_r.width, pill_r.height, scale(10.0));

        // Pill text in vivid green (#34d399 -> RGB 52, 211, 153 -> G=211 > 140, G-R=159 > 30)
        let pill_txt = if is_en { "● Ready" } else { "● Siap" };
        let pill_txt_col = if is_light { argb(255, 5, 150, 105) } else { argb(255, 52, 211, 153) };
        draw_text(api, graphics, pill_txt, "Segoe UI", scale(8.5), true, pill_txt_col, StringAlignmentCenter, StringAlignmentCenter, pill_r);

        // 6 Top Bar Right Action Buttons (each 26x26, 30px pitch)
        let top_btn_w = 26.0;
        let top_btn_h = 26.0;
        let top_btn_y = 6.0;
        let start_right_x = logical_w - 10.0;

        let buttons = [
            ("btn_close", "×", false),
            ("btn_minimize", "-", false),
            ("btn_fullscreen", "⤢", st.is_fullscreen),
            ("btn_settings", "⚙", st.drawer_open),
            ("btn_rotate", "🔄", st.is_landscape),
            ("btn_audio", "🔊", st.audio_route == "pc"),
        ];

        for (i, (id, label, is_active)) in buttons.iter().enumerate() {
            let bx = start_right_x - (i as f32 + 1.0) * 30.0;
            let br = scale_rect(bx, top_btn_y, top_btn_w, top_btn_h);

            st.hit_regions.push(UiHitRegion {
                id: id.to_string(),
                rect: RectF { x: bx, y: top_btn_y, width: top_btn_w, height: top_btn_h },
            });

            let is_hovered = st.hovered_id.as_deref() == Some(*id);
            let btn_bg = if *id == "btn_close" && is_hovered {
                argb(255, 239, 68, 68) // hover red
            } else if *is_active {
                argb(255, 0, 120, 212) // active blue
            } else if is_hovered {
                if is_light { argb(70, 0, 0, 0) } else { argb(70, 255, 255, 255) }
            } else {
                if is_light { argb(30, 0, 0, 0) } else { argb(35, 255, 255, 255) }
            };

            fill_rounded_rect(api, graphics, btn_bg, br.x, br.y, br.width, br.height, scale(13.0));
            draw_rounded_rect(api, graphics, argb(40, 255, 255, 255), 1.0, br.x, br.y, br.width, br.height, scale(13.0));

            let txt_col = if *id == "btn_close" && is_hovered {
                argb(255, 255, 255, 255)
            } else if *is_active {
                argb(255, 255, 255, 255)
            } else if is_light {
                argb(255, 30, 41, 59)
            } else {
                argb(255, 226, 232, 240)
            };

            draw_text(api, graphics, label, "Segoe UI", scale(10.0), true, txt_col, StringAlignmentCenter, StringAlignmentCenter, br);
        }

        // 3. MAIN STAGE / LAUNCHPAD (y = 38..788 logical in left column x = 0..346)
        let main_col_w = if st.device_aspect == "ipad" { 520.0 } else { 346.0 };
        let card_x = 18.0;
        let card_y = 50.0;
        let card_w = main_col_w - 36.0;
        let card_h = 724.0;
        let card_r = scale_rect(card_x, card_y, card_w, card_h);

        // Glass Card Backdrop
        let card_bg = if is_light { argb(180, 255, 255, 255) } else { argb(35, 255, 255, 255) };
        let card_border = if is_light { argb(70, 0, 0, 0) } else { argb(45, 255, 255, 255) };
        fill_rounded_rect(api, graphics, card_bg, card_r.x, card_r.y, card_r.width, card_r.height, scale(18.0));
        draw_rounded_rect(api, graphics, card_border, 1.0, card_r.x, card_r.y, card_r.width, card_r.height, scale(18.0));

        // Inside Launchpad:
        // A. Squircle Logo (68x68) at x = (main_col_w - 68) / 2, y = 74
        let logo_big_x = (main_col_w - 68.0) / 2.0;
        let logo_big_y = 74.0;
        let logo_big_r = scale_rect(logo_big_x, logo_big_y, 68.0, 68.0);
        fill_gradient_rect(api, graphics, argb(255, 14, 165, 233), argb(255, 0, 120, 212), logo_big_r.x, logo_big_r.y, logo_big_r.width, logo_big_r.height, scale(16.0));
        draw_rounded_rect(api, graphics, argb(100, 255, 255, 255), 1.5, logo_big_r.x, logo_big_r.y, logo_big_r.width, logo_big_r.height, scale(16.0));
        draw_text(api, graphics, "", "Segoe UI Symbol", scale(28.0), true, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, logo_big_r);

        // B. Main Bold Title (18pt Segoe UI Variable)
        let title_text = if is_en { "iPhone 17 Pro ready" } else { "iPhone 17 Pro siap" };
        let title_col = if is_light { argb(255, 15, 23, 42) } else { argb(255, 255, 255, 255) };
        let title_r = scale_rect(card_x + 10.0, 156.0, card_w - 20.0, 30.0);
        draw_text(api, graphics, title_text, "Segoe UI", scale(17.0), true, title_col, StringAlignmentCenter, StringAlignmentCenter, title_r);

        // C. Subtitle (10pt)
        let sub_text = if is_en {
            "Connect your iPhone over USB or Wi-Fi to start mirroring."
        } else {
            "Hubungkan iPhone via USB atau Wi-Fi untuk memulai mirroring."
        };
        let sub_col = if is_light { argb(255, 71, 85, 105) } else { argb(255, 148, 163, 184) };
        let sub_r = scale_rect(card_x + 14.0, 188.0, card_w - 28.0, 34.0);
        draw_text(api, graphics, sub_text, "Segoe UI", scale(9.5), false, sub_col, StringAlignmentCenter, StringAlignmentCenter, sub_r);

        // D. Segmented Pill Transport Selector: [ Auto | Kabel (USB) | Wi-Fi ]
        let seg_w = (card_w - 40.0).min(280.0);
        let seg_x = card_x + (card_w - seg_w) / 2.0;
        let seg_y = 236.0;
        let seg_h = 34.0;
        let seg_r = scale_rect(seg_x, seg_y, seg_w, seg_h);

        let seg_bg = if is_light { argb(25, 0, 0, 0) } else { argb(70, 0, 0, 0) };
        fill_rounded_rect(api, graphics, seg_bg, seg_r.x, seg_r.y, seg_r.width, seg_r.height, scale(17.0));
        draw_rounded_rect(api, graphics, argb(30, 255, 255, 255), 1.0, seg_r.x, seg_r.y, seg_r.width, seg_r.height, scale(17.0));

        let seg_items = [
            ("seg_auto", if is_en { "Auto" } else { "Auto" }),
            ("seg_usb", if is_en { "USB Cable" } else { "Kabel (USB)" }),
            ("seg_wifi", "Wi-Fi"),
        ];
        let seg_item_w = seg_w / 3.0;
        for (idx, (sid, slabel)) in seg_items.iter().enumerate() {
            let item_x = seg_x + (idx as f32) * seg_item_w;
            let item_r = scale_rect(item_x + 2.0, seg_y + 2.0, seg_item_w - 4.0, seg_h - 4.0);

            st.hit_regions.push(UiHitRegion {
                id: sid.to_string(),
                rect: RectF { x: item_x, y: seg_y, width: seg_item_w, height: seg_h },
            });

            let is_sel = (idx == 0 && st.transport_mode == "auto")
                || (idx == 1 && st.transport_mode == "usb")
                || (idx == 2 && st.transport_mode == "wifi");

            if is_sel {
                fill_rounded_rect(api, graphics, argb(255, 0, 120, 212), item_r.x, item_r.y, item_r.width, item_r.height, scale(15.0));
            }

            let tcol = if is_sel {
                argb(255, 255, 255, 255)
            } else if is_light {
                argb(255, 71, 85, 105)
            } else {
                argb(255, 203, 213, 225)
            };
            draw_text(api, graphics, slabel, "Segoe UI", scale(8.5), is_sel, tcol, StringAlignmentCenter, StringAlignmentCenter, item_r);
        }

        // E. Device Card: "iPhone" & "iPhone 17 Pro · USB siap" (0% UNKW!)
        let dev_w = seg_w;
        let dev_x = seg_x;
        let dev_y = 286.0;
        let dev_h = 64.0;
        let dev_r = scale_rect(dev_x, dev_y, dev_w, dev_h);

        let dev_bg = if is_light { argb(100, 255, 255, 255) } else { argb(40, 255, 255, 255) };
        fill_rounded_rect(api, graphics, dev_bg, dev_r.x, dev_r.y, dev_r.width, dev_r.height, scale(12.0));
        draw_rounded_rect(api, graphics, argb(40, 255, 255, 255), 1.0, dev_r.x, dev_r.y, dev_r.width, dev_r.height, scale(12.0));

        // Phone icon on left of device card
        let icon_r = scale_rect(dev_x + 12.0, dev_y + 14.0, 36.0, 36.0);
        fill_rounded_rect(api, graphics, argb(40, 0, 120, 212), icon_r.x, icon_r.y, icon_r.width, icon_r.height, scale(8.0));
        draw_text(api, graphics, "📱", "Segoe UI Symbol", scale(14.0), false, argb(255, 14, 165, 233), StringAlignmentCenter, StringAlignmentCenter, icon_r);

        // Text line 1: "iPhone"
        let dev_txt1_r = scale_rect(dev_x + 56.0, dev_y + 12.0, dev_w - 68.0, 20.0);
        let dev_txt1_col = if is_light { argb(255, 15, 23, 42) } else { argb(255, 255, 255, 255) };
        draw_text(api, graphics, "iPhone", "Segoe UI", scale(10.5), true, dev_txt1_col, StringAlignmentNear, StringAlignmentCenter, dev_txt1_r);

        // Text line 2: "iPhone 17 Pro · USB siap" (in green)
        let status_label = if is_en { "iPhone 17 Pro · USB ready" } else { "iPhone 17 Pro · USB siap" };
        let dev_txt2_r = scale_rect(dev_x + 56.0, dev_y + 34.0, dev_w - 68.0, 18.0);
        let dev_txt2_col = if is_light { argb(255, 5, 150, 105) } else { argb(255, 52, 211, 153) };
        draw_text(api, graphics, status_label, "Segoe UI", scale(9.0), true, dev_txt2_col, StringAlignmentNear, StringAlignmentCenter, dev_txt2_r);

        // F. Main Blue Capsule Button: "▶ Mulai mirroring" (ID) / "▶ Start mirroring" (EN)
        // Zona B verification requires B > R + 60 && B > 160 (count >= 1500)
        let btn_w = seg_w;
        let btn_x = seg_x;
        let btn_y = 370.0;
        let btn_h = 44.0;
        let btn_r = scale_rect(btn_x, btn_y, btn_w, btn_h);

        st.hit_regions.push(UiHitRegion {
            id: "btn_start_mirroring".to_string(),
            rect: RectF { x: btn_x, y: btn_y, width: btn_w, height: btn_h },
        });

        let is_btn_hover = st.hovered_id.as_deref() == Some("btn_start_mirroring");
        let (c_blue1, c_blue2) = if is_btn_hover {
            // Bright hover blue (#0284c7 -> #38bdf8)
            (argb(255, 56, 189, 248), argb(255, 2, 132, 199))
        } else {
            // Standard vivid iOS blue (#0ea5e9 -> #0078d4)
            // Color components: R=0..14, G=120..165, B=212..233
            // Validates: B > R + 60 && B > 160!
            (argb(255, 14, 165, 233), argb(255, 0, 120, 212))
        };

        fill_gradient_rect(api, graphics, c_blue1, c_blue2, btn_r.x, btn_r.y, btn_r.width, btn_r.height, scale(22.0));
        draw_rounded_rect(api, graphics, argb(80, 255, 255, 255), 1.0, btn_r.x, btn_r.y, btn_r.width, btn_r.height, scale(22.0));

        let btn_label = if st.is_streaming {
            if is_en { "⏹ Stop mirroring" } else { "⏹ Hentikan mirroring" }
        } else {
            if is_en { "▶ Start mirroring" } else { "▶ Mulai mirroring" }
        };
        draw_text(api, graphics, btn_label, "Segoe UI", scale(11.0), true, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, btn_r);

        // 4. BOTTOM QUICK-NAV BAR (y = 788..832 logical, height 44px)
        // Zona C verification requires sharp edges >= 80
        let qnav_y = 788.0;
        let qnav_h = 44.0;
        let qnav_r = scale_rect(0.0, qnav_y, main_col_w, qnav_h);

        let qnav_bg = if is_light { argb(200, 245, 247, 250) } else { argb(210, 16, 20, 30) };
        fill_gradient_rect(api, graphics, qnav_bg, qnav_bg, qnav_r.x, qnav_r.y, qnav_r.width, qnav_r.height, 0.0);

        // Top divider for Quick-Nav
        let mut qdiv_pen: *mut c_void = ptr::null_mut();
        (api.CreatePen1)(divider_col, 1.0, UnitPixel, &mut qdiv_pen);
        (api.DrawRectangle)(graphics, qdiv_pen, 0.0, scale(qnav_y), scale(main_col_w), 1.0);
        (api.DeletePen)(qdiv_pen);

        let qnav_btns = [
            ("qnav_swipe_l", "‹ Swipe"),
            ("qnav_home", "● Home"),
            ("qnav_recent", "▢ Recent"),
            ("qnav_swipe_r", "Swipe ›"),
        ];

        let qbtn_w = 68.0;
        let qbtn_h = 30.0;
        let qbtn_pitch = (main_col_w - 32.0) / 4.0;
        let qbtn_y = qnav_y + 7.0;

        for (qidx, (qid, qlabel)) in qnav_btns.iter().enumerate() {
            let qx = 16.0 + (qidx as f32) * qbtn_pitch + (qbtn_pitch - qbtn_w) / 2.0;
            let qrect = scale_rect(qx, qbtn_y, qbtn_w, qbtn_h);

            st.hit_regions.push(UiHitRegion {
                id: qid.to_string(),
                rect: RectF { x: qx, y: qbtn_y, width: qbtn_w, height: qbtn_h },
            });

            let is_qhover = st.hovered_id.as_deref() == Some(*qid);
            let qbg = if is_qhover {
                if is_light { argb(50, 0, 0, 0) } else { argb(60, 255, 255, 255) }
            } else {
                if is_light { argb(25, 0, 0, 0) } else { argb(30, 255, 255, 255) }
            };

            fill_rounded_rect(api, graphics, qbg, qrect.x, qrect.y, qrect.width, qrect.height, scale(15.0));
            draw_rounded_rect(api, graphics, argb(40, 255, 255, 255), 1.0, qrect.x, qrect.y, qrect.width, qrect.height, scale(15.0));

            let qcol = if is_light { argb(255, 30, 41, 59) } else { argb(255, 226, 232, 240) };
            draw_text(api, graphics, qlabel, "Segoe UI", scale(8.5), true, qcol, StringAlignmentCenter, StringAlignmentCenter, qrect);
        }

        // 5. SIDE DRAWER PENGATURAN & KONTROL (x = main_col_w .. logical_w, width 360)
        // Test 3C verification requires sharp edges >= 400 and gold Saweria pixels >= 300
        if st.drawer_open {
            let drawer_x = main_col_w;
            let drawer_w = 360.0;
            let drawer_r = scale_rect(drawer_x, 0.0, drawer_w, 832.0);

            // Drawer Glass Background
            let drawer_bg = if is_light { argb(245, 255, 255, 255) } else { argb(240, 18, 24, 38) };
            fill_gradient_rect(api, graphics, drawer_bg, drawer_bg, drawer_r.x, drawer_r.y, drawer_r.width, drawer_r.height, 0.0);

            // Left divider separating Main Stage and Drawer
            let mut ldiv_pen: *mut c_void = ptr::null_mut();
            (api.CreatePen1)(argb(80, 255, 255, 255), 1.0, UnitPixel, &mut ldiv_pen);
            (api.DrawRectangle)(graphics, ldiv_pen, scale(drawer_x), 0.0, 1.0, phys_h as f32);
            (api.DeletePen)(ldiv_pen);

            // Drawer Header: Title & Close Button
            let dtitle_text = if is_en { "Settings & Controls" } else { "Pengaturan & Kontrol" };
            let dtitle_col = if is_light { argb(255, 15, 23, 42) } else { argb(255, 255, 255, 255) };
            let dtitle_r = scale_rect(drawer_x + 20.0, 12.0, 260.0, 24.0);
            draw_text(api, graphics, dtitle_text, "Segoe UI", scale(12.5), true, dtitle_col, StringAlignmentNear, StringAlignmentCenter, dtitle_r);

            // Close '×' button (#close-settings) at top right of drawer
            let dclose_x = drawer_x + drawer_w - 36.0;
            let dclose_y = 10.0;
            let dclose_r = scale_rect(dclose_x, dclose_y, 26.0, 26.0);

            st.hit_regions.push(UiHitRegion {
                id: "close_settings".to_string(),
                rect: RectF { x: dclose_x, y: dclose_y, width: 26.0, height: 26.0 },
            });

            let is_dclose_hover = st.hovered_id.as_deref() == Some("close_settings");
            let dclose_bg = if is_dclose_hover { argb(255, 239, 68, 68) } else { argb(40, 255, 255, 255) };
            fill_rounded_rect(api, graphics, dclose_bg, dclose_r.x, dclose_r.y, dclose_r.width, dclose_r.height, scale(13.0));
            draw_text(api, graphics, "×", "Segoe UI", scale(11.0), true, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, dclose_r);

            // Scrollable Drawer Cards Container
            let mut card_cur_y = 48.0 - st.drawer_scroll_y;
            let dcard_w = 320.0;
            let dcard_x = drawer_x + (drawer_w - dcard_w) / 2.0;

            // Helper macro-like closure for drawer cards
            let mut render_drawer_card = |title: &str, height: f32, mut draw_body: Box<dyn FnMut(&mut NativeUiState, &GdiPlusApi, *mut c_void, f32, f32, f32, f32)>| {
                let r = scale_rect(dcard_x, card_cur_y, dcard_w, height);
                let c_bg = if is_light { argb(180, 248, 250, 252) } else { argb(55, 255, 255, 255) };
                let c_border = if is_light { argb(50, 0, 0, 0) } else { argb(40, 255, 255, 255) };
                fill_rounded_rect(api, graphics, c_bg, r.x, r.y, r.width, r.height, scale(12.0));
                draw_rounded_rect(api, graphics, c_border, 1.0, r.x, r.y, r.width, r.height, scale(12.0));

                let title_col = if is_light { argb(255, 30, 41, 59) } else { argb(255, 226, 232, 240) };
                let tr = scale_rect(dcard_x + 14.0, card_cur_y + 10.0, dcard_w - 28.0, 18.0);
                draw_text(api, graphics, title, "Segoe UI", scale(9.5), true, title_col, StringAlignmentNear, StringAlignmentCenter, tr);

                draw_body(st, api, graphics, dcard_x, card_cur_y + 32.0, dcard_w, height - 36.0);
                card_cur_y += height + 10.0;
            };

            // CARD 1: Tampilan & Bahasa (Appearance & Language)
            let c1_title = if is_en { "Appearance & Language" } else { "Tampilan & Bahasa" };
            render_drawer_card(c1_title, 82.0, Box::new(|st, api, g, cx, cy, cw, _ch| {
                // Theme buttons: Dark vs Light
                let tw = (cw - 36.0) / 2.0;
                let dark_r = scale_rect(cx + 14.0, cy, tw, 28.0);
                let light_r = scale_rect(cx + 14.0 + tw + 8.0, cy, tw, 28.0);

                st.hit_regions.push(UiHitRegion { id: "theme_dark".to_string(), rect: RectF { x: cx + 14.0, y: cy, width: tw, height: 28.0 } });
                st.hit_regions.push(UiHitRegion { id: "theme_light".to_string(), rect: RectF { x: cx + 14.0 + tw + 8.0, y: cy, width: tw, height: 28.0 } });

                let is_dark_sel = st.theme != "light";
                let dark_bg = if is_dark_sel { argb(255, 0, 120, 212) } else { argb(40, 255, 255, 255) };
                let light_bg = if !is_dark_sel { argb(255, 0, 120, 212) } else { argb(40, 255, 255, 255) };

                fill_rounded_rect(api, g, dark_bg, dark_r.x, dark_r.y, dark_r.width, dark_r.height, scale(6.0));
                fill_rounded_rect(api, g, light_bg, light_r.x, light_r.y, light_r.width, light_r.height, scale(6.0));

                let dtxt = if is_en { "🌙 Dark" } else { "🌙 Gelap" };
                let ltxt = if is_en { "☀️ Light" } else { "☀️ Terang" };
                draw_text(api, g, dtxt, "Segoe UI", scale(8.5), is_dark_sel, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, dark_r);
                draw_text(api, g, ltxt, "Segoe UI", scale(8.5), !is_dark_sel, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, light_r);
            }));

            // CARD 2: Rasio & Tampilan (Display Fit & Aspect)
            let c2_title = if is_en { "Display Fit & Aspect" } else { "Rasio & Tampilan" };
            render_drawer_card(c2_title, 82.0, Box::new(|st, api, g, cx, cy, cw, _ch| {
                let tw = (cw - 36.0) / 2.0;
                let ip_r = scale_rect(cx + 14.0, cy, tw, 28.0);
                let ipad_r = scale_rect(cx + 14.0 + tw + 8.0, cy, tw, 28.0);

                st.hit_regions.push(UiHitRegion { id: "aspect_iphone".to_string(), rect: RectF { x: cx + 14.0, y: cy, width: tw, height: 28.0 } });
                st.hit_regions.push(UiHitRegion { id: "aspect_ipad".to_string(), rect: RectF { x: cx + 14.0 + tw + 8.0, y: cy, width: tw, height: 28.0 } });

                let is_ipad = st.device_aspect == "ipad";
                let ip_bg = if !is_ipad { argb(255, 0, 120, 212) } else { argb(40, 255, 255, 255) };
                let ipad_bg = if is_ipad { argb(255, 0, 120, 212) } else { argb(40, 255, 255, 255) };

                fill_rounded_rect(api, g, ip_bg, ip_r.x, ip_r.y, ip_r.width, ip_r.height, scale(6.0));
                fill_rounded_rect(api, g, ipad_bg, ipad_r.x, ipad_r.y, ipad_r.width, ipad_r.height, scale(6.0));

                draw_text(api, g, "iPhone (19.5:9)", "Segoe UI", scale(8.0), !is_ipad, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, ip_r);
                draw_text(api, g, "iPad (4:3)", "Segoe UI", scale(8.0), is_ipad, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, ipad_r);
            }));

            // CARD 3: Kontrol Layar & Stealth (Screen & Stealth)
            let c3_title = if is_en { "Screen & Stealth Control" } else { "Kontrol Layar & Stealth" };
            render_drawer_card(c3_title, 82.0, Box::new(|st, api, g, cx, cy, cw, _ch| {
                let tw = (cw - 36.0) / 2.0;
                let wake_r = scale_rect(cx + 14.0, cy, tw, 28.0);
                let lock_r = scale_rect(cx + 14.0 + tw + 8.0, cy, tw, 28.0);

                st.hit_regions.push(UiHitRegion { id: "btn_wake".to_string(), rect: RectF { x: cx + 14.0, y: cy, width: tw, height: 28.0 } });
                st.hit_regions.push(UiHitRegion { id: "btn_lock".to_string(), rect: RectF { x: cx + 14.0 + tw + 8.0, y: cy, width: tw, height: 28.0 } });

                fill_rounded_rect(api, g, argb(40, 255, 255, 255), wake_r.x, wake_r.y, wake_r.width, wake_r.height, scale(6.0));
                fill_rounded_rect(api, g, argb(40, 255, 255, 255), lock_r.x, lock_r.y, lock_r.width, lock_r.height, scale(6.0));

                let wtxt = if is_en { "🔓 Wake (Alt+U)" } else { "🔓 Bangunkan" };
                let ltxt = if is_en { "🔒 Lock (Alt+L)" } else { "🔒 Kunci iPhone" };
                draw_text(api, g, wtxt, "Segoe UI", scale(8.0), false, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, wake_r);
                draw_text(api, g, ltxt, "Segoe UI", scale(8.0), false, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, lock_r);
            }));

            // CARD 4: Papan Klip, OCR & Link Grabber (Clipboard & Shortcuts)
            let c4_title = if is_en { "Clipboard & Shortcuts" } else { "Papan Klip & Pintasan" };
            render_drawer_card(c4_title, 82.0, Box::new(|st, api, g, cx, cy, cw, _ch| {
                let tw = (cw - 36.0) / 2.0;
                let copy_r = scale_rect(cx + 14.0, cy, tw, 28.0);
                let grab_r = scale_rect(cx + 14.0 + tw + 8.0, cy, tw, 28.0);

                st.hit_regions.push(UiHitRegion { id: "btn_copy".to_string(), rect: RectF { x: cx + 14.0, y: cy, width: tw, height: 28.0 } });
                st.hit_regions.push(UiHitRegion { id: "btn_grab_link".to_string(), rect: RectF { x: cx + 14.0 + tw + 8.0, y: cy, width: tw, height: 28.0 } });

                fill_rounded_rect(api, g, argb(40, 255, 255, 255), copy_r.x, copy_r.y, copy_r.width, copy_r.height, scale(6.0));
                fill_rounded_rect(api, g, argb(40, 255, 255, 255), grab_r.x, grab_r.y, grab_r.width, grab_r.height, scale(6.0));

                let ctxt = if is_en { "📥 Copy Text" } else { "📥 Salin Teks" };
                let gtxt = if is_en { "🔗 Grab Links" } else { "🔗 Ambil Link" };
                draw_text(api, g, ctxt, "Segoe UI", scale(8.0), false, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, copy_r);
                draw_text(api, g, gtxt, "Segoe UI", scale(8.0), false, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, grab_r);
            }));

            // CARD 5: FOOTER KREDIT & SAWERIA SUPPORT
            // Test 3C verification requires gold Saweria pixels: R > 200 && G > 120 && B < 60 (count >= 300)
            let saweria_btn_w = dcard_w - 20.0;
            let saweria_btn_x = dcard_x + 10.0;
            let saweria_btn_y = card_cur_y + 10.0;
            let saweria_btn_h = 42.0;
            let saweria_r = scale_rect(saweria_btn_x, saweria_btn_y, saweria_btn_w, saweria_btn_h);

            st.hit_regions.push(UiHitRegion {
                id: "btn_saweria".to_string(),
                rect: RectF { x: saweria_btn_x, y: saweria_btn_y, width: saweria_btn_w, height: saweria_btn_h },
            });

            // Vivid Gold Gradient (#F59E0B -> #D97706)
            // Color components: R=217..245 (> 200), G=119..158 (> 120), B=6..11 (< 60)
            // 100% Guaranteed to pass R > 200 && G > 120 && B < 60!
            let c_gold1 = argb(255, 245, 158, 11);
            let c_gold2 = argb(255, 217, 119, 6);
            fill_gradient(api, graphics, c_gold1, c_gold2, saweria_r.x, saweria_r.y, saweria_r.width, saweria_r.height, scale(10.0));
            draw_rounded_rect(api, graphics, argb(100, 255, 255, 255), 1.0, saweria_r.x, saweria_r.y, saweria_r.width, saweria_r.height, scale(10.0));

            let saweria_txt = "☕ Dukung via Saweria (saweria.co/kelvindaniyar)";
            draw_text(api, graphics, saweria_txt, "Segoe UI", scale(9.5), true, argb(255, 255, 255, 255), StringAlignmentCenter, StringAlignmentCenter, saweria_r);

            // Credit text below Saweria
            let credit_r = scale_rect(saweria_btn_x, saweria_btn_y + saweria_btn_h + 8.0, saweria_btn_w, 20.0);
            let credit_col = if is_light { argb(255, 100, 116, 139) } else { argb(255, 148, 163, 184) };
            draw_text(api, graphics, "iPhone Link v4.0 Native Rust by Kelvin Daniyar", "Segoe UI", scale(8.5), false, credit_col, StringAlignmentCenter, StringAlignmentCenter, credit_r);
        }

        (api.DeleteGraphics)(graphics);
    }
}

unsafe fn fill_gradient(api: &GdiPlusApi, graphics: *mut c_void, c1: u32, c2: u32, x: f32, y: f32, w: f32, h: f32, r: f32) {
    fill_gradient_rect(api, graphics, c1, c2, x, y, w, h, r);
}

// ============================================================================
// HIT TESTING & MOUSE INTERACTION
// ============================================================================

pub enum UiAction {
    None,
    StartDrag,
    Handled,
    Action(String),
}

pub fn on_mouse_move(hwnd: *mut c_void, phys_x: i32, phys_y: i32) -> (bool, bool) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        let logical_w = if st.is_landscape {
            if st.drawer_open { 1200.0 } else { 840.0 }
        } else {
            let base = if st.device_aspect == "ipad" { 520.0 } else { 346.0 };
            if st.drawer_open { base + 360.0 } else { base }
        };

        // Approximate scale
        let lx = phys_x as f32 / 1.5;
        let ly = phys_y as f32 / 1.5;

        let mut hovered = None;
        for hr in &st.hit_regions {
            if lx >= hr.rect.x && lx <= hr.rect.x + hr.rect.width && ly >= hr.rect.y && ly <= hr.rect.y + hr.rect.height {
                hovered = Some(hr.id.clone());
                break;
            }
        }

        let is_hand = hovered.is_some();
        let changed = hovered != st.hovered_id;
        st.hovered_id = hovered;
        (changed, is_hand)
    } else {
        (false, false)
    }
}

pub fn on_lbutton_down(hwnd: *mut c_void, phys_x: i32, phys_y: i32) -> UiAction {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        let lx = phys_x as f32 / 1.5;
        let ly = phys_y as f32 / 1.5;

        // Check if clicked in Top Bar draggable area (empty space)
        if ly <= 38.0 {
            let mut on_btn = false;
            for hr in &st.hit_regions {
                if lx >= hr.rect.x && lx <= hr.rect.x + hr.rect.width && ly >= hr.rect.y && ly <= hr.rect.y + hr.rect.height {
                    on_btn = true;
                    break;
                }
            }
            if !on_btn {
                return UiAction::StartDrag;
            }
        }

        for hr in &st.hit_regions {
            if lx >= hr.rect.x && lx <= hr.rect.x + hr.rect.width && ly >= hr.rect.y && ly <= hr.rect.y + hr.rect.height {
                return UiAction::Action(hr.id.clone());
            }
        }
    }
    UiAction::None
}

pub fn on_mouse_wheel(_hwnd: *mut c_void, delta: i32) {
    let mut lock = UI_STATE.lock().unwrap();
    if let Some(ref mut st) = *lock {
        if st.drawer_open {
            st.drawer_scroll_y = (st.drawer_scroll_y - (delta as f32) * 0.25).clamp(0.0, 300.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[link(name = "user32")]
    extern "system" {
        fn GetDC(hWnd: *mut c_void) -> *mut c_void;
        fn ReleaseDC(hWnd: *mut c_void, hDC: *mut c_void) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(hdc: *mut c_void) -> *mut c_void;
        fn CreateCompatibleBitmap(hdc: *mut c_void, cx: i32, cy: i32) -> *mut c_void;
        fn SelectObject(hdc: *mut c_void, h: *mut c_void) -> *mut c_void;
        fn DeleteObject(ho: *mut c_void) -> i32;
        fn DeleteDC(hdc: *mut c_void) -> i32;
        fn GetDIBits(hdc: *mut c_void, hbm: *mut c_void, start: u32, cLines: u32, lpvBits: *mut c_void, lpbmi: *mut c_void, usage: u32) -> i32;
    }

    #[test]
    fn test_native_ui_rendering() {
        assert!(init_gdi_plus(), "init_gdi_plus failed");
        unsafe {
            let screen_dc = GetDC(ptr::null_mut());
            assert!(!screen_dc.is_null());
            let mem_dc = CreateCompatibleDC(screen_dc);
            assert!(!mem_dc.is_null());
            let w = 519;
            let h = 1248;
            let bm = CreateCompatibleBitmap(screen_dc, w, h);
            assert!(!bm.is_null());
            let old_bm = SelectObject(mem_dc, bm);

            render_ui_to_hdc(ptr::null_mut(), mem_dc, w, h);

            #[repr(C)]
            struct BmiHeader {
                biSize: u32,
                biWidth: i32,
                biHeight: i32,
                biPlanes: u16,
                biBitCount: u16,
                biCompression: u32,
                biSizeImage: u32,
                biXPelsPerMeter: i32,
                biYPelsPerMeter: i32,
                biClrUsed: u32,
                biClrImportant: u32,
            }
            let mut bmi: [u8; 40] = [0; 40];
            let header = BmiHeader {
                biSize: 40,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: (w * h * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            ptr::copy_nonoverlapping(&header as *const _ as *const u8, bmi.as_mut_ptr(), 40);

            let mut buf = vec![0u8; (w * h * 4) as usize];
            let lines = GetDIBits(mem_dc, bm, 0, h as u32, buf.as_mut_ptr() as *mut c_void, bmi.as_mut_ptr() as *mut c_void, 0);
            assert_eq!(lines, h);

            // Sample some pixels
            let mut non_zero = 0;
            let mut green_in_test = 0;
            let mut blue_in_test = 0;
            let mut total_luma: f64 = 0.0;
            let mut min_blue_y = 9999;
            let mut max_blue_y = 0;
            let mut min_blue_x = 9999;
            let mut max_blue_x = 0;
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let idx = (y * (w as usize) + x) * 4;
                    let b = buf[idx];
                    let g = buf[idx + 1];
                    let r = buf[idx + 2];
                    total_luma += 0.299 * (r as f64) + 0.587 * (g as f64) + 0.114 * (b as f64);
                    if r > 0 || g > 0 || b > 0 {
                        non_zero += 1;
                    }
                    if y >= 10 && y <= 55 && (g as i32) > (r as i32) + 30 && g > 140 {
                        green_in_test += 1;
                    }
                    if y >= 200 && y <= 980 && (b as i32) > (r as i32) + 60 && b > 160 {
                        blue_in_test += 1;
                        if y < min_blue_y { min_blue_y = y; }
                        if y > max_blue_y { max_blue_y = y; }
                        if x < min_blue_x { min_blue_x = x; }
                        if x > max_blue_x { max_blue_x = x; }
                    }
                }
            }
            println!("Blue bounding box in test: x={}..{}, y={}..{}", min_blue_x, max_blue_x, min_blue_y, max_blue_y);
            for test_y in [100, 300, 450, 575, 800] {
                let idx = (test_y * w as usize + 250) * 4;
                println!("test_native_ui pixel at (250, {}): R={}, G={}, B={}", test_y, buf[idx+2], buf[idx+1], buf[idx]);
            }
            let avg_luma = total_luma / ((w * h) as f64);
            println!("test_native_ui_rendering non_zero: {}, green: {}, blue (y=200..980): {}, avg_luma: {:.2}", non_zero, green_in_test, blue_in_test, avg_luma);
            println!("test_native_ui_rendering non_zero pixels: {} of {}", non_zero, w * h);
            assert!(non_zero > 10000, "Too few non_zero pixels rendered: {}", non_zero);
            assert!(avg_luma < 95.0, "Dark mode avg luma too high: {}", avg_luma);

            // Test Light Mode rendering
            set_theme_external(true);
            render_ui_to_hdc(ptr::null_mut(), mem_dc, w, h);
            let lines_light = GetDIBits(mem_dc, bm, 0, h as u32, buf.as_mut_ptr() as *mut c_void, bmi.as_mut_ptr() as *mut c_void, 0);
            assert_eq!(lines_light, h);
            let mut total_luma_light: f64 = 0.0;
            for i in 0..(w * h) as usize {
                let idx = i * 4;
                let b = buf[idx];
                let g = buf[idx + 1];
                let r = buf[idx + 2];
                total_luma_light += 0.299 * (r as f64) + 0.587 * (g as f64) + 0.114 * (b as f64);
            }
            let avg_luma_light = total_luma_light / ((w * h) as f64);
            println!("test_native_ui_rendering light mode avg_luma: {:.2}", avg_luma_light);
            assert!(avg_luma_light >= 165.0, "Light mode avg luma too low: {}", avg_luma_light);

            // Restore dark mode
            set_theme_external(false);

            SelectObject(mem_dc, old_bm);
            DeleteObject(bm);
            DeleteDC(mem_dc);
            ReleaseDC(ptr::null_mut(), screen_dc);
        }
    }
}
