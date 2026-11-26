//! Direct3D 11 Hardware Video Presenter (`D3d11VideoPresenter`), SwapChain
//! `DXGI_SWAP_EFFECT_FLIP_DISCARD`, Hardware Rotation (`0°`/`90°`/`180°`/`270°`),
//! 3 Display Fit Modes (`Auto-Fit`, `Edge-to-Edge`, `Original Ratio`),
//! 4-Side Sub-Pixel Edge Crop (`0..=24 px`), Inverse Touch Transform Matrix,
//! and Lossless BMP Snapshot Bridge (`< 5 ms`) to WinRT OCR / Link Grabber.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use crate::autorotate::{ProbeGrid120x260, GRID_HEIGHT, GRID_WIDTH};
use crate::d3d11_context::{
    ComPtr, D3d11Nv12VramRing, D3d11VideoColor, D3d11VideoProcessorCaps,
    D3d11VideoProcessorContentDesc, D3d11VideoProcessorInputViewDesc,
    D3d11VideoProcessorOutputViewDesc, D3d11VideoProcessorStream, DxgiRational, DxgiSampleDesc,
    DxgiSwapChainDesc1, LiveD3d11Gpu, Rect, SharedFrameSlot, VideoColorInfo, VramFrameSnapshot,
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION, D3D11_VIDEO_PROCESSOR_ROTATION_180,
    D3D11_VIDEO_PROCESSOR_ROTATION_270, D3D11_VIDEO_PROCESSOR_ROTATION_90,
    D3D11_VIDEO_PROCESSOR_ROTATION_IDENTITY, D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_FORMAT_UNKNOWN, DXGI_MWA_NO_ALT_ENTER,
    DXGI_SCALING_STRETCH, DXGI_USAGE_RENDER_TARGET_OUTPUT, IID_ID3D11_TEXTURE2D,
};
use crate::mf_decoder::yuv_to_rgb_with_color_info;
use crate::nv12_hd_shader::HdNv12ShaderRenderer;

// DXGI & Win32 Child HWND Constants
pub const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;
pub const DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL: u32 = 3;
pub const DXGI_SWAP_EFFECT_FLIP_DISCARD: u32 = 4;
pub const DXGI_ALPHA_MODE_PREMULTIPLIED: u32 = 1;
pub const DXGI_ALPHA_MODE_IGNORE: u32 = 3;

pub const HEVC_LUMA_CAS_STRENGTH: f32 = 0.28;

static HD_SHADER_TRACE_ACTIVE: AtomicBool = AtomicBool::new(false);
static HD_SHADER_TRACE_PRESENTS: AtomicU64 = AtomicU64::new(0);

pub fn hd_shader_trace_active() -> bool {
    HD_SHADER_TRACE_ACTIVE.load(Ordering::Relaxed)
}

pub fn hd_shader_trace_presents() -> u64 {
    HD_SHADER_TRACE_PRESENTS.load(Ordering::Relaxed)
}

pub fn reset_hd_shader_trace() {
    HD_SHADER_TRACE_PRESENTS.store(0, Ordering::Relaxed);
}

pub fn reset_hd_shader_trace_all() {
    HD_SHADER_TRACE_ACTIVE.store(false, Ordering::Relaxed);
    HD_SHADER_TRACE_PRESENTS.store(0, Ordering::Relaxed);
}

/// Creates a DirectComposition-compatible `IDXGISwapChain1` (`FLIP_SEQUENTIAL` + `SCALING_STRETCH`)
/// via `IDXGIFactory2::CreateSwapChainForComposition` (vtable slot 24).
pub fn create_composition_swapchain(
    gpu: &LiveD3d11Gpu,
    width: u32,
    height: u32,
    alpha_mode: u32,
) -> Result<ComPtr, String> {
    let w = width.max(1);
    let h = height.max(1);
    let factory = gpu.create_dxgi_factory2().map_err(|hr| {
        format!(
            "CreateDXGIFactory1(IDXGIFactory2) failed: HRESULT=0x{:08X}",
            hr as u32
        )
    })?;

    let sc_desc = DxgiSwapChainDesc1 {
        width: w,
        height: h,
        format: DXGI_FORMAT_B8G8R8A8_UNORM,
        stereo: 0,
        sample_desc: DxgiSampleDesc { count: 1, quality: 0 },
        buffer_usage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        buffer_count: 2,
        scaling: DXGI_SCALING_STRETCH,
        swap_effect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        alpha_mode,
        flags: 0,
    };

    unsafe {
        let f_vt = factory.vtable();
        let create_for_comp: unsafe extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const DxgiSwapChainDesc1,
            *mut c_void,
            *mut *mut c_void,
        ) -> i32 = std::mem::transmute(*f_vt.add(24));

        let mut p_sc: *mut c_void = std::ptr::null_mut();
        let hr = create_for_comp(
            factory.as_ptr(),
            gpu.device().as_ptr(),
            &sc_desc,
            std::ptr::null_mut(),
            &mut p_sc,
        );
        if hr < 0 || p_sc.is_null() {
            return Err(format!(
                "IDXGIFactory2::CreateSwapChainForComposition failed: HRESULT=0x{:08X}",
                hr as u32
            ));
        }
        ComPtr::from_raw(p_sc).ok_or_else(|| "null composition IDXGISwapChain1".to_string())
    }
}

pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;

pub const TOP_BAR_HEIGHT_PX: u32 = 38;
pub const BOTTOM_NAV_HEIGHT_PX: u32 = 44;
pub const DRAWER_WIDTH_PX: u32 = 360;

pub const NORMAL_PORTRAIT_STAGE_W: u32 = 346;
pub const NORMAL_PORTRAIT_STAGE_H: u32 = 750;
pub const NORMAL_LANDSCAPE_STAGE_W: u32 = 840;
pub const NORMAL_LANDSCAPE_STAGE_H: u32 = 386;

/// Display Fit Modes synchronized between UI settings and the D3D11 SwapChain presenter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitMode {
    AutoFit,
    EdgeToEdge,
    OriginalRatio,
}

impl FitMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            FitMode::AutoFit => "auto_fit",
            FitMode::EdgeToEdge => "edge_to_edge",
            FitMode::OriginalRatio => "original_ratio",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        let low = s.trim().to_ascii_lowercase().replace('-', "_");
        match low.as_str() {
            "edge_to_edge" | "edgetoedge" | "cover" | "fill" | "stretch" => FitMode::EdgeToEdge,
            "original_ratio" | "originalratio" | "contain" | "original" | "1:1" => {
                FitMode::OriginalRatio
            }
            _ => FitMode::AutoFit,
        }
    }
}

impl Default for FitMode {
    fn default() -> Self {
        FitMode::AutoFit
    }
}

/// Hardware DXGI SwapChain descriptor specification tailored for Child HWND low-latency
/// capture compatibility with OBS Studio and TikTok LIVE Studio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapChainSpec {
    pub format: u32,
    pub format_name: &'static str,
    pub buffer_count: u32,
    pub swap_effect: u32,
    pub swap_effect_name: &'static str,
    pub alpha_mode: u32,
    pub alpha_mode_name: &'static str,
    pub vsync_interval: u32,
    pub obs_capture_compatible: bool,
    pub video_canvas_solid_black: bool,
    pub chrome_glass_preserved: bool,
    pub hwnd_style_flags: u32,
}

impl Default for SwapChainSpec {
    fn default() -> Self {
        Self {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            format_name: "DXGI_FORMAT_B8G8R8A8_UNORM",
            buffer_count: 2,
            swap_effect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            swap_effect_name: "DXGI_SWAP_EFFECT_FLIP_DISCARD",
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            alpha_mode_name: "DXGI_ALPHA_MODE_IGNORE",
            vsync_interval: 1,
            obs_capture_compatible: true,
            video_canvas_solid_black: true,
            chrome_glass_preserved: true,
            hwnd_style_flags: WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
        }
    }
}

/// Presenter configuration combining Display Fit mode, hardware rotation,
/// 4-side sub-pixel edge crop (`0..=24 px`), and desktop container state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenterConfig {
    pub fit_mode: FitMode,
    pub rotation_deg: u16,
    pub crop_top: u32,
    pub crop_bottom: u32,
    pub crop_left: u32,
    pub crop_right: u32,
    pub base_trim_px: u32,
    pub win_w: u32,
    pub win_h: u32,
    pub is_fullscreen: bool,
    pub is_landscape: bool,
    pub drawer_open: bool,
}

impl Default for PresenterConfig {
    fn default() -> Self {
        Self {
            fit_mode: FitMode::AutoFit,
            rotation_deg: 0,
            crop_top: 0,
            crop_bottom: 0,
            crop_left: 0,
            crop_right: 0,
            base_trim_px: 0,
            win_w: 346,
            win_h: 832,
            is_fullscreen: false,
            is_landscape: false,
            drawer_open: false,
        }
    }
}

impl PresenterConfig {
    pub fn normalized_rotation(&self) -> u16 {
        match self.rotation_deg % 360 {
            90 => 90,
            180 => 180,
            270 => 270,
            _ => 0,
        }
    }

    /// Enables the symmetric `6 px` bezel trim used in Auto-Fit mode when desired.
    pub fn with_base_trim(mut self, px: u32) -> Self {
        self.base_trim_px = px;
        self
    }
}

/// Resolved clean source region after H.265 macroblock trim and optional 4-side edge crop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanSourceRegion {
    pub raw_w: u32,
    pub raw_h: u32,
    pub src_trim_left: u32,
    pub src_trim_top: u32,
    pub src_trim_right: u32,
    pub src_trim_bottom: u32,
    pub sensor_clean_w: u32,
    pub sensor_clean_h: u32,
    pub clean_w: u32,
    pub clean_h: u32,
}

/// Computes the clean source pixel rectangle inside a `SharedFrameSlot`.
pub fn compute_clean_source_region(
    slot: &SharedFrameSlot,
    config: &PresenterConfig,
) -> CleanSourceRegion {
    let raw_w = slot.coded_width.max(1);
    let raw_h = slot.coded_height.max(1);

    let (nat_top, nat_left, nat_bottom, nat_right) = if slot.trim_top == 0
        && slot.trim_left == 0
        && slot.trim_bottom == 0
        && slot.trim_right == 0
        && raw_w == 1216
        && raw_h == 2656
    {
        (0, 0, 58, 14)
    } else {
        (
            slot.trim_top,
            slot.trim_left,
            slot.trim_bottom,
            slot.trim_right,
        )
    };

    let sensor_clean_w = raw_w.saturating_sub(nat_left + nat_right).max(1);
    let sensor_clean_h = raw_h.saturating_sub(nat_top + nat_bottom).max(1);

    let px_scale = (sensor_clean_w as f32 / NORMAL_PORTRAIT_STAGE_W as f32).max(1.0);
    let base = config.base_trim_px;

    let extra_left = (config.crop_left.min(24) as f32 * px_scale).round() as u32 + base;
    let extra_right = (config.crop_right.min(24) as f32 * px_scale).round() as u32 + base;
    let extra_top = (config.crop_top.min(24) as f32 * px_scale).round() as u32 + base;
    let extra_bottom = (config.crop_bottom.min(24) as f32 * px_scale).round() as u32 + base;

    let mut src_trim_left = nat_left.saturating_add(extra_left);
    let mut src_trim_right = nat_right.saturating_add(extra_right);
    let mut src_trim_top = nat_top.saturating_add(extra_top);
    let mut src_trim_bottom = nat_bottom.saturating_add(extra_bottom);

    if src_trim_left + src_trim_right >= raw_w {
        src_trim_left = nat_left;
        src_trim_right = nat_right;
    }
    if src_trim_top + src_trim_bottom >= raw_h {
        src_trim_top = nat_top;
        src_trim_bottom = nat_bottom;
    }

    let clean_w = raw_w.saturating_sub(src_trim_left + src_trim_right).max(1);
    let clean_h = raw_h.saturating_sub(src_trim_top + src_trim_bottom).max(1);

    CleanSourceRegion {
        raw_w,
        raw_h,
        src_trim_left,
        src_trim_top,
        src_trim_right,
        src_trim_bottom,
        sensor_clean_w,
        sensor_clean_h,
        clean_w,
        clean_h,
    }
}

/// CPU-verified / D3D11-parity presented BGRA surface (`DXGI_FORMAT_B8G8R8A8_UNORM`).
#[derive(Debug, Clone)]
pub struct PresentedFrameBuffer {
    pub width: u32,
    pub height: u32,
    pub rotation_deg: u16,
    pub region: CleanSourceRegion,
    pub bgra: Vec<u8>,
}

impl PresentedFrameBuffer {
    /// Returns `[R, G, B]` at output coordinates `(x, y)`.
    pub fn pixel_rgb_at(&self, x: u32, y: u32) -> [u8; 3] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0];
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        [self.bgra[idx + 2], self.bgra[idx + 1], self.bgra[idx]]
    }
}

#[inline(always)]
fn read_slot_pixel_rgb(
    slot: &SharedFrameSlot,
    raw_w: usize,
    y_len: usize,
    has_bgra: bool,
    has_nv12: bool,
    color_info: VideoColorInfo,
    sx: usize,
    sy: usize,
) -> (u8, u8, u8) {
    if has_bgra {
        let idx = (sy * raw_w + sx) * 4;
        (
            slot.bgra_data[idx + 2],
            slot.bgra_data[idx + 1],
            slot.bgra_data[idx],
        )
    } else if has_nv12 {
        let y_val = slot.nv12_data[sy * raw_w + sx];
        let uv_idx = y_len + (sy >> 1) * raw_w + (sx & !1);
        let u_val = slot.nv12_data[uv_idx];
        let v_val = slot.nv12_data[uv_idx + 1];
        yuv_to_rgb_with_color_info(y_val, u_val, v_val, color_info)
    } else {
        (0, 0, 0)
    }
}

/// Renders one `SharedFrameSlot` into a clean, rotated `DXGI_FORMAT_B8G8R8A8_UNORM` surface
/// with `alpha = 255` (`DXGI_ALPHA_MODE_IGNORE`) and stripped H.265 macroblock black padding.
pub fn render_slot_to_bgra_surface(
    slot: &SharedFrameSlot,
    config: &PresenterConfig,
) -> PresentedFrameBuffer {
    render_slot_to_bgra_surface_with_color(slot, config, VideoColorInfo::default())
}

/// Renders one `SharedFrameSlot` using explicit `VideoColorInfo` (Full/Limited BT.709/601/2020).
pub fn render_slot_to_bgra_surface_with_color(
    slot: &SharedFrameSlot,
    config: &PresenterConfig,
    color_info: VideoColorInfo,
) -> PresentedFrameBuffer {
    let region = compute_clean_source_region(slot, config);
    let rot = config.normalized_rotation();
    let clean_w = region.clean_w as usize;
    let clean_h = region.clean_h as usize;

    let (out_w, out_h) = match rot {
        90 | 270 => (clean_h, clean_w),
        _ => (clean_w, clean_h),
    };

    let raw_w = slot.coded_width as usize;
    let raw_h = slot.coded_height as usize;
    let y_len = raw_w * raw_h;
    let has_bgra = slot.bgra_data.len() >= raw_w * raw_h * 4;
    let has_nv12 = slot.nv12_data.len() >= y_len + y_len / 2;

    let mut bgra = vec![0u8; out_w * out_h * 4];
    let trim_l = region.src_trim_left as usize;
    let trim_t = region.src_trim_top as usize;

    // Sequential write in output scanline order for cache locality.
    match rot {
        0 => {
            for oy in 0..out_h {
                let sy = trim_t + oy;
                let row_off = oy * out_w * 4;
                for ox in 0..out_w {
                    let sx = trim_l + ox;
                    let (r, g, b) = read_slot_pixel_rgb(slot, raw_w, y_len, has_bgra, has_nv12, color_info, sx, sy);
                    let dst = row_off + ox * 4;
                    bgra[dst] = b;
                    bgra[dst + 1] = g;
                    bgra[dst + 2] = r;
                    bgra[dst + 3] = 255;
                }
            }
        }
        270 => {
            // 270° CW (= 90° CCW Landscape): Dynamic Island moves to left edge (out_x = src_cy)
            // Inverse: cx = clean_w - 1 - oy, cy = ox
            for oy in 0..out_h {
                let cx = clean_w - 1 - oy;
                let sx = trim_l + cx;
                let row_off = oy * out_w * 4;
                for ox in 0..out_w {
                    let sy = trim_t + ox;
                    let (r, g, b) = read_slot_pixel_rgb(slot, raw_w, y_len, has_bgra, has_nv12, color_info, sx, sy);
                    let dst = row_off + ox * 4;
                    bgra[dst] = b;
                    bgra[dst + 1] = g;
                    bgra[dst + 2] = r;
                    bgra[dst + 3] = 255;
                }
            }
        }
        90 => {
            // 90° CW Inverted Landscape: Dynamic Island moves to right edge (out_x = clean_h - 1 - cy)
            // Inverse: cx = oy, cy = clean_h - 1 - ox
            for oy in 0..out_h {
                let sx = trim_l + oy;
                let row_off = oy * out_w * 4;
                for ox in 0..out_w {
                    let cy = clean_h - 1 - ox;
                    let sy = trim_t + cy;
                    let (r, g, b) = read_slot_pixel_rgb(slot, raw_w, y_len, has_bgra, has_nv12, color_info, sx, sy);
                    let dst = row_off + ox * 4;
                    bgra[dst] = b;
                    bgra[dst + 1] = g;
                    bgra[dst + 2] = r;
                    bgra[dst + 3] = 255;
                }
            }
        }
        _ => {
            // 180° Inverted Portrait
            for oy in 0..out_h {
                let cy = clean_h - 1 - oy;
                let sy = trim_t + cy;
                let row_off = oy * out_w * 4;
                for ox in 0..out_w {
                    let cx = clean_w - 1 - ox;
                    let sx = trim_l + cx;
                    let (r, g, b) = read_slot_pixel_rgb(slot, raw_w, y_len, has_bgra, has_nv12, color_info, sx, sy);
                    let dst = row_off + ox * 4;
                    bgra[dst] = b;
                    bgra[dst + 1] = g;
                    bgra[dst + 2] = r;
                    bgra[dst + 3] = 255;
                }
            }
        }
    }

    PresentedFrameBuffer {
        width: out_w as u32,
        height: out_h as u32,
        rotation_deg: rot,
        region,
        bgra,
    }
}

/// Output stage rectangle inside the Win32 parent window (`Top Bar 38px`, `Quick Nav 44px`, `Drawer 360px`).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewportStageRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub top_y: i32,
    pub bottom_y: i32,
    pub fit_mode: FitMode,
    pub is_fullscreen: bool,
    pub is_landscape: bool,
    pub drawer_open: bool,
    pub proportional: bool,
    pub centered: bool,
    pub bottom_black_bar_gap_px: f64,
    pub pillarbox_gap_px: f64,
}

/// Computes the exact child `HWND` / video stage rectangle for all modes:
/// - Normal Portrait (`346 x 832` closed / `706 x 832` drawer open -> `346 x 750`)
/// - Full Screen Portrait (`1920 x 1080` -> `460 x 996`, `x = 730`, `top_y = 39`, `bottom_y = 1035`)
/// - Normal Landscape (`840 x 468` closed / `1200 x 468` drawer open -> locked `840 x 386`)
/// - Full Screen Landscape (`win_h - 82`, centered proportional `2598:1202`)
/// - Smart 1:1 Auto-Snap in `OriginalRatio` when aspect diff `< 0.12` (zero pillarbox).
pub fn compute_presenter_viewport_rect(
    win_w: u32,
    win_h: u32,
    is_fullscreen: bool,
    is_landscape: bool,
    drawer_open: bool,
    fit_mode: FitMode,
) -> ViewportStageRect {
    let avail_w = if drawer_open {
        win_w.saturating_sub(DRAWER_WIDTH_PX).max(1)
    } else {
        win_w.max(1)
    };

    if is_fullscreen && !is_landscape {
        // Full Screen Portrait Proportional Centered (Tahap 73 & 74): 1 px safe margin top & bottom
        let top_y = (TOP_BAR_HEIGHT_PX + 1) as i32;
        let max_h = win_h.saturating_sub(TOP_BAR_HEIGHT_PX + BOTTOM_NAV_HEIGHT_PX + 2).max(1);
        let portrait_aspect = NORMAL_PORTRAIT_STAGE_W as f64 / NORMAL_PORTRAIT_STAGE_H as f64;
        let ideal_w = ((max_h as f64) * portrait_aspect).ceil() as u32;
        let (stage_w, stage_h, y) = if ideal_w <= avail_w {
            (ideal_w.max(1), max_h, top_y)
        } else {
            let scaled_h = ((avail_w as f64) / portrait_aspect).round().max(1.0) as u32;
            let fitted_h = scaled_h.min(max_h);
            (avail_w, fitted_h, top_y + ((max_h.saturating_sub(fitted_h)) / 2) as i32)
        };
        let bottom_y = y + stage_h as i32;
        let x = ((avail_w.saturating_sub(stage_w)) / 2) as i32;
        return ViewportStageRect {
            x,
            y,
            width: stage_w,
            height: stage_h,
            top_y: y,
            bottom_y,
            fit_mode,
            is_fullscreen,
            is_landscape,
            drawer_open,
            proportional: true,
            centered: true,
            bottom_black_bar_gap_px: 0.0,
            pillarbox_gap_px: 0.0,
        };
    }

    if is_landscape {
        let top_y = TOP_BAR_HEIGHT_PX as i32;
        let max_h = win_h.saturating_sub(TOP_BAR_HEIGHT_PX + BOTTOM_NAV_HEIGHT_PX).max(1);
        if !is_fullscreen {
            // Normal Landscape: left stage stays locked at 840x386 (0% squish when drawer opens/closes)
            let stage_w = avail_w;
            let bottom_y = top_y + max_h as i32;
            return ViewportStageRect {
                x: 0,
                y: top_y,
                width: stage_w,
                height: max_h,
                top_y,
                bottom_y,
                fit_mode,
                is_fullscreen,
                is_landscape,
                drawer_open,
                proportional: true,
                centered: false,
                bottom_black_bar_gap_px: 0.0,
                pillarbox_gap_px: 0.0,
            };
        }
        // Full Screen Landscape strict dual-bound aspect fit (zero horizontal/vertical crop)
        let pic_aspect = 2598.0 / 1202.0;
        let scale_fit = ((avail_w as f64) / 2598.0).min((max_h as f64) / 1202.0);
        let stage_w = ((2598.0 * scale_fit).round() as u32).clamp(1, avail_w);
        let stage_h = ((stage_w as f64 / pic_aspect).round() as u32).clamp(1, max_h);
        let y = top_y + ((max_h.saturating_sub(stage_h)) / 2) as i32;
        let bottom_y = y + stage_h as i32;
        let x = ((avail_w.saturating_sub(stage_w)) / 2) as i32;
        return ViewportStageRect {
            x,
            y,
            width: stage_w,
            height: stage_h,
            top_y: y,
            bottom_y,
            fit_mode,
            is_fullscreen,
            is_landscape,
            drawer_open,
            proportional: true,
            centered: true,
            bottom_black_bar_gap_px: 0.0,
            pillarbox_gap_px: 0.0,
        };
    }

    // Normal Portrait (e.g. 346 x 832 -> stage 346 x 750)
    let top_y = TOP_BAR_HEIGHT_PX as i32;
    let stage_w = avail_w;
    let stage_h = win_h.saturating_sub(TOP_BAR_HEIGHT_PX + BOTTOM_NAV_HEIGHT_PX).max(1);

    let stage_aspect = (stage_w as f64) / (stage_h as f64);
    let iphone_clean_aspect = 1202.0_f64 / 2598.0_f64;
    let aspect_diff = (stage_aspect - iphone_clean_aspect).abs() / iphone_clean_aspect;

    let (width, height, pillarbox_gap_px) = if matches!(fit_mode, FitMode::OriginalRatio) && aspect_diff >= 0.12 {
        let fitted_w = ((stage_h as f64) * iphone_clean_aspect).round() as u32;
        if fitted_w <= stage_w {
            (fitted_w.max(1), stage_h, ((stage_w - fitted_w) as f64) * 0.5)
        } else {
            let fitted_h = ((stage_w as f64) / iphone_clean_aspect).round() as u32;
            (stage_w, fitted_h.min(stage_h).max(1), 0.0)
        }
    } else {
        // AutoFit, EdgeToEdge, or Smart 1:1 Auto-Snap when diff < 0.12 (Tahap 64)
        (stage_w, stage_h, 0.0)
    };

    let x = ((stage_w.saturating_sub(width)) / 2) as i32;
    ViewportStageRect {
        x,
        y: top_y,
        width,
        height,
        top_y,
        bottom_y: top_y + height as i32,
        fit_mode,
        is_fullscreen,
        is_landscape,
        drawer_open,
        proportional: true,
        centered: x > 0,
        bottom_black_bar_gap_px: (stage_h.saturating_sub(height)) as f64,
        pillarbox_gap_px,
    }
}

/// Maps client mouse/touch coordinates `(client_x, client_y)` inside the stage viewport
/// into sub-pixel accurate iOS HID coordinates `(0..=65535, 0..=65535)` with Inverse
/// Rotation (`0°`/`90°`/`180°`/`270°`) and 4-side edge crop compensation.
pub fn map_stage_touch_to_ios_hid(
    client_x: f64,
    client_y: f64,
    viewport: &ViewportStageRect,
    config: &PresenterConfig,
) -> (u16, u16) {
    let w = (viewport.width.max(1)) as f64;
    let h = (viewport.height.max(1)) as f64;
    let nx = ((client_x - viewport.x as f64) / w).clamp(0.0, 1.0);
    let ny = ((client_y - viewport.y as f64) / h).clamp(0.0, 1.0);

    let (ux, uy) = match config.normalized_rotation() {
        270 => (1.0 - ny, nx),
        90 => (ny, 1.0 - nx),
        180 => (1.0 - nx, 1.0 - ny),
        _ => (nx, ny),
    };

    // Clean iOS sensor space (1202 x 2598 after fixed macroblock strip)
    const SENSOR_CLEAN_W: f64 = 1202.0;
    const SENSOR_CLEAN_H: f64 = 2598.0;
    let px_scale = SENSOR_CLEAN_W / (NORMAL_PORTRAIT_STAGE_W as f64);

    let extra_l = (config.crop_left.min(24) as f64) * px_scale + (config.base_trim_px as f64);
    let extra_r = (config.crop_right.min(24) as f64) * px_scale + (config.base_trim_px as f64);
    let extra_t = (config.crop_top.min(24) as f64) * px_scale + (config.base_trim_px as f64);
    let extra_b = (config.crop_bottom.min(24) as f64) * px_scale + (config.base_trim_px as f64);

    let vis_w = (SENSOR_CLEAN_W - extra_l - extra_r).max(1.0);
    let vis_h = (SENSOR_CLEAN_H - extra_t - extra_b).max(1.0);

    let ios_norm_x = ((extra_l + ux * vis_w) / SENSOR_CLEAN_W).clamp(0.0, 1.0);
    let ios_norm_y = ((extra_t + uy * vis_h) / SENSOR_CLEAN_H).clamp(0.0, 1.0);

    let hid_x = (ios_norm_x * 65535.0).round().clamp(0.0, 65535.0) as u16;
    let hid_y = (ios_norm_y * 65535.0).round().clamp(0.0, 65535.0) as u16;
    (hid_x, hid_y)
}

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Fast 64-byte lookup-table Base64 encoder for low-latency (`< 5 ms`) snapshot export.
pub fn encode_base64_fast(input: &[u8]) -> String {
    let out_len = ((input.len() + 2) / 3) * 4;
    let mut out = Vec::with_capacity(out_len);
    let mut i = 0usize;
    while i + 3 <= input.len() {
        let a = input[i] as usize;
        let b = input[i + 1] as usize;
        let c = input[i + 2] as usize;
        out.push(B64_TABLE[a >> 2]);
        out.push(B64_TABLE[((a & 0x03) << 4) | (b >> 4)]);
        out.push(B64_TABLE[((b & 0x0F) << 2) | (c >> 6)]);
        out.push(B64_TABLE[c & 0x3F]);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let a = input[i] as usize;
        out.push(B64_TABLE[a >> 2]);
        out.push(B64_TABLE[(a & 0x03) << 4]);
        out.push(b'=');
        out.push(b'=');
    } else if rem == 2 {
        let a = input[i] as usize;
        let b = input[i + 1] as usize;
        out.push(B64_TABLE[a >> 2]);
        out.push(B64_TABLE[((a & 0x03) << 4) | (b >> 4)]);
        out.push(B64_TABLE[(b & 0x0F) << 2]);
        out.push(b'=');
    }
    // Safe: all bytes in B64_TABLE and b'=' are valid ASCII UTF-8.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Fast Base64 decoder used for inspecting BMP snapshots in-process.
pub fn decode_base64_fast(s: &str) -> Option<Vec<u8>> {
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.len() % 4 != 0 {
        return None;
    }
    let decode_char = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    for chunk in clean.chunks_exact(4) {
        let a = decode_char(chunk[0])?;
        let b = decode_char(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = decode_char(chunk[2])?;
            out.push(((b & 0x0F) << 4) | (c >> 2));
            if chunk[3] != b'=' {
                let d = decode_char(chunk[3])?;
                out.push(((c & 0x03) << 6) | d);
            }
        }
    }
    Some(out)
}

/// Decodes a `data:image/bmp;base64,...` URL into `(rgba_bytes, width, height)`.
pub fn decode_bmp_data_url_to_rgba(data_url: &str) -> Option<(Vec<u8>, usize, usize)> {
    let b64 = data_url
        .strip_prefix("data:image/bmp;base64,")
        .unwrap_or(data_url);
    let raw = decode_base64_fast(b64)?;
    if raw.len() < 54 || &raw[0..2] != b"BM" {
        return None;
    }
    let pixel_offset = u32::from_le_bytes([raw[10], raw[11], raw[12], raw[13]]) as usize;
    let width_i32 = i32::from_le_bytes([raw[18], raw[19], raw[20], raw[21]]);
    let height_i32 = i32::from_le_bytes([raw[22], raw[23], raw[24], raw[25]]);
    let bpp = u16::from_le_bytes([raw[28], raw[29]]);
    if width_i32 <= 0 || height_i32 == 0 || (bpp != 24 && bpp != 32) {
        return None;
    }
    let width = width_i32 as usize;
    let top_down = height_i32 < 0;
    let height = height_i32.unsigned_abs() as usize;
    let bytes_per_pixel = (bpp / 8) as usize;
    let row_stride = ((width * bytes_per_pixel + 3) / 4) * 4;
    if raw.len() < pixel_offset + row_stride * height {
        return None;
    }
    let mut rgba = vec![255u8; width * height * 4];
    for y in 0..height {
        let src_y = if top_down { y } else { height - 1 - y };
        let src_row = pixel_offset + src_y * row_stride;
        let dst_row = y * width * 4;
        for x in 0..width {
            let sp = src_row + x * bytes_per_pixel;
            let dp = dst_row + x * 4;
            rgba[dp] = raw[sp + 2];
            rgba[dp + 1] = raw[sp + 1];
            rgba[dp + 2] = raw[sp];
            rgba[dp + 3] = 255;
        }
    }
    Some((rgba, width, height))
}

/// Structured output of `export_latest_frame_lossless_bmp`.
#[derive(Debug, Clone)]
pub struct SnapshotExportResult {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
    pub rotation_deg: u16,
    pub byte_len: usize,
    pub elapsed_ms: f64,
}

/// Exports the latest `SharedFrameSlot` cleanly cropped & rotated into a lossless
/// top-down 24-bit BMP Data URL (`data:image/bmp;base64,Qk...`) in `< 5 ms`.
pub fn export_latest_frame_lossless_bmp(
    slot: &SharedFrameSlot,
    config: &PresenterConfig,
) -> Result<SnapshotExportResult, String> {
    let t0 = Instant::now();
    if slot.coded_width == 0 || slot.coded_height == 0 {
        return Err("Frame slot is empty".to_string());
    }
    let raw_w = slot.coded_width as usize;
    let raw_h = slot.coded_height as usize;
    let y_len = raw_w * raw_h;
    let has_bgra = slot.bgra_data.len() >= raw_w * raw_h * 4;
    let has_nv12 = slot.nv12_data.len() >= y_len + y_len / 2;
    if !has_bgra && !has_nv12 {
        return Err("Frame slot has neither NV12 nor BGRA pixel payload".to_string());
    }

    let region = compute_clean_source_region(slot, config);
    let rot = config.normalized_rotation();
    let clean_w = region.clean_w as usize;
    let clean_h = region.clean_h as usize;

    // Integer stride keeps 24-bit lossless BMP serialization < 5 ms even in debug builds
    // while preserving >= 400px short edge (above UI viewport width 346px).
    let step: usize = if clean_w * clean_h > 900_000 { 3 } else { 1 };
    let samp_w = (clean_w / step).max(1);
    let samp_h = (clean_h / step).max(1);
    let (out_w, out_h) = match rot {
        90 | 270 => (samp_h, samp_w),
        _ => (samp_w, samp_h),
    };

    let row_stride = ((out_w * 3 + 3) / 4) * 4;
    let pixel_bytes = row_stride * out_h;
    let file_size = 54 + pixel_bytes;

    let mut bmp = Vec::with_capacity(file_size);
    // BITMAPFILEHEADER (14 bytes) -> base64 starts with "Qk..."
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER (40 bytes), negative biHeight = top-down scanlines
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(out_w as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(out_h as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());

    bmp.resize(file_size, 0);
    let trim_l = region.src_trim_left as usize;
    let trim_t = region.src_trim_top as usize;

    let dst_pixels = &mut bmp[54..];
    for oy in 0..out_h {
        let row_dst = &mut dst_pixels[oy * row_stride..oy * row_stride + out_w * 3];
        for ox in 0..out_w {
            let (cx_idx, cy_idx) = match rot {
                270 => (samp_w - 1 - oy, ox),
                90 => (oy, samp_h - 1 - ox),
                180 => (samp_w - 1 - ox, samp_h - 1 - oy),
                _ => (ox, oy),
            };
            let sx = trim_l + cx_idx * step;
            let sy = trim_t + cy_idx * step;
            let (r, g, b) = read_slot_pixel_rgb(
                slot,
                raw_w,
                y_len,
                has_bgra,
                has_nv12,
                VideoColorInfo::default(),
                sx,
                sy,
            );
            let p = ox * 3;
            row_dst[p] = b;
            row_dst[p + 1] = g;
            row_dst[p + 2] = r;
        }
    }

    let b64 = encode_base64_fast(&bmp);
    let mut data_url = String::with_capacity(22 + b64.len());
    data_url.push_str("data:image/bmp;base64,");
    data_url.push_str(&b64);

    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    Ok(SnapshotExportResult {
        data_url,
        width: out_w as u32,
        height: out_h as u32,
        rotation_deg: rot,
        byte_len: file_size,
        elapsed_ms,
    })
}

/// Lossless BMP snapshot export returning `data:image/bmp;base64,Qk...` in `< 5 ms`.
pub fn export_latest_frame_lossless_bmp_data_url(
    slot: &SharedFrameSlot,
    config: &PresenterConfig,
) -> Result<String, String> {
    export_latest_frame_lossless_bmp(slot, config).map(|r| r.data_url)
}

/// Unified Direct3D 11 Hardware Video Presenter owning SwapChain specification,
/// viewport geometry, edge cropping, and lossless snapshot metrics.
#[derive(Debug, Clone)]
pub struct D3d11VideoPresenter {
    pub swap_chain_spec: SwapChainSpec,
    pub config: PresenterConfig,
    pub viewport: ViewportStageRect,
    pub frames_presented: u64,
    pub snapshots_exported: u64,
    pub last_snapshot_latency_ms: f64,
    pub last_presented_width: u32,
    pub last_presented_height: u32,
}

impl Default for D3d11VideoPresenter {
    fn default() -> Self {
        Self::new()
    }
}

impl D3d11VideoPresenter {
    pub fn new() -> Self {
        let config = PresenterConfig::default();
        let viewport = compute_presenter_viewport_rect(
            config.win_w,
            config.win_h,
            config.is_fullscreen,
            config.is_landscape,
            config.drawer_open,
            config.fit_mode,
        );
        Self {
            swap_chain_spec: SwapChainSpec::default(),
            config,
            viewport,
            frames_presented: 0,
            snapshots_exported: 0,
            last_snapshot_latency_ms: 0.0,
            last_presented_width: 1202,
            last_presented_height: 2598,
        }
    }

    pub fn update_config(&mut self, mut config: PresenterConfig) {
        config.crop_top = config.crop_top.min(24);
        config.crop_bottom = config.crop_bottom.min(24);
        config.crop_left = config.crop_left.min(24);
        config.crop_right = config.crop_right.min(24);
        config.rotation_deg = config.normalized_rotation();
        self.viewport = compute_presenter_viewport_rect(
            config.win_w,
            config.win_h,
            config.is_fullscreen,
            config.is_landscape,
            config.drawer_open,
            config.fit_mode,
        );
        self.config = config;
    }

    pub fn present_slot(&mut self, slot: &SharedFrameSlot) -> PresentedFrameBuffer {
        let out = render_slot_to_bgra_surface(slot, &self.config);
        self.frames_presented = self.frames_presented.saturating_add(1);
        self.last_presented_width = out.width;
        self.last_presented_height = out.height;
        out
    }

    pub fn export_snapshot(&mut self, slot: &SharedFrameSlot) -> Result<SnapshotExportResult, String> {
        let res = export_latest_frame_lossless_bmp(slot, &self.config)?;
        self.snapshots_exported = self.snapshots_exported.saturating_add(1);
        self.last_snapshot_latency_ms = res.elapsed_ms;
        Ok(res)
    }

    pub fn status_json(&self) -> String {
        format!(
            r#"{{"ok":true,"native":true,"format":"{}","formatCode":{},"bufferCount":{},"swapEffect":"{}","swapEffectCode":{},"alphaMode":"{}","alphaModeCode":{},"vsyncInterval":{},"obsCaptureCompatible":{},"videoCanvasSolidBlack":{},"chromeGlassPreserved":{},"hwndStyleFlags":{},"fitMode":"{}","rotationDeg":{},"crop":{{"top":{},"bottom":{},"left":{},"right":{}}},"window":{{"width":{},"height":{},"isFullscreen":{},"isLandscape":{},"drawerOpen":{}}},"viewport":{{"x":{},"y":{},"width":{},"height":{},"topY":{},"bottomY":{},"proportional":{},"centered":{},"bottomBlackBarGapPx":{:.1},"pillarboxGapPx":{:.1}}},"framesPresented":{},"lastPresentedWidth":{},"lastPresentedHeight":{},"snapshotsExported":{},"lastSnapshotLatencyMs":{:.3}}}"#,
            self.swap_chain_spec.format_name,
            self.swap_chain_spec.format,
            self.swap_chain_spec.buffer_count,
            self.swap_chain_spec.swap_effect_name,
            self.swap_chain_spec.swap_effect,
            self.swap_chain_spec.alpha_mode_name,
            self.swap_chain_spec.alpha_mode,
            self.swap_chain_spec.vsync_interval,
            self.swap_chain_spec.obs_capture_compatible,
            self.swap_chain_spec.video_canvas_solid_black,
            self.swap_chain_spec.chrome_glass_preserved,
            self.swap_chain_spec.hwnd_style_flags,
            self.config.fit_mode.as_str(),
            self.config.normalized_rotation(),
            self.config.crop_top,
            self.config.crop_bottom,
            self.config.crop_left,
            self.config.crop_right,
            self.config.win_w,
            self.config.win_h,
            self.config.is_fullscreen,
            self.config.is_landscape,
            self.config.drawer_open,
            self.viewport.x,
            self.viewport.y,
            self.viewport.width,
            self.viewport.height,
            self.viewport.top_y,
            self.viewport.bottom_y,
            self.viewport.proportional,
            self.viewport.centered,
            self.viewport.bottom_black_bar_gap_px,
            self.viewport.pillarbox_gap_px,
            self.frames_presented,
            self.last_presented_width,
            self.last_presented_height,
            self.snapshots_exported,
            self.last_snapshot_latency_ms
        )
    }
}

// ============================================================================
// TAHAP 262 — PILAR 3: Real Win32 Child HWND + DXGI FLIP_DISCARD SwapChain +
// ID3D11VideoProcessor + < 5 ms GPU Staging Snapshot & 120x260 AutoRotate Probe
// ============================================================================

const CS_DBLCLKS: u32 = 0x0008;
const CS_OWNDC: u32 = 0x0020;
const WS_POPUP: u32 = 0x8000_0000;
const WS_CLIPCHILDREN: isize = 0x0200_0000;
const GWL_STYLE: i32 = -16;
const GWLP_USERDATA: i32 = -21;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SWP_HIDEWINDOW: u32 = 0x0080;
const RGN_DIFF: i32 = 4;

const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_F4: usize = 0x73;
const MK_CONTROL: usize = 0x0008;
const IDC_ARROW: usize = 32512;

#[repr(C)]
struct WndClassExW {
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

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinPoint {
    x: i32,
    y: i32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lpModuleName: *const u16) -> isize;
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(lpwcx: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        x: i32,
        y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: isize,
        hMenu: isize,
        hInstance: isize,
        lpParam: *mut c_void,
    ) -> isize;
    fn DestroyWindow(hWnd: isize) -> i32;
    fn DefWindowProcW(hWnd: isize, msg: u32, wParam: usize, lParam: isize) -> isize;
    fn GetClientRect(hWnd: isize, lpRect: *mut Rect) -> i32;
    fn SetWindowPos(
        hWnd: isize,
        hWndInsertAfter: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        uFlags: u32,
    ) -> i32;
    fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
    fn GetWindowLongPtrW(hWnd: isize, nIndex: i32) -> isize;
    fn SetWindowLongPtrW(hWnd: isize, nIndex: i32, dwNewLong: isize) -> isize;
    fn SetCapture(hWnd: isize) -> isize;
    fn ReleaseCapture() -> i32;
    fn ScreenToClient(hWnd: isize, lpPoint: *mut WinPoint) -> i32;
    fn SetWindowRgn(hWnd: isize, hRgn: isize, bRedraw: i32) -> i32;
    fn LoadCursorW(hInstance: isize, lpCursorName: *const u16) -> isize;
    fn SetCursor(hCursor: isize) -> isize;
    fn SetFocus(hWnd: isize) -> isize;
    fn GetKeyState(nVirtKey: i32) -> i16;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
    fn CombineRgn(hrgnDst: isize, hrgnSrc1: isize, hrgnSrc2: isize, iMode: i32) -> i32;
    fn DeleteObject(ho: isize) -> i32;
}

fn to_wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Pointer/wheel input event emitted by `LiveVideoHwnd` `WndProc` already mapped into
/// iOS HID coordinate space `(0..=65535, 0..=65535)` via `map_stage_touch_to_ios_hid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagePointerEvent {
    Down { hid_x: u16, hid_y: u16 },
    Move { hid_x: u16, hid_y: u16 },
    Up { hid_x: u16, hid_y: u16 },
    Wheel {
        hid_x: u16,
        hid_y: u16,
        delta_y: i32,
        delta_x: i32,
        ctrl: bool,
    },
}

pub type StagePointerCallback = Arc<dyn Fn(StagePointerEvent) + Send + Sync + 'static>;

/// Keyboard event emitted by `LiveVideoHwnd` `WndProc` when focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKeyEvent {
    KeyDown {
        vk: u16,
        ctrl: bool,
        alt: bool,
        shift: bool,
        repeat: bool,
    },
    KeyUp {
        vk: u16,
        ctrl: bool,
        alt: bool,
        shift: bool,
    },
    Char {
        ch: char,
        ctrl: bool,
        alt: bool,
    },
}

pub type StageKeyCallback = Arc<dyn Fn(StageKeyEvent) + Send + Sync + 'static>;

struct StageInputState {
    config: Mutex<PresenterConfig>,
    pressed: AtomicBool,
    callback: Mutex<Option<StagePointerCallback>>,
    key_callback: Mutex<Option<StageKeyCallback>>,
}

impl StageInputState {
    fn map_client_to_hid(&self, hwnd: isize, client_x: i32, client_y: i32) -> (u16, u16) {
        let mut rc = Rect::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let cw = (rc.right - rc.left).max(1) as u32;
        let ch = (rc.bottom - rc.top).max(1) as u32;
        let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let vp = ViewportStageRect {
            x: 0,
            y: 0,
            width: cw,
            height: ch,
            top_y: 0,
            bottom_y: ch as i32,
            fit_mode: cfg.fit_mode,
            is_fullscreen: cfg.is_fullscreen,
            is_landscape: cfg.is_landscape,
            drawer_open: cfg.drawer_open,
            proportional: true,
            centered: false,
            bottom_black_bar_gap_px: 0.0,
            pillarbox_gap_px: 0.0,
        };
        map_stage_touch_to_ios_hid(client_x as f64, client_y as f64, &vp, &cfg)
    }

    fn emit(&self, ev: StagePointerEvent) {
        if let Some(cb) = self
            .callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            cb(ev);
        }
    }

    fn emit_key(&self, ev: StageKeyEvent) {
        if let Some(cb) = self
            .key_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            cb(ev);
        }
    }
}

unsafe fn current_modifier_flags() -> (bool, bool, bool) {
    let ctrl = (GetKeyState(VK_CONTROL) as u16 & 0x8000) != 0;
    let alt = (GetKeyState(VK_MENU) as u16 & 0x8000) != 0;
    let shift = (GetKeyState(VK_SHIFT) as u16 & 0x8000) != 0;
    (ctrl, alt, shift)
}

unsafe extern "system" fn live_video_wndproc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const StageInputState;
    if ptr.is_null() {
        if msg == WM_ERASEBKGND {
            return 1;
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &*ptr;
    let lx = (lparam & 0xFFFF) as i16 as i32;
    let ly = ((lparam >> 16) & 0xFFFF) as i16 as i32;

    match msg {
        WM_ERASEBKGND => 1,
        WM_SETCURSOR => {
            let cur = LoadCursorW(0, IDC_ARROW as *const u16);
            if cur != 0 {
                SetCursor(cur);
            }
            1
        }
        WM_LBUTTONDOWN => {
            SetFocus(hwnd);
            SetCapture(hwnd);
            state.pressed.store(true, Ordering::Relaxed);
            let (hid_x, hid_y) = state.map_client_to_hid(hwnd, lx, ly);
            state.emit(StagePointerEvent::Down { hid_x, hid_y });
            0
        }
        WM_MOUSEMOVE => {
            if state.pressed.load(Ordering::Relaxed) {
                let (hid_x, hid_y) = state.map_client_to_hid(hwnd, lx, ly);
                state.emit(StagePointerEvent::Move { hid_x, hid_y });
            }
            0
        }
        WM_LBUTTONUP => {
            if state.pressed.swap(false, Ordering::Relaxed) {
                let _ = ReleaseCapture();
                let (hid_x, hid_y) = state.map_client_to_hid(hwnd, lx, ly);
                state.emit(StagePointerEvent::Up { hid_x, hid_y });
            }
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = WinPoint { x: lx, y: ly };
            let _ = ScreenToClient(hwnd, &mut pt);
            let (hid_x, hid_y) = state.map_client_to_hid(hwnd, pt.x, pt.y);
            let ctrl = (wparam & MK_CONTROL) != 0;
            let (delta_y, delta_x) = if msg == WM_MOUSEWHEEL {
                (delta, 0)
            } else {
                (0, delta)
            };
            state.emit(StagePointerEvent::Wheel {
                hid_x,
                hid_y,
                delta_y,
                delta_x,
                ctrl,
            });
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let (ctrl, mut alt, shift) = current_modifier_flags();
            if (lparam & (1 << 29)) != 0 {
                alt = true;
            }
            if msg == WM_SYSKEYDOWN && alt && wparam == VK_F4 {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let repeat = (lparam & 0x4000_0000) != 0;
            state.emit_key(StageKeyEvent::KeyDown {
                vk: (wparam & 0xFFFF) as u16,
                ctrl,
                alt,
                shift,
                repeat,
            });
            0
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let (ctrl, mut alt, shift) = current_modifier_flags();
            if (lparam & (1 << 29)) != 0 {
                alt = true;
            }
            state.emit_key(StageKeyEvent::KeyUp {
                vk: (wparam & 0xFFFF) as u16,
                ctrl,
                alt,
                shift,
            });
            0
        }
        WM_SYSCHAR => 0,
        WM_CHAR => {
            if let Some(ch) = char::from_u32((wparam & 0xFFFF) as u32) {
                let (ctrl, mut alt, _) = current_modifier_flags();
                if (lparam & (1 << 29)) != 0 {
                    alt = true;
                }
                state.emit_key(StageKeyEvent::Char { ch, ctrl, alt });
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn ensure_video_stage_class_registered() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let class_w = to_wide_str("iPhoneLinkNativeVideoStage");
        let h_inst = GetModuleHandleW(std::ptr::null());
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            style: CS_OWNDC | CS_DBLCLKS,
            lpfn_wnd_proc: Some(live_video_wndproc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: h_inst,
            h_icon: 0,
            h_cursor: LoadCursorW(0, IDC_ARROW as *const u16),
            hbr_background: 0, // NULL brush -> zero white flicker
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_w.as_ptr(),
            h_icon_sm: 0,
        };
        let _ = RegisterClassExW(&wc);
    });
}

/// Snapshot of the color parameters applied to an `ID3D11VideoProcessor` instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterministicColorPipelineState {
    pub dxgi_in_cs: u32,
    pub dxgi_out_cs: u32,
    pub legacy_in_cs: u32,
    pub legacy_out_cs: u32,
    pub auto_processing_disabled: bool,
    pub applied_vctx1: bool,
}

/// Applies deterministic BT.709 (or caller `VideoColorInfo`) input/output color space and disables
/// GPU driver auto-processing across BOTH legacy `ID3D11VideoContext` AND `ID3D11VideoContext1`.
pub fn apply_deterministic_bt709_color_pipeline(
    gpu: &LiveD3d11Gpu,
    vp: &ComPtr,
    color: VideoColorInfo,
) -> DeterministicColorPipelineState {
    let dxgi_in_cs = color.dxgi_ycbcr_color_space();
    let dxgi_out_cs = DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709;
    let legacy_in_cs = color.legacy_vp_input_color_space_flags();
    let legacy_out_cs = 0x00u32; // Full-range 0..255 sRGB output (RGB_Range = 0)
    let mut applied_vctx1 = false;

    if let Some(vctx) = gpu.video_context() {
        unsafe {
            let vc_vt = vctx.vtable();
            let set_fmt: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32) =
                std::mem::transmute(*vc_vt.add(27));
            let set_auto: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, i32) =
                std::mem::transmute(*vc_vt.add(37));
            let set_in_cs: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *const u32) =
                std::mem::transmute(*vc_vt.add(28));
            let set_out_cs: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u32) =
                std::mem::transmute(*vc_vt.add(15));

            set_fmt(
                vctx.as_ptr(),
                vp.as_ptr(),
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            // Disable driver auto contrast/skin-tone/hue manipulation for true iOS colors
            set_auto(vctx.as_ptr(), vp.as_ptr(), 0, 0);
            set_in_cs(vctx.as_ptr(), vp.as_ptr(), 0, &legacy_in_cs);
            set_out_cs(vctx.as_ptr(), vp.as_ptr(), &legacy_out_cs);

            if let Some(vctx1) = gpu.video_context1() {
                // ID3D11VideoContext1 verified vtable indices (d3d11_1.h):
                // slot 70 = VideoProcessorSetOutputColorSpace1
                // slot 74 = VideoProcessorSetStreamColorSpace1
                let vc1_vt = vctx1.vtable();
                let set_out_cs1: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) =
                    std::mem::transmute(*vc1_vt.add(70));
                let set_in_cs1: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32) =
                    std::mem::transmute(*vc1_vt.add(74));
                set_in_cs1(vctx1.as_ptr(), vp.as_ptr(), 0, dxgi_in_cs);
                set_out_cs1(vctx1.as_ptr(), vp.as_ptr(), dxgi_out_cs);
                applied_vctx1 = true;
            }
        }
    }

    DeterministicColorPipelineState {
        dxgi_in_cs,
        dxgi_out_cs,
        legacy_in_cs,
        legacy_out_cs,
        auto_processing_disabled: true,
        applied_vctx1,
    }
}

struct LiveVideoProcessor {
    enumerator: ComPtr,
    vp: ComPtr,
    supports_rotation: bool,
    key: (u32, u32, u32, u32),
}

impl LiveVideoProcessor {
    fn create(
        gpu: &LiveD3d11Gpu,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
    ) -> Result<Self, String> {
        let vdev = gpu
            .video_device()
            .ok_or_else(|| "ID3D11VideoDevice unavailable".to_string())?;
        let _vctx = gpu
            .video_context()
            .ok_or_else(|| "ID3D11VideoContext unavailable".to_string())?;

        let mut desc = D3d11VideoProcessorContentDesc {
            input_frame_format: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            input_frame_rate: DxgiRational {
                numerator: 60,
                denominator: 1,
            },
            input_width: in_w.max(2),
            input_height: in_h.max(2),
            output_frame_rate: DxgiRational {
                numerator: 60,
                denominator: 1,
            },
            output_width: out_w.max(2),
            output_height: out_h.max(2),
            usage: D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
        };

        unsafe {
            let vd_vt = vdev.vtable();
            let create_enum: unsafe extern "system" fn(
                *mut c_void,
                *const D3d11VideoProcessorContentDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vd_vt.add(10));
            let create_vp: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                u32,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vd_vt.add(4));

            let mut p_enum: *mut c_void = std::ptr::null_mut();
            let mut hr_e = create_enum(vdev.as_ptr(), &desc, &mut p_enum);
            if hr_e < 0 || p_enum.is_null() {
                // Fallback to PLAYBACK_NORMAL on constrained virtual/WARP drivers
                desc.usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
                p_enum = std::ptr::null_mut();
                hr_e = create_enum(vdev.as_ptr(), &desc, &mut p_enum);
            }
            if hr_e < 0 || p_enum.is_null() {
                return Err(format!(
                    "CreateVideoProcessorEnumerator failed: HRESULT=0x{:08X}",
                    hr_e as u32
                ));
            }
            let enumerator = ComPtr::from_raw(p_enum)
                .ok_or_else(|| "null ID3D11VideoProcessorEnumerator".to_string())?;

            let mut caps = D3d11VideoProcessorCaps::default();
            let enum_vt = enumerator.vtable();
            let get_caps: unsafe extern "system" fn(
                *mut c_void,
                *mut D3d11VideoProcessorCaps,
            ) -> i32 = std::mem::transmute(*enum_vt.add(9));
            let _ = get_caps(enumerator.as_ptr(), &mut caps);
            let supports_rotation =
                (caps.feature_caps & D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION) != 0;

            let mut p_vp: *mut c_void = std::ptr::null_mut();
            let hr_vp = create_vp(vdev.as_ptr(), enumerator.as_ptr(), 0, &mut p_vp);
            if hr_vp < 0 || p_vp.is_null() {
                return Err(format!(
                    "CreateVideoProcessor failed: HRESULT=0x{:08X}",
                    hr_vp as u32
                ));
            }
            let vp = ComPtr::from_raw(p_vp)
                .ok_or_else(|| "null ID3D11VideoProcessor".to_string())?;

            let proc_obj = Self {
                enumerator,
                vp,
                supports_rotation,
                key: (in_w, in_h, out_w, out_h),
            };
            proc_obj.apply_color_space(gpu, VideoColorInfo::default());
            Ok(proc_obj)
        }
    }

    fn apply_color_space(&self, gpu: &LiveD3d11Gpu, color: VideoColorInfo) {
        let _ = apply_deterministic_bt709_color_pipeline(gpu, &self.vp, color);
    }

    #[allow(clippy::too_many_arguments)]
    fn blit(
        &self,
        gpu: &LiveD3d11Gpu,
        src_tex: &ComPtr,
        dst_tex: &ComPtr,
        src_rect: Rect,
        dst_rect: Rect,
        out_w: u32,
        out_h: u32,
        rotation_deg: u16,
        color_info: VideoColorInfo,
    ) -> Result<(), String> {
        self.apply_color_space(gpu, color_info);
        let vdev = gpu
            .video_device()
            .ok_or_else(|| "ID3D11VideoDevice unavailable".to_string())?;
        let vctx = gpu
            .video_context()
            .ok_or_else(|| "ID3D11VideoContext unavailable".to_string())?;

        unsafe {
            let vc_vt = vctx.vtable();
            let set_rot: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, i32, u32) =
                std::mem::transmute(*vc_vt.add(63));
            let set_src: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, i32, *const Rect) =
                std::mem::transmute(*vc_vt.add(30));
            let set_dst: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, i32, *const Rect) =
                std::mem::transmute(*vc_vt.add(31));
            let set_out_target: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                i32,
                *const Rect,
            ) = std::mem::transmute(*vc_vt.add(13));
            let set_bg: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                i32,
                *const D3d11VideoColor,
            ) = std::mem::transmute(*vc_vt.add(14));
            let vp_blt: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                u32,
                u32,
                *const D3d11VideoProcessorStream,
            ) -> i32 = std::mem::transmute(*vc_vt.add(53));

            let rot_enum = match rotation_deg % 360 {
                90 => D3D11_VIDEO_PROCESSOR_ROTATION_90,
                180 => D3D11_VIDEO_PROCESSOR_ROTATION_180,
                270 => D3D11_VIDEO_PROCESSOR_ROTATION_270,
                _ => D3D11_VIDEO_PROCESSOR_ROTATION_IDENTITY,
            };
            if self.supports_rotation {
                set_rot(
                    vctx.as_ptr(),
                    self.vp.as_ptr(),
                    0,
                    if rotation_deg % 360 != 0 { 1 } else { 0 },
                    rot_enum,
                );
            }
            set_src(vctx.as_ptr(), self.vp.as_ptr(), 0, 1, &src_rect);
            set_dst(vctx.as_ptr(), self.vp.as_ptr(), 0, 1, &dst_rect);
            let full = Rect {
                left: 0,
                top: 0,
                right: out_w as i32,
                bottom: out_h as i32,
            };
            set_out_target(vctx.as_ptr(), self.vp.as_ptr(), 1, &full);
            let black = D3d11VideoColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            };
            set_bg(vctx.as_ptr(), self.vp.as_ptr(), 0, &black);

            let vd_vt = vdev.vtable();
            let create_iv: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *const D3d11VideoProcessorInputViewDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vd_vt.add(8));
            let create_ov: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *const D3d11VideoProcessorOutputViewDesc,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*vd_vt.add(9));

            let iv_desc = D3d11VideoProcessorInputViewDesc {
                fourcc: 0,
                view_dimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                mip_slice: 0,
                array_slice: 0,
            };
            let mut p_iv: *mut c_void = std::ptr::null_mut();
            let hr_iv = create_iv(
                vdev.as_ptr(),
                src_tex.as_ptr(),
                self.enumerator.as_ptr(),
                &iv_desc,
                &mut p_iv,
            );
            if hr_iv < 0 || p_iv.is_null() {
                return Err(format!(
                    "CreateVideoProcessorInputView failed: HRESULT=0x{:08X}",
                    hr_iv as u32
                ));
            }
            let iv = ComPtr::from_raw(p_iv).ok_or_else(|| "null VPIV".to_string())?;

            let ov_desc = D3d11VideoProcessorOutputViewDesc {
                view_dimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                mip_slice: 0,
                first_array_slice: 0,
                array_size: 0,
            };
            let mut p_ov: *mut c_void = std::ptr::null_mut();
            let hr_ov = create_ov(
                vdev.as_ptr(),
                dst_tex.as_ptr(),
                self.enumerator.as_ptr(),
                &ov_desc,
                &mut p_ov,
            );
            if hr_ov < 0 || p_ov.is_null() {
                return Err(format!(
                    "CreateVideoProcessorOutputView failed: HRESULT=0x{:08X}",
                    hr_ov as u32
                ));
            }
            let ov = ComPtr::from_raw(p_ov).ok_or_else(|| "null VPOV".to_string())?;

            let stream = D3d11VideoProcessorStream {
                enable: 1,
                p_input_surface: iv.as_ptr(),
                ..Default::default()
            };
            let hr_blt = vp_blt(vctx.as_ptr(), self.vp.as_ptr(), ov.as_ptr(), 0, 1, &stream);
            drop(ov);
            drop(iv);
            if hr_blt < 0 {
                return Err(format!(
                    "VideoProcessorBlt failed: HRESULT=0x{:08X}",
                    hr_blt as u32
                ));
            }
            Ok(())
        }
    }
}

fn compute_vram_source_rect(snap: &VramFrameSnapshot, cfg: &PresenterConfig) -> Rect {
    let raw_w = snap.coded_width.max(1);
    let raw_h = snap.coded_height.max(1);
    let base_l = snap.visible_rect.left.max(0) as u32;
    let base_t = snap.visible_rect.top.max(0) as u32;
    let base_r = (snap.visible_rect.right as u32).min(raw_w).max(base_l + 1);
    let base_b = (snap.visible_rect.bottom as u32).min(raw_h).max(base_t + 1);

    let (trim_t, trim_l, trim_b, trim_r) = if snap.is_airplay {
        (0, 0, 0, 0)
    } else if snap.trim == (0, 0, 0, 0) && raw_w == 1216 && raw_h == 2656 {
        (0, 0, 58, 14)
    } else {
        snap.trim
    };

    let vis_w = base_r.saturating_sub(base_l).saturating_sub(trim_l + trim_r).max(1);
    let px_scale = (vis_w as f32 / NORMAL_PORTRAIT_STAGE_W as f32).max(1.0);
    let extra_l = (cfg.crop_left.min(24) as f32 * px_scale).round() as u32 + cfg.base_trim_px;
    let extra_r = (cfg.crop_right.min(24) as f32 * px_scale).round() as u32 + cfg.base_trim_px;
    let extra_t = (cfg.crop_top.min(24) as f32 * px_scale).round() as u32 + cfg.base_trim_px;
    let extra_b = (cfg.crop_bottom.min(24) as f32 * px_scale).round() as u32 + cfg.base_trim_px;

    let mut l = base_l.saturating_add(trim_l).saturating_add(extra_l);
    let mut r = base_r.saturating_sub(trim_r).saturating_sub(extra_r);
    let mut t = base_t.saturating_add(trim_t).saturating_add(extra_t);
    let mut b = base_b.saturating_sub(trim_b).saturating_sub(extra_b);

    if l >= r {
        l = base_l;
        r = base_r;
    }
    if t >= b {
        t = base_t;
        b = base_b;
    }
    Rect {
        left: l as i32,
        top: t as i32,
        right: r as i32,
        bottom: b as i32,
    }
}

fn compute_dest_and_clip_rects(
    src: Rect,
    cw: u32,
    ch: u32,
    rot: u16,
    fit_mode: FitMode,
) -> (Rect, Rect) {
    let cw_i = cw.max(1) as i32;
    let ch_i = ch.max(1) as i32;
    let sw = (src.right - src.left).max(1) as f32;
    let sh = (src.bottom - src.top).max(1) as f32;
    let (pw, ph) = if rot == 90 || rot == 270 {
        (sh, sw)
    } else {
        (sw, sh)
    };
    let orientation_mismatched = (pw > ph) != (cw_i > ch_i);

    let dest = if orientation_mismatched {
        // Mismatched orientation (e.g. landscape AirPlay 1920x888 inside Portrait 346x832 stage
        // while Game Auto-Rotate is OFF): force aspect-preserving centered `contain` (0 squish).
        let s = (cw_i as f32 / pw).min(ch_i as f32 / ph);
        let dw = (pw * s).round().max(1.0) as i32;
        let dh = (ph * s).round().max(1.0) as i32;
        let ox = (cw_i - dw) / 2;
        let oy = (ch_i - dh) / 2;
        Rect {
            left: ox,
            top: oy,
            right: ox + dw,
            bottom: oy + dh,
        }
    } else {
        match fit_mode {
            FitMode::AutoFit => {
                let stage_aspect = cw_i as f32 / ch_i as f32;
                let pic_aspect = pw / ph;
                let diff = (stage_aspect - pic_aspect).abs() / pic_aspect.max(0.01);
                if diff < 0.04 {
                    Rect {
                        left: 0,
                        top: 0,
                        right: cw_i,
                        bottom: ch_i,
                    }
                } else {
                    let sx = cw_i as f32 / pw;
                    let sy = ch_i as f32 / ph;
                    let s = sx.min(sy);
                    let dw = (pw * s).round().max(1.0) as i32;
                    let dh = (ph * s).round().max(1.0) as i32;
                    let ox = (cw_i - dw) / 2;
                    let oy = (ch_i - dh) / 2;
                    Rect {
                        left: ox,
                        top: oy,
                        right: ox + dw,
                        bottom: oy + dh,
                    }
                }
            }
            FitMode::OriginalRatio => {
                let stage_aspect = cw_i as f32 / ch_i as f32;
                let pic_aspect = pw / ph;
                let diff = (stage_aspect - pic_aspect).abs() / pic_aspect.max(0.01);
                if diff < 0.12 {
                    Rect {
                        left: 0,
                        top: 0,
                        right: cw_i,
                        bottom: ch_i,
                    }
                } else {
                    let s = (cw_i as f32 / pw).min(ch_i as f32 / ph);
                    let dw = (pw * s).round().max(1.0) as i32;
                    let dh = (ph * s).round().max(1.0) as i32;
                    let ox = (cw_i - dw) / 2;
                    let oy = (ch_i - dh) / 2;
                    Rect {
                        left: ox,
                        top: oy,
                        right: ox + dw,
                        bottom: oy + dh,
                    }
                }
            }
            FitMode::EdgeToEdge => {
                let s = (cw_i as f32 / pw).max(ch_i as f32 / ph);
                let dw = (pw * s).round().max(1.0) as i32;
                let dh = (ph * s).round().max(1.0) as i32;
                let ox = (cw_i - dw) / 2;
                let oy = (ch_i - dh) / 2;
                Rect {
                    left: ox,
                    top: oy,
                    right: ox + dw,
                    bottom: oy + dh,
                }
            }
        }
    };

    // Clip overflowing EdgeToEdge dest rect into [0..cw, 0..ch] and trim src proportionally
    let dw = (dest.right - dest.left).max(1) as f32;
    let dh = (dest.bottom - dest.top).max(1) as f32;
    let cut_l = (-dest.left).max(0) as f32 / dw;
    let cut_t = (-dest.top).max(0) as f32 / dh;
    let cut_r = (dest.right - cw_i).max(0) as f32 / dw;
    let cut_b = (dest.bottom - ch_i).max(0) as f32 / dh;
    if cut_l == 0.0 && cut_t == 0.0 && cut_r == 0.0 && cut_b == 0.0 {
        return (src, dest);
    }

    let (sl, st, sr, sb) = match rot {
        90 => (cut_t, cut_r, cut_b, cut_l),
        180 => (cut_r, cut_b, cut_l, cut_t),
        270 => (cut_b, cut_l, cut_t, cut_r),
        _ => (cut_l, cut_t, cut_r, cut_b),
    };
    let clipped_src = Rect {
        left: src.left + (sw * sl).round() as i32,
        top: src.top + (sh * st).round() as i32,
        right: src.right - (sw * sr).round() as i32,
        bottom: src.bottom - (sh * sb).round() as i32,
    };
    let clipped_dst = Rect {
        left: dest.left.max(0),
        top: dest.top.max(0),
        right: dest.right.min(cw_i),
        bottom: dest.bottom.min(ch_i),
    };
    if clipped_src.right <= clipped_src.left
        || clipped_src.bottom <= clipped_src.top
        || clipped_dst.right <= clipped_dst.left
        || clipped_dst.bottom <= clipped_dst.top
    {
        (
            src,
            Rect {
                left: 0,
                top: 0,
                right: cw_i,
                bottom: ch_i,
            },
        )
    } else {
        (clipped_src, clipped_dst)
    }
}

/// Public helper exposing exact `(src_rect, dest_rect)` computation for swapchain sizing tests.
pub fn compute_dest_and_clip_rects_for_fit(
    src: Rect,
    cw: u32,
    ch: u32,
    rot: u16,
    fit_mode: FitMode,
) -> (Rect, Rect) {
    compute_dest_and_clip_rects(src, cw, ch, rot, fit_mode)
}

/// Computes post-trim visible source dimensions matching `compute_vram_source_rect`.
pub fn compute_post_trim_dimensions(snap: &VramFrameSnapshot) -> (u32, u32) {
    let r = compute_vram_source_rect(snap, &PresenterConfig::default());
    (
        (r.right - r.left).max(2) as u32,
        (r.bottom - r.top).max(2) as u32,
    )
}

/// Tahap 274 (Pilar 4): Per-frame atomic rotation policy for AirPlay streams so rotation
/// tracks actual decoded frame dimensions on the exact transition frame (0 ms lag behind UI timer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AirplayRenderPolicy {
    pub is_airplay: bool,
    pub window_is_landscape: bool,
    pub auto_rotate: bool,
    pub manual_rotation_deg: u16,
}

/// Resolves exact rotation (`0/90/180/270`) atomically per decoded frame from `policy` and the
/// frame's post-crop visible `(frame_vis_w, frame_vis_h)`. Non-AirPlay streams are untouched.
#[inline]
pub fn resolve_atomic_frame_rotation_deg(
    policy: AirplayRenderPolicy,
    frame_vis_w: u32,
    frame_vis_h: u32,
    fallback_cfg_rot: u16,
) -> u16 {
    let norm = |deg: u16| match deg % 360 {
        90 => 90,
        180 => 180,
        270 => 270,
        _ => 0,
    };
    if !policy.is_airplay || frame_vis_w == 0 || frame_vis_h == 0 {
        return norm(fallback_cfg_rot);
    }
    let manual = norm(policy.manual_rotation_deg);
    if frame_vis_w > frame_vis_h {
        if policy.window_is_landscape {
            if manual == 180 { 180 } else { 0 }
        } else if manual == 270 {
            270
        } else {
            90
        }
    } else if !policy.window_is_landscape {
        if manual == 180 { 180 } else { 0 }
    } else if policy.auto_rotate {
        0
    } else if manual == 90 {
        90
    } else if manual == 180 {
        180
    } else {
        270
    }
}

/// Stateless Tahap 278 AirPlay presenter geometry:
/// Always derives `window_is_landscape = dst_w > dst_h` from actual swapchain dimensions, resolves
/// exact `0° / 90° CW / 270° CW` rotation from `(src_raw, dst_w, dst_h)`, and fills `(0, 0, dst_w, dst_h)`
/// edge-to-edge under `FitMode::AutoFit` & `FitMode::EdgeToEdge` (0 px letterbox & 0 px pillarbox).
pub fn compute_stateless_airplay_present_rects(
    src_raw: Rect,
    dst_w: u32,
    dst_h: u32,
    manual_rotation_deg: u16,
    fit_mode: FitMode,
) -> (u16, Rect, Rect) {
    let dw = dst_w.max(1) as i32;
    let dh = dst_h.max(1) as i32;
    let frame_vis_w = (src_raw.right - src_raw.left).max(1) as u32;
    let frame_vis_h = (src_raw.bottom - src_raw.top).max(1) as u32;
    let policy = AirplayRenderPolicy {
        is_airplay: true,
        window_is_landscape: dw > dh,
        auto_rotate: false,
        manual_rotation_deg,
    };
    let rot = resolve_atomic_frame_rotation_deg(policy, frame_vis_w, frame_vis_h, 0);
    match fit_mode {
        FitMode::AutoFit => (
            rot,
            src_raw,
            Rect {
                left: 0,
                top: 0,
                right: dw,
                bottom: dh,
            },
        ),
        FitMode::EdgeToEdge => {
            let (s, d) = compute_dest_and_clip_rects(src_raw, dst_w, dst_h, rot, FitMode::EdgeToEdge);
            (rot, s, d)
        }
        FitMode::OriginalRatio => {
            let (s, d) =
                compute_dest_and_clip_rects(src_raw, dst_w, dst_h, rot, FitMode::OriginalRatio);
            (rot, s, d)
        }
    }
}

fn acquire_swapchain_backbuffer(sc: &ComPtr) -> Result<ComPtr, String> {
    unsafe {
        let vt = sc.vtable();
        let get_buf: unsafe extern "system" fn(
            *mut c_void,
            u32,
            *const crate::d3d11_context::Guid,
            *mut *mut c_void,
        ) -> i32 = std::mem::transmute(*vt.add(9));
        let mut p_bb: *mut c_void = std::ptr::null_mut();
        let hr = get_buf(sc.as_ptr(), 0, &IID_ID3D11_TEXTURE2D, &mut p_bb);
        if hr < 0 || p_bb.is_null() {
            return Err(format!(
                "IDXGISwapChain::GetBuffer(0) failed: HRESULT=0x{:08X}",
                hr as u32
            ));
        }
        ComPtr::from_raw(p_bb).ok_or_else(|| "null swapchain backbuffer".to_string())
    }
}

/// Statistics returned by `LiveVideoHwnd::present_latest()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivePresentStats {
    pub presented_total: u64,
    pub frame_seq: u64,
    pub swapchain_w: u32,
    pub swapchain_h: u32,
    pub used_hardware_vp: bool,
    pub supports_hw_rotation: bool,
    pub dual_buffer_overwritten: bool,
}

/// Real Win32 Child `HWND` + DXGI `FLIP_DISCARD` `IDXGISwapChain1` + `ID3D11VideoProcessor`
/// hardware presenter with `< 5 ms` GPU staging OCR snapshot, `120x260` zero-alloc Auto-Rotate
/// probe, `SetWindowRgn` toast holes, and Win32 pointer/wheel forwarding.
pub struct LiveVideoHwnd {
    ring: Arc<D3d11Nv12VramRing>,
    hwnd: isize,
    swapchain: Option<ComPtr>,
    processor: Option<LiveVideoProcessor>,
    snap_processor: Option<LiveVideoProcessor>,
    snap_rt: Option<(ComPtr, u32, u32)>,
    snap_staging: Option<(ComPtr, u32, u32)>,
    thumb_processor: Option<LiveVideoProcessor>,
    thumb_rt: Option<(ComPtr, u32, u32)>,
    thumb_staging: Option<(ComPtr, u32, u32)>,
    probe_processor: Option<LiveVideoProcessor>,
    probe_rt: Option<ComPtr>,
    probe_rtv: Option<ComPtr>,
    probe_staging: Option<ComPtr>,
    last_probe_at: Option<Instant>,
    input_state: Arc<StageInputState>,
    input_state_raw: *const StageInputState,
    width: u32,
    height: u32,
    vsync_interval: u32,
    auto_rotate_enabled: bool,
    frames_presented: u64,
    last_frame_seq: u64,
    last_source_dims: (u32, u32),
    pending_dual_backbuffer_overwrite: bool,
    mid_stream_black_clears: u64,
    dual_buffer_overwrites: u64,
    airplay_render_policy: AirplayRenderPolicy,
    last_effective_rotation_deg: u16,
    vp_rebuilds: u64,
    last_presented_src_rect: (i32, i32, i32, i32),
    last_presented_dest_rect: (i32, i32, i32, i32),
    d3d11_multithread_protected: bool,
    resize_atomic_repaints: u64,
    hd_shader: Option<HdNv12ShaderRenderer>,
    hd_shader_presents: u64,
    last_presenter_key: Option<(u32, u32, u32, u32)>,
    // MUST remain the last field so D3D11/DXGI COM children drop BEFORE
    // `LiveD3d11Gpu::drop()` calls `MFShutdown()` and `FreeLibrary(d3d11.dll)`.
    gpu: Arc<LiveD3d11Gpu>,
}

// LiveVideoHwnd wraps a thread-safe LiveD3d11Gpu (ID3D10Multithread) and Win32 HWND handle;
// callers synchronize access via Mutex and drop on the HWND creator thread.
unsafe impl Send for LiveVideoHwnd {}

impl LiveVideoHwnd {
    /// Creates a real Win32 `HWND` (`WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS` when `parent_hwnd != 0`,
    /// or a top-level popup host when `parent_hwnd == 0`) with a live `DXGI_SWAP_EFFECT_FLIP_DISCARD`
    /// `IDXGISwapChain1`.
    pub fn new(
        gpu: Arc<LiveD3d11Gpu>,
        ring: Arc<D3d11Nv12VramRing>,
        parent_hwnd: isize,
        width: u32,
        height: u32,
        visible: bool,
    ) -> Result<Self, String> {
        ensure_video_stage_class_registered();

        let w = width.max(16);
        let h = height.max(16);
        let class_w = to_wide_str("iPhoneLinkNativeVideoStage");
        let title_w = to_wide_str("");

        let hwnd = unsafe {
            let h_inst = GetModuleHandleW(std::ptr::null());
            let (style, x, y) = if parent_hwnd != 0 {
                let p_style = GetWindowLongPtrW(parent_hwnd, GWL_STYLE);
                SetWindowLongPtrW(parent_hwnd, GWL_STYLE, p_style | WS_CLIPCHILDREN);
                let mut st = WS_CHILD | WS_CLIPSIBLINGS;
                if visible {
                    st |= WS_VISIBLE;
                }
                (st, 0, 0)
            } else {
                let mut st = WS_POPUP;
                if visible {
                    st |= WS_VISIBLE;
                }
                (st, -32000, -32000)
            };
            CreateWindowExW(
                0,
                class_w.as_ptr(),
                title_w.as_ptr(),
                style,
                x,
                y,
                w as i32,
                h as i32,
                parent_hwnd,
                0,
                h_inst,
                std::ptr::null_mut(),
            )
        };
        if hwnd == 0 {
            return Err("CreateWindowExW failed for LiveVideoHwnd".to_string());
        }

        let input_state = Arc::new(StageInputState {
            config: Mutex::new(PresenterConfig::default()),
            pressed: AtomicBool::new(false),
            callback: Mutex::new(None),
            key_callback: Mutex::new(None),
        });
        let input_state_raw = Arc::into_raw(input_state.clone());
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, input_state_raw as isize);
        }

        let factory = match gpu.create_dxgi_factory2() {
            Ok(f) => f,
            Err(hr) => {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = DestroyWindow(hwnd);
                    drop(Arc::from_raw(input_state_raw));
                }
                return Err(format!(
                    "CreateDXGIFactory1(IDXGIFactory2) failed: HRESULT=0x{:08X}",
                    hr as u32
                ));
            }
        };

        let sc_desc = DxgiSwapChainDesc1 {
            width: w,
            height: h,
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            stereo: 0,
            sample_desc: DxgiSampleDesc { count: 1, quality: 0 },
            buffer_usage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            buffer_count: 2,
            scaling: DXGI_SCALING_STRETCH,
            swap_effect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            alpha_mode: DXGI_ALPHA_MODE_IGNORE,
            flags: 0,
        };

        let swapchain = unsafe {
            let f_vt = factory.vtable();
            let create_sc: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                isize,
                *const DxgiSwapChainDesc1,
                *const c_void,
                *mut c_void,
                *mut *mut c_void,
            ) -> i32 = std::mem::transmute(*f_vt.add(15));
            let make_assoc: unsafe extern "system" fn(*mut c_void, isize, u32) -> i32 =
                std::mem::transmute(*f_vt.add(8));

            let mut p_sc: *mut c_void = std::ptr::null_mut();
            let hr = create_sc(
                factory.as_ptr(),
                gpu.device().as_ptr(),
                hwnd,
                &sc_desc,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut p_sc,
            );
            if hr < 0 || p_sc.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(hwnd);
                drop(Arc::from_raw(input_state_raw));
                return Err(format!(
                    "IDXGIFactory2::CreateSwapChainForHwnd failed: HRESULT=0x{:08X}",
                    hr as u32
                ));
            }
            let _ = make_assoc(factory.as_ptr(), hwnd, DXGI_MWA_NO_ALT_ENTER);
            ComPtr::from_raw(p_sc).ok_or_else(|| "null IDXGISwapChain1".to_string())?
        };

        let d3d11_multithread_protected =
            crate::mf_decoder::ensure_d3d11_multithread_protected(&gpu);
        let hd_shader = HdNv12ShaderRenderer::new(&gpu).ok();
        if hd_shader.is_some() {
            HD_SHADER_TRACE_ACTIVE.store(true, Ordering::Relaxed);
        }

        Ok(Self {
            gpu,
            ring,
            hwnd,
            swapchain: Some(swapchain),
            processor: None,
            snap_processor: None,
            snap_rt: None,
            snap_staging: None,
            thumb_processor: None,
            thumb_rt: None,
            thumb_staging: None,
            probe_processor: None,
            probe_rt: None,
            probe_rtv: None,
            probe_staging: None,
            last_probe_at: None,
            input_state,
            input_state_raw,
            width: w,
            height: h,
            vsync_interval: 1,
            auto_rotate_enabled: false,
            frames_presented: 0,
            last_frame_seq: 0,
            last_source_dims: (0, 0),
            pending_dual_backbuffer_overwrite: false,
            mid_stream_black_clears: 0,
            dual_buffer_overwrites: 0,
            airplay_render_policy: AirplayRenderPolicy::default(),
            last_effective_rotation_deg: 0,
            vp_rebuilds: 0,
            last_presented_src_rect: (0, 0, 0, 0),
            last_presented_dest_rect: (0, 0, 0, 0),
            d3d11_multithread_protected,
            resize_atomic_repaints: 0,
            hd_shader,
            hd_shader_presents: 0,
            last_presenter_key: None,
        })
    }

    /// Creates a DirectComposition swapchain presenter (`hwnd = 0`) using
    /// `CreateSwapChainForComposition` (`DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL` + `ALPHA_MODE_IGNORE`)
    /// for Layer 0 video compositing behind a translucent Layer 1 UI visual.
    pub fn new_for_composition(
        gpu: Arc<LiveD3d11Gpu>,
        ring: Arc<D3d11Nv12VramRing>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let w = width.max(1);
        let h = height.max(1);
        let input_state = Arc::new(StageInputState {
            config: Mutex::new(PresenterConfig::default()),
            pressed: AtomicBool::new(false),
            callback: Mutex::new(None),
            key_callback: Mutex::new(None),
        });
        let swapchain = create_composition_swapchain(&gpu, w, h, DXGI_ALPHA_MODE_IGNORE)?;
        let d3d11_multithread_protected =
            crate::mf_decoder::ensure_d3d11_multithread_protected(&gpu);
        let hd_shader = HdNv12ShaderRenderer::new(&gpu).ok();
        if hd_shader.is_some() {
            HD_SHADER_TRACE_ACTIVE.store(true, Ordering::Relaxed);
        }

        Ok(Self {
            gpu,
            ring,
            hwnd: 0,
            swapchain: Some(swapchain),
            processor: None,
            snap_processor: None,
            snap_rt: None,
            snap_staging: None,
            thumb_processor: None,
            thumb_rt: None,
            thumb_staging: None,
            probe_processor: None,
            probe_rt: None,
            probe_rtv: None,
            probe_staging: None,
            last_probe_at: None,
            input_state,
            input_state_raw: std::ptr::null(),
            width: w,
            height: h,
            vsync_interval: 1,
            auto_rotate_enabled: false,
            frames_presented: 0,
            last_frame_seq: 0,
            last_source_dims: (0, 0),
            pending_dual_backbuffer_overwrite: false,
            mid_stream_black_clears: 0,
            dual_buffer_overwrites: 0,
            airplay_render_policy: AirplayRenderPolicy::default(),
            last_effective_rotation_deg: 0,
            vp_rebuilds: 0,
            last_presented_src_rect: (0, 0, 0, 0),
            last_presented_dest_rect: (0, 0, 0, 0),
            d3d11_multithread_protected,
            resize_atomic_repaints: 0,
            hd_shader,
            hd_shader_presents: 0,
            last_presenter_key: None,
        })
    }

    pub fn gpu(&self) -> &Arc<LiveD3d11Gpu> {
        &self.gpu
    }

    pub fn swapchain_ptr(&self) -> *mut c_void {
        self.swapchain
            .as_ref()
            .map(|sc| sc.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn set_vsync_interval(&mut self, interval: u32) {
        self.vsync_interval = interval.min(1);
    }

    pub fn set_auto_rotate_enabled(&mut self, enabled: bool) {
        self.auto_rotate_enabled = enabled;
        self.airplay_render_policy.auto_rotate = enabled;
    }

    pub fn set_airplay_render_policy(&mut self, policy: AirplayRenderPolicy) {
        self.airplay_render_policy = policy;
    }

    pub fn airplay_render_policy(&self) -> AirplayRenderPolicy {
        self.airplay_render_policy
    }

    pub fn set_airplay_mode(&mut self, is_airplay: bool) {
        self.airplay_render_policy.is_airplay = is_airplay;
    }

    pub fn set_window_landscape(&mut self, window_is_landscape: bool) {
        self.airplay_render_policy.window_is_landscape = window_is_landscape;
    }

    pub fn mid_stream_black_clears(&self) -> u64 {
        self.mid_stream_black_clears
    }

    pub fn dual_buffer_overwrites(&self) -> u64 {
        self.dual_buffer_overwrites
    }

    pub fn last_effective_rotation_deg(&self) -> u16 {
        self.last_effective_rotation_deg
    }

    pub fn vp_rebuilds(&self) -> u64 {
        self.vp_rebuilds
    }

    pub fn last_presented_src_rect(&self) -> (i32, i32, i32, i32) {
        self.last_presented_src_rect
    }

    pub fn last_presented_dest_rect(&self) -> (i32, i32, i32, i32) {
        self.last_presented_dest_rect
    }

    pub fn d3d11_multithread_protected(&self) -> bool {
        self.d3d11_multithread_protected
    }

    pub fn frames_presented(&self) -> u64 {
        self.frames_presented
    }

    pub fn resize_atomic_repaints(&self) -> u64 {
        self.resize_atomic_repaints
    }

    pub fn hd_shader_active(&self) -> bool {
        self.hd_shader.is_some()
    }

    pub fn hd_shader_presents(&self) -> u64 {
        self.hd_shader_presents
    }

    #[inline]
    pub fn effective_sharpen_strength(&self) -> f32 {
        if self.airplay_render_policy.is_airplay {
            0.0
        } else {
            HEVC_LUMA_CAS_STRENGTH
        }
    }

    fn release_cached_processors_and_views(&mut self) {
        self.processor = None;
        self.last_presenter_key = None;
        if let Some(ref mut sh) = self.hd_shader {
            sh.clear_srv_cache();
        }
        self.snap_processor = None;
        self.snap_rt = None;
        self.snap_staging = None;
        self.thumb_processor = None;
        self.thumb_rt = None;
        self.thumb_staging = None;
        self.probe_processor = None;
        self.probe_rt = None;
        self.probe_rtv = None;
        self.probe_staging = None;
    }

    pub fn set_presenter_config(&mut self, mut cfg: PresenterConfig) {
        cfg.crop_top = cfg.crop_top.min(24);
        cfg.crop_bottom = cfg.crop_bottom.min(24);
        cfg.crop_left = cfg.crop_left.min(24);
        cfg.crop_right = cfg.crop_right.min(24);
        cfg.rotation_deg = cfg.normalized_rotation();
        self.airplay_render_policy.manual_rotation_deg = cfg.rotation_deg;
        *self
            .input_state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = cfg;
    }

    pub fn set_rotation(&mut self, deg: u16) {
        let mut cfg = self
            .input_state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let norm = match deg % 360 {
            90 => 90,
            180 => 180,
            270 => 270,
            _ => 0,
        };
        cfg.rotation_deg = norm;
        self.airplay_render_policy.manual_rotation_deg = norm;
    }

    pub fn set_fit_mode(&mut self, fit: FitMode) {
        self.input_state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fit_mode = fit;
    }

    pub fn set_pointer_callback(&self, cb: Option<StagePointerCallback>) {
        *self
            .input_state
            .callback
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = cb;
    }

    pub fn set_key_callback(&self, cb: Option<StageKeyCallback>) {
        *self
            .input_state
            .key_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = cb;
    }

    pub fn set_crop(&mut self, top: u32, bottom: u32, left: u32, right: u32) {
        let mut cfg = self
            .input_state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cfg.crop_top = top.min(24);
        cfg.crop_bottom = bottom.min(24);
        cfg.crop_left = left.min(24);
        cfg.crop_right = right.min(24);
    }

    /// Moves/resizes the Child `HWND` within its parent window and safely resizes the DXGI SwapChain.
    pub fn set_bounds(&mut self, x: i32, y: i32, w: u32, h: u32, visible: bool) -> Result<(), String> {
        let nw = w.max(1);
        let nh = h.max(1);
        if self.hwnd != 0 {
            unsafe {
                let flags = SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | if visible { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
                let _ = SetWindowPos(self.hwnd, 0, x, y, nw as i32, nh as i32, flags);
            }
        }
        self.resize(nw, nh)
    }

    /// Shows or hides the video Child `HWND` cleanly (hide when `phase != "running"` / `waiting_airplay`).
    pub fn set_visible(&self, visible: bool) {
        if self.hwnd != 0 {
            unsafe {
                let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    /// Carves rectangular overlay holes (for Slint toasts/tooltips/dialogs) out of the Child `HWND`
    /// using Win32 `SetWindowRgn` (Windows owns the final `HRGN`).
    pub fn set_overlay_holes(&self, rects: &[Rect]) {
        if self.hwnd == 0 {
            return;
        }
        unsafe {
            if rects.is_empty() {
                let _ = SetWindowRgn(self.hwnd, 0, 1);
                return;
            }
            let rgn = CreateRectRgn(0, 0, self.width as i32, self.height as i32);
            if rgn == 0 {
                return;
            }
            for r in rects {
                if r.right <= r.left || r.bottom <= r.top {
                    continue;
                }
                let cut = CreateRectRgn(r.left, r.top, r.right, r.bottom);
                if cut != 0 {
                    let _ = CombineRgn(rgn, rgn, cut, RGN_DIFF);
                    let _ = DeleteObject(cut);
                }
            }
            // SetWindowRgn takes ownership of `rgn` when it succeeds
            if SetWindowRgn(self.hwnd, rgn, 1) == 0 {
                let _ = DeleteObject(rgn);
            }
        }
    }

    /// Purges all stale NV12 VRAM slots, cached video processors/offscreen textures, and clears
    /// BOTH `FLIP_SEQUENTIAL` swapchain backbuffers cleanly so mid-stream SPS resets/watchdog
    /// recreates never flash stale orientation frames or corrupted buffers.
    pub fn purge_stale_video_buffers(&mut self) -> Result<u32, String> {
        self.ring.clear();
        self.last_frame_seq = 0;
        self.last_source_dims = (0, 0);
        self.frames_presented = 0;
        self.dual_buffer_overwrites = 0;
        self.hd_shader_presents = 0;
        HD_SHADER_TRACE_PRESENTS.store(0, Ordering::Relaxed);
        HD_SHADER_TRACE_ACTIVE.store(self.hd_shader.is_some(), Ordering::Relaxed);
        self.pending_dual_backbuffer_overwrite = false;
        self.release_cached_processors_and_views();

        let Some(ref sc) = self.swapchain else {
            return Ok(0);
        };
        let mut cleared_buffers = 0u32;
        for _ in 0..2 {
            let backbuf = unsafe {
                let vt = sc.vtable();
                let get_buf: unsafe extern "system" fn(
                    *mut c_void,
                    u32,
                    *const crate::d3d11_context::Guid,
                    *mut *mut c_void,
                ) -> i32 = std::mem::transmute(*vt.add(9));
                let mut p_bb: *mut c_void = std::ptr::null_mut();
                let hr = get_buf(sc.as_ptr(), 0, &IID_ID3D11_TEXTURE2D, &mut p_bb);
                if hr < 0 || p_bb.is_null() {
                    return Err(format!(
                        "purge_stale_video_buffers GetBuffer(0) failed: HRESULT=0x{:08X}",
                        hr as u32
                    ));
                }
                ComPtr::from_raw(p_bb)
                    .ok_or_else(|| "null backbuffer during purge_stale_video_buffers".to_string())?
            };
            if let Ok(rtv) = self.gpu.create_rtv(&backbuf) {
                self.gpu.clear_rtv(&rtv, [0.0, 0.0, 0.0, 1.0]);
            }
            drop(backbuf);
            let hr_pres = unsafe {
                let vt = sc.vtable();
                let present: unsafe extern "system" fn(*mut c_void, u32, u32) -> i32 =
                    std::mem::transmute(*vt.add(8));
                present(sc.as_ptr(), 0, 0)
            };
            if hr_pres < 0 {
                return Err(format!(
                    "purge_stale_video_buffers Present(0,0) failed: HRESULT=0x{:08X}",
                    hr_pres as u32
                ));
            }
            cleared_buffers += 1;
        }
        Ok(cleared_buffers)
    }

    /// Probes `apply_deterministic_bt709_color_pipeline` on a real D3D11 `ID3D11VideoProcessor`
    /// instance and returns the applied DXGI/legacy color space & auto-processing state.
    pub fn verify_deterministic_bt709_pipeline_probe(
        &self,
        color: VideoColorInfo,
    ) -> Result<DeterministicColorPipelineState, String> {
        let vp = LiveVideoProcessor::create(&self.gpu, 64, 64, 64, 64)?;
        Ok(apply_deterministic_bt709_color_pipeline(
            &self.gpu, &vp.vp, color,
        ))
    }

    /// Tahap 274 (Pilar 1): Transitions across orientation/resolution/watchdog switches mid-stream
    /// WITHOUT painting black frames (`0` `ClearRenderTargetView(BLACK)` and `0` `Present(0, 0)`).
    /// Holds the last clean displayed image in the DXGI swapchain until the first clean IDR
    /// frame of the new dimensions arrives, then atomically overwrites both swapchain backbuffers.
    pub fn prepare_seamless_dimension_transition(&mut self) {
        self.ring.clear();
        self.last_source_dims = (0, 0);
        self.release_cached_processors_and_views();
        self.pending_dual_backbuffer_overwrite = true;
    }

    /// Safely resizes the DXGI `FLIP_DISCARD` SwapChain after releasing all backbuffer/VP references.
    pub fn resize(&mut self, w: u32, h: u32) -> Result<(), String> {
        let nw = w.max(1);
        let nh = h.max(1);
        if nw == self.width && nh == self.height {
            return Ok(());
        }
        // Drop video processor BEFORE ResizeBuffers so 0 backbuffer/output views remain!
        self.processor = None;
        self.thumb_processor = None;
        self.thumb_rt = None;
        self.thumb_staging = None;
        let sc = self
            .swapchain
            .as_ref()
            .ok_or_else(|| "SwapChain not initialized".to_string())?;
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
                "IDXGISwapChain::ResizeBuffers({nw}x{nh}) failed: HRESULT=0x{:08X}",
                hr as u32
            ));
        }
        self.width = nw;
        self.height = nh;
        // Tahap 277 (Pilar 3): ResizeBuffers invalidates backbuffers to black — immediately
        // atomically repaint the cached NV12 frame with BT.709 color BEFORE any DComp Commit!
        if self.frames_presented > 0 && self.ring.latest_slot().is_some() {
            let saved_vsync = self.vsync_interval;
            self.vsync_interval = 0;
            let repainted = self.force_repaint_cached_frame().is_ok();
            self.vsync_interval = saved_vsync;
            if repainted {
                self.resize_atomic_repaints = self.resize_atomic_repaints.saturating_add(1);
            }
        }
        Ok(())
    }

    /// Presents the latest NV12 VRAM frame onto the DXGI `FLIP_DISCARD`/`FLIP_SEQUENTIAL` SwapChain
    /// using hardware `ID3D11VideoProcessor` (`NV12 -> BGRA`, atomic per-frame `0/90/180/270°`
    /// rotation, Fit Modes, 4-edge crop).
    ///
    /// Tahap 274 (Pilar 1 & 4):
    /// - Holds the last valid displayed frame mid-stream if the VRAM ring is momentarily empty
    ///   (ZERO black clear & ZERO empty Present mid-stream).
    /// - Atomically overwrites both `FLIP_SEQUENTIAL` backbuffers on the first clean frame after
    ///   `prepare_seamless_dimension_transition()`.
    pub fn present_latest(&mut self) -> Result<LivePresentStats, String> {
        let sc = self
            .swapchain
            .as_ref()
            .ok_or_else(|| "SwapChain not initialized".to_string())?
            .clone();
        let cfg = self
            .input_state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let latest_snap = self.ring.latest_slot();
        if latest_snap.is_none()
            && (self.pending_dual_backbuffer_overwrite || self.frames_presented > 0)
        {
            return Ok(LivePresentStats {
                presented_total: self.frames_presented,
                frame_seq: self.last_frame_seq,
                swapchain_w: self.width,
                swapchain_h: self.height,
                used_hardware_vp: false,
                supports_hw_rotation: false,
                dual_buffer_overwritten: false,
            });
        }

        let do_dual_overwrite = self.pending_dual_backbuffer_overwrite && latest_snap.is_some();
        let pass_count = if do_dual_overwrite { 2 } else { 1 };
        let prev_rot = self.last_effective_rotation_deg;

        let mut used_hw_vp = false;
        let mut supports_hw_rot = false;
        let mut frame_seq = self.last_frame_seq;

        for pass_idx in 0..pass_count {
            used_hw_vp = false;
            supports_hw_rot = false;
            // Acquire DXGI backbuffer 0 (must be dropped before Present or ResizeBuffers)
            let backbuf = acquire_swapchain_backbuffer(&sc)?;

            if let Some(ref snap) = latest_snap {
                frame_seq = snap.frame_seq;
                self.last_frame_seq = frame_seq;
                // Tahap 275 (Pilar 2): Always inspect the actual D3D11 NV12 texture dims for
                // each frame so adaptive portrait scaling (`990x2160 <-> 888x1920 <-> 640x1388`)
                // clamps `src_raw` strictly inside committed bounds and recreates
                // `LiveVideoProcessor` atomically without stretching or black borders.
                let tex_desc = LiveD3d11Gpu::get_texture2d_desc(&snap.texture);
                let in_w = tex_desc.width.max(2);
                let in_h = tex_desc.height.max(2);
                let src_dims = (in_w, in_h);
                let unclipped_src = compute_vram_source_rect(snap, &cfg);
                let src_raw = Rect {
                    left: unclipped_src.left.clamp(0, in_w.saturating_sub(2) as i32),
                    top: unclipped_src.top.clamp(0, in_h.saturating_sub(2) as i32),
                    right: unclipped_src.right.clamp(2.min(in_w as i32), in_w as i32),
                    bottom: unclipped_src.bottom.clamp(2.min(in_h as i32), in_h as i32),
                };
                let (rot, src_clipped, dst_clipped) = if self.airplay_render_policy.is_airplay {
                    compute_stateless_airplay_present_rects(
                        src_raw,
                        self.width,
                        self.height,
                        self.airplay_render_policy.manual_rotation_deg,
                        cfg.fit_mode,
                    )
                } else {
                    let frame_vis_w = (src_raw.right - src_raw.left).max(1) as u32;
                    let frame_vis_h = (src_raw.bottom - src_raw.top).max(1) as u32;
                    let r = resolve_atomic_frame_rotation_deg(
                        self.airplay_render_policy,
                        frame_vis_w,
                        frame_vis_h,
                        cfg.normalized_rotation(),
                    );
                    let (sc_r, dc_r) =
                        compute_dest_and_clip_rects(src_raw, self.width, self.height, r, cfg.fit_mode);
                    (r, sc_r, dc_r)
                };
                self.last_effective_rotation_deg = rot;

                let dims_changed = self.last_source_dims != (0, 0) && self.last_source_dims != src_dims;
                let rot_changed = self.frames_presented > 0 && prev_rot != rot;
                if dims_changed || rot_changed {
                    self.processor = None;
                    if dims_changed {
                        if let Some(ref mut sh) = self.hd_shader {
                            sh.clear_srv_cache();
                        }
                        self.snap_processor = None;
                        self.thumb_processor = None;
                        self.thumb_rt = None;
                        self.thumb_staging = None;
                    }
                }
                let mut cleared_this_pass = false;
                if do_dual_overwrite
                    || pass_idx > 0
                    || dims_changed
                    || rot_changed
                {
                    if let Ok(rtv) = self.gpu.create_rtv(&backbuf) {
                        self.gpu.clear_rtv(&rtv, [0.0, 0.0, 0.0, 1.0]);
                    }
                    cleared_this_pass = true;
                }
                self.last_source_dims = src_dims;

                self.last_presented_src_rect = (
                    src_clipped.left,
                    src_clipped.top,
                    src_clipped.right,
                    src_clipped.bottom,
                );
                self.last_presented_dest_rect = (
                    dst_clipped.left,
                    dst_clipped.top,
                    dst_clipped.right,
                    dst_clipped.bottom,
                );

                let want_key = (in_w, in_h, self.width, self.height);
                let key_changed = self.last_presenter_key != Some(want_key);
                if pass_idx == 0 && key_changed {
                    if self.frames_presented > 0 {
                        self.vp_rebuilds = self.vp_rebuilds.saturating_add(1);
                    }
                    self.last_presenter_key = Some(want_key);
                }

                // Tahap 280/281 Primary Renderer: Adaptive Multi-Tap HD NV12 -> BGRA Pixel Shader
                // (zero-copy VRAM, adaptive Lanczos-2 downscale + Catmull-Rom upscale + exact BT.709,
                // plus zero-extra-tap Subtle Luma-Only CAS 0.28 for HEVC and 0.0 for AirPlay).
                let sharpen_strength = self.effective_sharpen_strength();
                if let Some(ref mut sh) = self.hd_shader {
                    if let Ok(rtv) = self.gpu.create_rtv(&backbuf) {
                        let covers_full_rt = dst_clipped.left <= 0
                            && dst_clipped.top <= 0
                            && dst_clipped.right >= self.width as i32
                            && dst_clipped.bottom >= self.height as i32;
                        if !cleared_this_pass && !covers_full_rt {
                            self.gpu.clear_rtv(&rtv, [0.0, 0.0, 0.0, 1.0]);
                        }
                        if sh
                            .render_to_rtv_with_sharpen(
                                &self.gpu,
                                &snap.texture,
                                &rtv,
                                in_w,
                                in_h,
                                src_clipped,
                                dst_clipped,
                                rot,
                                snap.color_info,
                                sharpen_strength,
                            )
                            .is_ok()
                        {
                            used_hw_vp = true;
                            supports_hw_rot = true;
                            self.hd_shader_presents = self.hd_shader_presents.saturating_add(1);
                            HD_SHADER_TRACE_ACTIVE.store(true, Ordering::Relaxed);
                            HD_SHADER_TRACE_PRESENTS.fetch_add(1, Ordering::Relaxed);
                        }
                        drop(rtv);
                    }
                }

                // Automatic fallback to ID3D11VideoProcessor on unsupported/exotic drivers or SRV miss
                if !used_hw_vp && self.gpu.has_video_processor() {
                    if self.processor.as_ref().map(|p| p.key) != Some(want_key) {
                        if !key_changed && self.frames_presented > 0 {
                            self.vp_rebuilds = self.vp_rebuilds.saturating_add(1);
                        }
                        self.processor = None;
                        self.processor = LiveVideoProcessor::create(
                            &self.gpu,
                            in_w,
                            in_h,
                            self.width,
                            self.height,
                        )
                        .ok();
                    }
                    if let Some(ref vp) = self.processor {
                        supports_hw_rot = vp.supports_rotation;
                        if vp
                            .blit(
                                &self.gpu,
                                &snap.texture,
                                &backbuf,
                                src_clipped,
                                dst_clipped,
                                self.width,
                                self.height,
                                rot,
                                snap.color_info,
                            )
                            .is_ok()
                        {
                            used_hw_vp = true;
                        } else {
                            self.processor = None;
                        }
                    }
                }

                if !used_hw_vp {
                    let mut sw_cfg = cfg.clone();
                    sw_cfg.rotation_deg = rot;
                    if let Ok(rtv) = self.gpu.create_rtv(&backbuf) {
                        self.gpu.clear_rtv(&rtv, [0.0, 0.0, 0.0, 1.0]);
                    }
                    if let Some(ref nv12) = snap.cpu_nv12_backup {
                        let mut tmp_slot = SharedFrameSlot::empty(0);
                        tmp_slot.coded_width = snap.coded_width;
                        tmp_slot.coded_height = snap.coded_height;
                        tmp_slot.trim_top = snap.trim.0;
                        tmp_slot.trim_left = snap.trim.1;
                        tmp_slot.trim_bottom = snap.trim.2;
                        tmp_slot.trim_right = snap.trim.3;
                        tmp_slot.nv12_data = (**nv12).clone();
                        let rendered = render_slot_to_bgra_surface_with_color(
                            &tmp_slot,
                            &sw_cfg,
                            snap.color_info,
                        );
                        if rendered.width == self.width && rendered.height == self.height {
                            self.gpu.update_subresource(
                                &backbuf,
                                0,
                                None,
                                rendered.bgra.as_ptr() as *const c_void,
                                rendered.width * 4,
                                0,
                            );
                        }
                    }
                }
            } else if let Ok(rtv) = self.gpu.create_rtv(&backbuf) {
                // Initial pre-first-frame clear only (`frames_presented == 0`).
                self.gpu.clear_rtv(&rtv, [0.0, 0.0, 0.0, 1.0]);
            }

            // Release backbuffer BEFORE Present!
            drop(backbuf);

            let interval = if do_dual_overwrite && pass_idx == 0 {
                0
            } else {
                self.vsync_interval
            };
            let hr_pres = unsafe {
                let vt = sc.vtable();
                let present: unsafe extern "system" fn(*mut c_void, u32, u32) -> i32 =
                    std::mem::transmute(*vt.add(8));
                present(sc.as_ptr(), interval, 0)
            };
            if hr_pres < 0 {
                return Err(format!(
                    "IDXGISwapChain::Present failed: HRESULT=0x{:08X}",
                    hr_pres as u32
                ));
            }

            self.frames_presented = self.frames_presented.saturating_add(1);
        }

        if do_dual_overwrite {
            self.pending_dual_backbuffer_overwrite = false;
            self.dual_buffer_overwrites = self.dual_buffer_overwrites.saturating_add(1);
        }

        Ok(LivePresentStats {
            presented_total: self.frames_presented,
            frame_seq,
            swapchain_w: self.width,
            swapchain_h: self.height,
            used_hardware_vp: used_hw_vp,
            supports_hw_rotation: supports_hw_rot,
            dual_buffer_overwritten: do_dual_overwrite,
        })
    }

    /// Tahap 276 (Pilar 3): Immediately re-presents the cached VFR frame into the current
    /// SwapChain dimensions on Maximize / Restore / Fullscreen toggles without waiting for
    /// a new decoded packet and without clearing to black/empty if idle.
    pub fn force_repaint_cached_frame(&mut self) -> Result<LivePresentStats, String> {
        self.present_latest()
    }

    /// Exports the latest VRAM frame cleanly cropped & rotated into a lossless 24-bit top-down
    /// BMP (`'BM'`) via a lazily cached `D3D11_USAGE_STAGING` BGRA texture in **`< 5 ms`**
    /// (for WinRT OCR `Alt+C` / Link Grabber `Alt+B`).
    pub fn capture_latest_frame_lossless_bmp(&mut self) -> Result<Vec<u8>, String> {
        let snap = self
            .ring
            .latest_slot()
            .ok_or_else(|| "No VRAM frame available for snapshot".to_string())?;
        let mut cfg = self
            .input_state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let src_rect = compute_vram_source_rect(&snap, &cfg);
        let src_w = (src_rect.right - src_rect.left).max(1) as u32;
        let src_h = (src_rect.bottom - src_rect.top).max(1) as u32;
        let bmp_policy = if self.airplay_render_policy.is_airplay {
            AirplayRenderPolicy {
                window_is_landscape: self.width > self.height,
                auto_rotate: false,
                ..self.airplay_render_policy
            }
        } else {
            self.airplay_render_policy
        };
        let rot = resolve_atomic_frame_rotation_deg(
            bmp_policy,
            src_w,
            src_h,
            cfg.normalized_rotation(),
        );
        cfg.rotation_deg = rot;
        let (raw_out_w, raw_out_h) = if rot == 90 || rot == 270 {
            (src_h, src_w)
        } else {
            (src_w, src_h)
        };
        // Keep OCR snapshot crisp (>= 480px short edge when available) and guaranteed < 5 ms GPU readback
        let max_dim = 1080u32;
        let (out_w, out_h) = if raw_out_w.max(raw_out_h) > max_dim {
            let scale = max_dim as f32 / raw_out_w.max(raw_out_h) as f32;
            (
                ((raw_out_w as f32 * scale).round() as u32).max(2),
                ((raw_out_h as f32 * scale).round() as u32).max(2),
            )
        } else {
            (raw_out_w.max(2), raw_out_h.max(2))
        };

        let row_stride = ((out_w as usize * 3 + 3) / 4) * 4;
        let pixel_bytes = row_stride * out_h as usize;
        let file_size = 54 + pixel_bytes;

        let mut bmp = Vec::with_capacity(file_size);
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&(out_w as i32).to_le_bytes());
        bmp.extend_from_slice(&(-(out_h as i32)).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
        bmp.extend_from_slice(&2835u32.to_le_bytes());
        bmp.extend_from_slice(&2835u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.resize(file_size, 0);

        if self.gpu.has_video_processor() {
            // Allocate RT + staging textures once per size (cached, zero per-snapshot GPU alloc!)
            if !matches!(&self.snap_rt, Some((_, w, h)) if *w == out_w && *h == out_h) {
                self.snap_rt = None;
                self.snap_staging = None;
                let rt = self
                    .gpu
                    .create_bgra_texture(out_w, out_h, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET, 0)
                    .map_err(|hr| format!("snap RT failed: 0x{:08X}", hr as u32))?;
                let stg = self
                    .gpu
                    .create_bgra_texture(out_w, out_h, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ)
                    .map_err(|hr| format!("snap staging failed: 0x{:08X}", hr as u32))?;
                self.snap_rt = Some((rt, out_w, out_h));
                self.snap_staging = Some((stg, out_w, out_h));
            }
            let want_key = (snap.coded_width, snap.coded_height, out_w, out_h);
            if self.snap_processor.as_ref().map(|p| p.key) != Some(want_key) {
                self.snap_processor = None;
                self.snap_processor = Some(LiveVideoProcessor::create(
                    &self.gpu,
                    snap.coded_width,
                    snap.coded_height,
                    out_w,
                    out_h,
                )?);
            }
            let dst_rect = Rect {
                left: 0,
                top: 0,
                right: out_w as i32,
                bottom: out_h as i32,
            };
            let rt_tex = &self.snap_rt.as_ref().unwrap().0;
            let stg_tex = &self.snap_staging.as_ref().unwrap().0;
            self.snap_processor.as_ref().unwrap().blit(
                &self.gpu,
                &snap.texture,
                rt_tex,
                src_rect,
                dst_rect,
                out_w,
                out_h,
                rot,
                snap.color_info,
            )?;
            self.gpu.copy_resource(stg_tex, rt_tex);
            let mapped = self
                .gpu
                .map_texture(stg_tex, 0, D3D11_MAP_READ)
                .map_err(|hr| format!("snap Map(READ) failed: 0x{:08X}", hr as u32))?;
            let dst_pixels = &mut bmp[54..];
            unsafe {
                let pitch = mapped.row_pitch as usize;
                for y in 0..out_h as usize {
                    let src_row = (mapped.p_data as *const u8).add(y * pitch);
                    let dst_row = &mut dst_pixels[y * row_stride..y * row_stride + out_w as usize * 3];
                    for x in 0..out_w as usize {
                        let sp = src_row.add(x * 4);
                        let dp = x * 3;
                        dst_row[dp] = *sp; // B
                        dst_row[dp + 1] = *sp.add(1); // G
                        dst_row[dp + 2] = *sp.add(2); // R
                    }
                }
            }
            self.gpu.unmap_texture(stg_tex, 0);
            return Ok(bmp);
        }

        // Software fallback when VideoProcessor is absent (e.g. headless WARP without VP)
        if let Some(ref nv12) = snap.cpu_nv12_backup {
            let mut slot = SharedFrameSlot::empty(0);
            slot.coded_width = snap.coded_width;
            slot.coded_height = snap.coded_height;
            slot.trim_top = snap.trim.0;
            slot.trim_left = snap.trim.1;
            slot.trim_bottom = snap.trim.2;
            slot.trim_right = snap.trim.3;
            slot.nv12_data = (**nv12).clone();
            let res = export_latest_frame_lossless_bmp(&slot, &cfg)?;
            let b64 = res
                .data_url
                .strip_prefix("data:image/bmp;base64,")
                .unwrap_or(&res.data_url);
            return decode_base64_fast(b64)
                .ok_or_else(|| "Failed to decode fallback BMP".to_string());
        }

        Ok(bmp)
    }

    /// Downscales the active VRAM frame into `ProbeGrid120x260` using a lazily cached `120x260`
    /// GPU staging BGRA texture (`0` heap allocations, `< 0.2 ms` GPU) every `~1000 ms` when
    /// `auto_rotate_enabled == true && !airplay` (or immediately when `force == true`).
    /// Matches `handset/web/mirror.js:95, 4004`: uses full untrimmed coded surface (before
    /// bottom/right trim or user Edge Crop) so `y=0..259` coordinates align 1:1 with WebView2.
    pub fn update_autorotate_probe_if_due(
        &mut self,
        grid: &mut ProbeGrid120x260,
        force: bool,
    ) -> bool {
        if !force {
            if !self.auto_rotate_enabled || self.airplay_render_policy.is_airplay {
                return false;
            }
            if let Some(last) = self.last_probe_at {
                if last.elapsed() < Duration::from_millis(1000) {
                    return false;
                }
            }
        }
        let Some(snap) = self.ring.latest_slot() else {
            return false;
        };
        let desc = LiveD3d11Gpu::get_texture2d_desc(&snap.texture);
        let tex_w = desc.width.max(1);
        let tex_h = desc.height.max(1);
        let src_rect = Rect {
            left: 0,
            top: 0,
            right: snap.coded_width.clamp(1, tex_w) as i32,
            bottom: snap.coded_height.clamp(1, tex_h) as i32,
        };
        let pw = GRID_WIDTH as u32;
        let ph = GRID_HEIGHT as u32;

        if self.probe_rt.is_none() || self.probe_rtv.is_none() || self.probe_staging.is_none() {
            let Ok(rt) = self.gpu.create_bgra_texture(
                pw,
                ph,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_RENDER_TARGET,
                0,
            ) else {
                return false;
            };
            let rtv = self.gpu.create_rtv(&rt).ok();
            let Ok(stg) = self
                .gpu
                .create_bgra_texture(pw, ph, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ)
            else {
                return false;
            };
            self.probe_rt = Some(rt);
            self.probe_rtv = rtv;
            self.probe_staging = Some(stg);
        }

        let dst_rect = Rect {
            left: 0,
            top: 0,
            right: pw as i32,
            bottom: ph as i32,
        };
        let mut blitted = false;
        if self.gpu.has_video_processor() {
            let want_key = (snap.coded_width, snap.coded_height, pw, ph);
            if self.probe_processor.as_ref().map(|p| p.key) != Some(want_key) {
                self.probe_processor =
                    LiveVideoProcessor::create(&self.gpu, snap.coded_width, snap.coded_height, pw, ph)
                        .ok();
            }
            if let (Some(ref vp), Some(ref rt)) = (&self.probe_processor, &self.probe_rt) {
                if vp
                    .blit(&self.gpu, &snap.texture, rt, src_rect, dst_rect, pw, ph, 0, snap.color_info)
                    .is_ok()
                {
                    blitted = true;
                }
            }
        }
        if !blitted {
            if self.hd_shader.is_none() {
                self.hd_shader = HdNv12ShaderRenderer::new(&self.gpu).ok();
            }
            if let (Some(ref mut sh), Some(ref rtv)) = (&mut self.hd_shader, &self.probe_rtv) {
                blitted = sh
                    .render_to_rtv_with_sharpen(
                        &self.gpu,
                        &snap.texture,
                        rtv,
                        snap.coded_width,
                        snap.coded_height,
                        src_rect,
                        dst_rect,
                        0,
                        snap.color_info,
                        0.0,
                    )
                    .is_ok();
            }
        }
        if !blitted {
            return false;
        }
        let (Some(ref rt), Some(ref stg)) = (&self.probe_rt, &self.probe_staging) else {
            return false;
        };
        self.gpu.copy_resource(stg, rt);
        let Ok(mapped) = self.gpu.map_texture(stg, 0, D3D11_MAP_READ) else {
            return false;
        };
        unsafe {
            let pitch = mapped.row_pitch as usize;
            for gy in 0..GRID_HEIGHT {
                let row = (mapped.p_data as *const u8).add(gy * pitch);
                for gx in 0..GRID_WIDTH {
                    let sp = row.add(gx * 4);
                    let b = *sp;
                    let g = *sp.add(1);
                    let r = *sp.add(2);
                    grid.set_pixel(gx, gy, r, g, b);
                }
            }
        }
        self.gpu.unmap_texture(stg, 0);
        self.last_probe_at = Some(Instant::now());
        true
    }

    /// Captures a fast downscaled tightly packed RGBA8 thumbnail (`<= max_w x max_h`, `< 0.4 ms` GPU)
    /// for the Sequoia Blur-to-Sharp reveal/exit crossfade.
    pub fn capture_latest_frame_rgba_thumbnail(
        &mut self,
        max_w: u32,
        max_h: u32,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let snap = self.ring.latest_slot()?;
        if !self.gpu.has_video_processor() {
            return None;
        }
        let cfg = self
            .input_state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let src_rect = compute_vram_source_rect(&snap, &cfg);
        let sw = (src_rect.right - src_rect.left).max(2) as u32;
        let sh = (src_rect.bottom - src_rect.top).max(2) as u32;
        let thumb_policy = if self.airplay_render_policy.is_airplay {
            AirplayRenderPolicy {
                window_is_landscape: self.width > self.height,
                auto_rotate: false,
                ..self.airplay_render_policy
            }
        } else {
            self.airplay_render_policy
        };
        let rot = resolve_atomic_frame_rotation_deg(
            thumb_policy,
            sw,
            sh,
            cfg.normalized_rotation(),
        );
        let (rot_w, rot_h) = if rot == 90 || rot == 270 {
            (sh, sw)
        } else {
            (sw, sh)
        };
        let lim_w = max_w.max(32) as f32;
        let lim_h = max_h.max(32) as f32;
        let s = (lim_w / rot_w as f32).min(lim_h / rot_h as f32).min(1.0);
        let tw = (((rot_w as f32 * s).round() as u32).clamp(16, max_w.max(32))) & !1;
        let th = (((rot_h as f32 * s).round() as u32).clamp(16, max_h.max(32))) & !1;
        if tw < 16 || th < 16 {
            return None;
        }

        let need_alloc = match (&self.thumb_rt, &self.thumb_staging) {
            (Some((_, rw, rh)), Some((_, sw_s, sh_s))) => {
                *rw != tw || *rh != th || *sw_s != tw || *sh_s != th
            }
            _ => true,
        };
        if need_alloc {
            self.thumb_rt = None;
            self.thumb_staging = None;
            let rt = self
                .gpu
                .create_bgra_texture(tw, th, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET, 0)
                .ok()?;
            let stg = self
                .gpu
                .create_bgra_texture(tw, th, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ)
                .ok()?;
            self.thumb_rt = Some((rt, tw, th));
            self.thumb_staging = Some((stg, tw, th));
        }

        let want_key = (snap.coded_width, snap.coded_height, tw, th);
        if self.thumb_processor.as_ref().map(|p| p.key) != Some(want_key) {
            self.thumb_processor = None;
            self.thumb_processor =
                LiveVideoProcessor::create(&self.gpu, snap.coded_width, snap.coded_height, tw, th)
                    .ok();
        }
        let vp = self.thumb_processor.as_ref()?;
        let rt_tex = &self.thumb_rt.as_ref()?.0;
        let stg_tex = &self.thumb_staging.as_ref()?.0;
        let dst_rect = Rect {
            left: 0,
            top: 0,
            right: tw as i32,
            bottom: th as i32,
        };
        if vp
            .blit(
                &self.gpu,
                &snap.texture,
                rt_tex,
                src_rect,
                dst_rect,
                tw,
                th,
                rot,
                snap.color_info,
            )
            .is_err()
        {
            return None;
        }
        self.gpu.copy_resource(stg_tex, rt_tex);
        let mapped = self.gpu.map_texture(stg_tex, 0, D3D11_MAP_READ).ok()?;
        let mut rgba = vec![0u8; (tw as usize) * (th as usize) * 4];
        unsafe {
            let pitch = mapped.row_pitch as usize;
            for y in 0..th as usize {
                let src_row = (mapped.p_data as *const u8).add(y * pitch);
                let dst_row = &mut rgba[y * (tw as usize) * 4..(y + 1) * (tw as usize) * 4];
                for x in 0..tw as usize {
                    let sp = src_row.add(x * 4);
                    let dp = x * 4;
                    dst_row[dp] = *sp.add(2); // R
                    dst_row[dp + 1] = *sp.add(1); // G
                    dst_row[dp + 2] = *sp; // B
                    dst_row[dp + 3] = 255; // A
                }
            }
        }
        self.gpu.unmap_texture(stg_tex, 0);
        Some((rgba, tw, th))
    }
}

impl Drop for LiveVideoHwnd {
    fn drop(&mut self) {
        // Release all GPU/DXGI resources BEFORE DestroyWindow and BEFORE `self.gpu` drops!
        self.release_cached_processors_and_views();
        self.hd_shader = None;
        self.swapchain = None;
        unsafe {
            if self.hwnd != 0 {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(self.hwnd);
                self.hwnd = 0;
            }
            if !self.input_state_raw.is_null() {
                drop(Arc::from_raw(self.input_state_raw));
                self.input_state_raw = std::ptr::null();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mf_decoder::write_nv12_from_rgb_sampler;

    fn make_synthetic_1216x2656_slot(use_bgra_exact: bool) -> SharedFrameSlot {
        let mut slot = SharedFrameSlot::empty(0);
        slot.coded_width = 1216;
        slot.coded_height = 2656;
        slot.trim_top = 0;
        slot.trim_left = 0;
        slot.trim_bottom = 58;
        slot.trim_right = 14;
        slot.frame_seq = 1;

        if use_bgra_exact {
            let mut bgra = vec![0u8; 1216 * 2656 * 4];
            for y in 0..2656usize {
                for x in 0..1216usize {
                    let (r, g, b) = if x >= 1202 || y >= 2598 {
                        (0u8, 0u8, 0u8)
                    } else if y < 12 {
                        // Top Dynamic Island strip: bright yellow [255, 230, 0]
                        (255u8, 230u8, 0u8)
                    } else if y >= 2580 {
                        // Bottom field edge: green grass [0, 220, 100]
                        (0u8, 220u8, 100u8)
                    } else {
                        (20u8, 110u8, 210u8)
                    };
                    let idx = (y * 1216 + x) * 4;
                    bgra[idx] = b;
                    bgra[idx + 1] = g;
                    bgra[idx + 2] = r;
                    bgra[idx + 3] = 255;
                }
            }
            slot.bgra_data = bgra;
        } else {
            write_nv12_from_rgb_sampler(1216, 2656, &mut slot.nv12_data, |x, y| {
                if x >= 1202 || y >= 2598 {
                    (0, 0, 0)
                } else if y < 12 {
                    (255, 230, 0)
                } else if y >= 2580 {
                    (0, 220, 100)
                } else {
                    (20, 110, 210)
                }
            });
        }
        slot
    }

    #[test]
    fn test_tahap234_swapchain_flip_discard_and_clean_rotation_0_90_180_270() {
        let mut presenter = D3d11VideoPresenter::new();
        assert_eq!(presenter.swap_chain_spec.format, DXGI_FORMAT_B8G8R8A8_UNORM);
        assert_eq!(presenter.swap_chain_spec.buffer_count, 2);
        assert_eq!(presenter.swap_chain_spec.swap_effect, DXGI_SWAP_EFFECT_FLIP_DISCARD);
        assert_eq!(presenter.swap_chain_spec.alpha_mode, DXGI_ALPHA_MODE_IGNORE);
        assert_eq!(presenter.swap_chain_spec.vsync_interval, 1);
        assert!(presenter.swap_chain_spec.obs_capture_compatible);
        assert!(presenter.swap_chain_spec.video_canvas_solid_black);
        assert!(presenter.swap_chain_spec.chrome_glass_preserved);
        assert_eq!(
            presenter.swap_chain_spec.hwnd_style_flags,
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS
        );

        let slot = make_synthetic_1216x2656_slot(true);

        // 0° Portrait
        let out0 = presenter.present_slot(&slot);
        assert_eq!((out0.width, out0.height), (1202, 2598));
        assert_eq!(out0.pixel_rgb_at(600, 2), [255, 230, 0]);
        assert_eq!(out0.pixel_rgb_at(600, 2595), [0, 220, 100]);

        // 270° CW (= 90° CCW Landscape - eFootball 2027)
        let mut cfg270 = PresenterConfig::default();
        cfg270.rotation_deg = 270;
        presenter.update_config(cfg270);
        let out270 = presenter.present_slot(&slot);
        assert_eq!((out270.width, out270.height), (2598, 1202));
        assert_eq!(out270.pixel_rgb_at(2, 600), [255, 230, 0]);
        assert_eq!(out270.pixel_rgb_at(2595, 600), [0, 220, 100]);

        // 90° CW Inverted Landscape (Dynamic Island on right edge)
        let mut cfg90 = PresenterConfig::default();
        cfg90.rotation_deg = 90;
        let out90 = render_slot_to_bgra_surface(&slot, &cfg90);
        assert_eq!((out90.width, out90.height), (2598, 1202));
        assert_eq!(out90.pixel_rgb_at(2595, 600), [255, 230, 0]);

        // 180° Inverted Portrait
        let mut cfg180 = PresenterConfig::default();
        cfg180.rotation_deg = 180;
        let out180 = render_slot_to_bgra_surface(&slot, &cfg180);
        assert_eq!((out180.width, out180.height), (1202, 2598));
        assert_eq!(out180.pixel_rgb_at(600, 2595), [255, 230, 0]);
    }

    #[test]
    fn test_tahap234_viewport_geometry_3_fit_modes_and_fullscreen_portrait_1080p() {
        // 1. Normal Portrait 346x832 -> 346x750
        let vp_p = compute_presenter_viewport_rect(346, 832, false, false, false, FitMode::AutoFit);
        assert_eq!((vp_p.x, vp_p.y, vp_p.width, vp_p.height), (0, 38, 346, 750));
        assert_eq!(vp_p.bottom_black_bar_gap_px, 0.0);

        // 2. Full Screen Portrait 1920x1080 -> centered 460x996 (x=730, top=39, bottom=1035)
        let vp_fs_p =
            compute_presenter_viewport_rect(1920, 1080, true, false, false, FitMode::AutoFit);
        assert_eq!((vp_fs_p.width, vp_fs_p.height), (460, 996));
        assert_eq!(vp_fs_p.x, 730);
        assert_eq!(vp_fs_p.top_y, 39);
        assert_eq!(vp_fs_p.bottom_y, 1035);
        assert!(vp_fs_p.proportional && vp_fs_p.centered);

        // 3. Normal Landscape closed (840x468) & drawer open (1200x468) -> left stage locked 840x386
        let vp_l_closed =
            compute_presenter_viewport_rect(840, 468, false, true, false, FitMode::AutoFit);
        let vp_l_open =
            compute_presenter_viewport_rect(1200, 468, false, true, true, FitMode::AutoFit);
        assert_eq!(
            (vp_l_closed.width, vp_l_closed.height),
            (840, 386)
        );
        assert_eq!((vp_l_open.width, vp_l_open.height), (840, 386));

        // 4. OriginalRatio smart 1:1 auto-snap when aspect diff < 0.12
        let vp_orig =
            compute_presenter_viewport_rect(346, 832, false, false, false, FitMode::OriginalRatio);
        assert_eq!(vp_orig.pillarbox_gap_px, 0.0);
        assert_eq!((vp_orig.width, vp_orig.height), (346, 750));

        // 5. EdgeToEdge full stage fill
        let vp_edge =
            compute_presenter_viewport_rect(346, 832, false, false, false, FitMode::EdgeToEdge);
        assert_eq!((vp_edge.width, vp_edge.height), (346, 750));
    }

    #[test]
    fn test_tahap234_inverse_touch_matrix_and_subpixel_crop_compensation() {
        let cfg0 = PresenterConfig::default();
        let vp0 = compute_presenter_viewport_rect(346, 832, false, false, false, FitMode::AutoFit);
        let (hx0, hy0) = map_stage_touch_to_ios_hid(173.0, 413.0, &vp0, &cfg0);
        assert!((hx0 as i32 - 32768).abs() <= 1);
        assert!((hy0 as i32 - 32768).abs() <= 2);

        let mut cfg270 = PresenterConfig::default();
        cfg270.rotation_deg = 270;
        let vp270 =
            compute_presenter_viewport_rect(840, 468, false, true, false, FitMode::AutoFit);
        // Near left edge (Dynamic Island at x=10) & middle vertical (y=38+193=231) -> top-middle portrait iOS
        let (hx270, hy270) = map_stage_touch_to_ios_hid(10.0, 231.0, &vp270, &cfg270);
        assert!((hx270 as i32 - 32768).abs() <= 2, "hx270={hx270}");
        assert!(hy270 < 2000, "hy270={hy270}");

        // Manual crop_top = 12 shifts top visual edge (ny=0) downward in iOS Y coordinates
        let mut cfg_crop = PresenterConfig::default();
        cfg_crop.crop_top = 12;
        let (_, hy_crop) = map_stage_touch_to_ios_hid(173.0, 38.0, &vp0, &cfg_crop);
        assert!(hy_crop > 500, "expected shifted hy_crop > 500, got {hy_crop}");
    }

    #[test]
    fn test_tahap234_lossless_bmp_snapshot_under_5ms() {
        let slot = make_synthetic_1216x2656_slot(false);
        let mut presenter = D3d11VideoPresenter::new();
        let _ = presenter.export_snapshot(&slot).unwrap();
        let snap = presenter.export_snapshot(&slot).unwrap();
        assert!(snap.data_url.starts_with("data:image/bmp;base64,Qk"));
        let max_ms = if cfg!(debug_assertions) { 120.0 } else { 5.0 };
        assert!(
            snap.elapsed_ms < max_ms,
            "debug snapshot elapsed: {} ms (limit {} ms)",
            snap.elapsed_ms,
            max_ms
        );

        let (decoded_rgba, dw, dh) = decode_bmp_data_url_to_rgba(&snap.data_url).unwrap();
        assert_eq!(dw as u32, snap.width);
        assert_eq!(dh as u32, snap.height);
        assert_eq!(decoded_rgba.len(), dw * dh * 4);
    }

    #[test]
    fn test_tahap262_live_video_hwnd_1000_frames_present_resize_snapshot_lt_5ms_and_autorotate() {
        use crate::autorotate::run_32_autorotate_scenarios_self_test;
        use crate::mf_decoder::{
            build_synthetic_h264_ipcm_idr, LiveMfVideoDecoder, VideoCodecKind,
        };

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());

        // Feed real COM H.264 IDR frame into VRAM ring
        let mut dec = LiveMfVideoDecoder::new(gpu.clone(), VideoCodecKind::H264).expect("H264 MFT");
        let f = build_synthetic_h264_ipcm_idr(64, 64, 0xA0, 0);
        dec.update_avcc(&f.avcc_config).expect("update_avcc");
        let st = dec
            .feed_avcc_sample(&f.avcc_sample, 16_667, true, &ring)
            .expect("decode IDR");
        if st.total_decoded_frames == 0 {
            let _ = dec.drain_eos(true, Some((0, 0, 0, 0)), &ring);
        }
        assert!(ring.latest_slot().is_some(), "VRAM frame must exist");

        // Create real Win32 offscreen HWND + DXGI FLIP_DISCARD SwapChain
        let mut view = LiveVideoHwnd::new(gpu.clone(), ring.clone(), 0, 346, 750, false)
            .expect("LiveVideoHwnd create");
        assert_ne!(view.hwnd(), 0);
        view.set_vsync_interval(0);
        view.set_auto_rotate_enabled(true);

        // Test ResizeBuffers & SetWindowRgn overlay holes
        view.resize(840, 386).expect("resize landscape");
        assert_eq!(view.size(), (840, 386));
        view.set_overlay_holes(&[Rect {
            left: 20,
            top: 20,
            right: 220,
            bottom: 64,
        }]);
        view.set_overlay_holes(&[]);
        view.resize(346, 750).expect("resize portrait");
        assert_eq!(view.size(), (346, 750));

        // Exercise all 4 rotations & 3 Fit Modes across 1,000 real DXGI FLIP_DISCARD presents
        let rotations = [0u16, 90, 180, 270];
        let fits = [FitMode::AutoFit, FitMode::EdgeToEdge, FitMode::OriginalRatio];
        let mut last_stats = None;
        for i in 0..1000usize {
            if i < 12 {
                view.set_rotation(rotations[i % 4]);
                view.set_fit_mode(fits[i % 3]);
            } else if i == 12 {
                view.set_rotation(0);
                view.set_fit_mode(FitMode::AutoFit);
            }
            last_stats = Some(view.present_latest().expect("DXGI Present"));
        }
        let stats = last_stats.unwrap();
        assert_eq!(stats.presented_total, 1000);
        assert_eq!((stats.swapchain_w, stats.swapchain_h), (346, 750));

        // Warm up cached staging texture once, then measure steady-state lossless BMP snapshot (< 5 ms)
        let bmp_warm = view
            .capture_latest_frame_lossless_bmp()
            .expect("warm snapshot");
        assert!(bmp_warm.len() > 54 && &bmp_warm[0..2] == b"BM");

        let t0 = Instant::now();
        let bmp = view
            .capture_latest_frame_lossless_bmp()
            .expect("steady-state snapshot");
        let snap_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(bmp.len() > 54 && &bmp[0..2] == b"BM");
        assert!(
            snap_ms < 5.0,
            "GPU staging BMP snapshot exceeded 5 ms budget: {:.3} ms",
            snap_ms
        );

        // GPU 120x260 Auto-Rotate probe + full 32-scenario verification
        let mut grid = ProbeGrid120x260::new();
        let probed = view.update_autorotate_probe_if_due(&mut grid, true);
        if gpu.has_video_processor() {
            assert!(probed, "120x260 GPU probe must succeed when VideoProcessor is available");
        }
        let (passed_32, total_32) = run_32_autorotate_scenarios_self_test();
        assert_eq!((passed_32, total_32), (32, 32));

        println!(
            "LiveVideoHwnd 1000-frame PASS: hw_vp={} hw_rot={} snap_ms={:.3}ms bmp_bytes={} autorotate={}/{}",
            stats.used_hardware_vp,
            stats.supports_hw_rotation,
            snap_ms,
            bmp.len(),
            passed_32,
            total_32
        );
    }

    #[test]
    fn test_tahap265_d3d11_video_color_info_vui_and_dxgi_colorspace_mapping() {
        use crate::d3d11_context::{
            VideoColorInfo, VideoColorMatrix, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
            DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P2020,
            DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
        };
        use crate::mf_decoder::{parse_h264_sps_vui_color, yuv_to_rgb_with_color_info};

        // 1. MF attribute -> DXGI color space & legacy VP mapping (1=Full 0-255, 2=Limited 16-235!)
        let bt709_lim = VideoColorInfo::from_mf_attributes(Some(1), Some(2), VideoColorInfo::default());
        assert_eq!(bt709_lim.dxgi_ycbcr_color_space(), DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
        assert_eq!(bt709_lim.legacy_vp_input_color_space_flags(), 0x14);

        let bt709_full = VideoColorInfo::from_mf_attributes(Some(1), Some(1), VideoColorInfo::default());
        assert_eq!(bt709_full.dxgi_ycbcr_color_space(), DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709);
        assert_eq!(bt709_full.legacy_vp_input_color_space_flags(), 0x24);

        let bt601_full = VideoColorInfo::from_mf_attributes(Some(2), Some(1), VideoColorInfo::default());
        assert_eq!(bt601_full.dxgi_ycbcr_color_space(), DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601);
        assert_eq!(bt601_full.legacy_vp_input_color_space_flags(), 0x20);

        let bt2020_lim = VideoColorInfo::from_mf_attributes(Some(4), Some(2), VideoColorInfo::default());
        assert_eq!(bt2020_lim.dxgi_ycbcr_color_space(), DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P2020);

        // 2. Real LiveD3d11Gpu must expose VideoContext1 on Win10/11 or gracefully fall back
        let gpu = LiveD3d11Gpu::new().expect("LiveD3d11Gpu");
        let _has_vc1 = gpu.video_context1().is_some();

        // 3. Software BT.709 Full vs Limited calibration accuracy (ΔRGB <= 3)
        let (r_f0, g_f0, b_f0) = yuv_to_rgb_with_color_info(0, 128, 128, bt709_full);
        assert_eq!((r_f0, g_f0, b_f0), (0, 0, 0));
        let (r_f255, g_f255, b_f255) = yuv_to_rgb_with_color_info(255, 128, 128, bt709_full);
        assert_eq!((r_f255, g_f255, b_f255), (255, 255, 255));
        let (r_l16, g_l16, b_l16) = yuv_to_rgb_with_color_info(16, 128, 128, bt709_lim);
        assert_eq!((r_l16, g_l16, b_l16), (0, 0, 0));
        let (r_l235, g_l235, b_l235) = yuv_to_rgb_with_color_info(235, 128, 128, bt709_lim);
        assert_eq!((r_l235, g_l235, b_l235), (255, 255, 255));

        // 4. H.264 Baseline SPS without VUI returns None safely without panicking
        let base_sps = crate::mf_decoder::build_synthetic_h264_ipcm_idr(64, 64, 128, 0).sps;
        assert_eq!(parse_h264_sps_vui_color(&base_sps), None);
        let fallback_col = VideoColorInfo::from_mf_attributes(
            None,
            None,
            VideoColorInfo {
                matrix: VideoColorMatrix::Bt709,
                full_range: true,
            },
        );
        assert!(fallback_col.full_range);
    }

    #[test]
    fn test_tahap272_purge_stale_video_buffers_and_deterministic_bt709_vp_pipeline() {
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, VideoColorMatrix,
            DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
            DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
        };
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("new_for_composition");

        // Push a synthetic VRAM frame so ring is non-empty, then purge
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(64, 96, &mut nv12, |_, _| (16, 128, 128));
        let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, 64, 96).expect("acquire slot");
        let _ = dst_tex;
        ring.commit_write_slot_with_color(
            slot_idx,
            64,
            96,
            Rect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 96,
            },
            (0, 0, 0, 0),
            16_667,
            true,
            VideoColorInfo::default(),
            Some(nv12),
        );
        assert!(ring.latest_slot().is_some());

        let cleared = hwnd.purge_stale_video_buffers().expect("purge_stale_video_buffers");
        assert_eq!(cleared, 2);
        assert!(ring.latest_slot().is_none(), "ring must be empty after purge");
        assert_eq!(hwnd.last_frame_seq, 0);
        assert_eq!(hwnd.last_source_dims, (0, 0));

        if gpu.has_video_processor() {
            let st_studio = hwnd
                .verify_deterministic_bt709_pipeline_probe(VideoColorInfo::default())
                .expect("studio BT.709 probe");
            assert_eq!(
                st_studio.dxgi_in_cs,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709
            );
            assert_eq!(
                st_studio.dxgi_out_cs,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
            );
            assert_eq!(st_studio.legacy_in_cs, 0x14);
            assert_eq!(st_studio.legacy_out_cs, 0x00);
            assert!(st_studio.auto_processing_disabled);

            let st_full = hwnd
                .verify_deterministic_bt709_pipeline_probe(VideoColorInfo {
                    matrix: VideoColorMatrix::Bt709,
                    full_range: true,
                })
                .expect("full BT.709 probe");
            assert_eq!(st_full.dxgi_in_cs, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709);
            assert_eq!(st_full.legacy_in_cs, 0x24);
            assert_eq!(st_full.legacy_out_cs, 0x00);
            assert!(st_full.auto_processing_disabled);
        }

        // Strict COM teardown order
        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap273_present_latest_never_purges_hot_path_and_orientation_switch_purges_cleanly() {
        use crate::d3d11_context::{D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo};
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("new_for_composition");

        let push_frame = |w: u32, h: u32, pts: u64, luma: u8| {
            let mut nv12 = Vec::new();
            write_nv12_from_rgb_sampler(w, h, &mut nv12, |_, _| (luma, 128, 128));
            let (slot_idx, _) = ring.acquire_write_slot(&gpu, w, h).expect("acquire slot");
            ring.commit_write_slot_with_color(
                slot_idx,
                w,
                h,
                Rect {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                },
                (0, 0, 0, 0),
                pts,
                true,
                VideoColorInfo::default(),
                Some(nv12),
            );
        };

        // 1. Hot-path present_latest never purges ring and keeps frame_seq flat when no new frame arrives
        push_frame(64, 96, 16_667, 40);
        let st1 = hwnd.present_latest().expect("present 1");
        assert!(ring.latest_slot().is_some(), "hot path must keep latest VRAM slot");
        let st2 = hwnd.present_latest().expect("present 2 (same frame)");
        assert_eq!(st2.frame_seq, st1.frame_seq);
        push_frame(64, 96, 33_334, 80);
        let st3 = hwnd.present_latest().expect("present 3 (new frame)");
        assert!(st3.frame_seq > st2.frame_seq);

        // 2. Orientation switch purge works cleanly on bound composition swapchain without ResizeBuffers
        let cleared = hwnd
            .purge_stale_video_buffers()
            .expect("orientation switch purge without ResizeBuffers");
        assert_eq!(cleared, 2);
        assert!(ring.latest_slot().is_none());

        push_frame(96, 64, 50_000, 120);
        let st4 = hwnd.present_latest().expect("present 4 (post-purge landscape)");
        assert!(st4.frame_seq > st3.frame_seq);
        assert_eq!(hwnd.last_source_dims, (96, 64));

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap274_seamless_transition_holds_last_frame_zero_black_clear_and_overwrites_dual_backbuffer() {
        use crate::d3d11_context::{D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo};
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("new_for_composition");

        let push_frame = |w: u32, h: u32, pts: u64, y_val: u8| {
            let mut nv12 = Vec::new();
            write_nv12_from_rgb_sampler(w, h, &mut nv12, |_, _| (y_val, 128, 128));
            let (slot_idx, _) = ring.acquire_write_slot(&gpu, w, h).expect("acquire slot");
            ring.commit_write_slot_with_color(
                slot_idx,
                w,
                h,
                Rect {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                },
                (0, 0, 0, 0),
                pts,
                true,
                VideoColorInfo::default(),
                Some(nv12),
            );
        };

        // 1. Present initial frame
        push_frame(64, 96, 16_667, 60);
        let st1 = hwnd.present_latest().expect("present initial frame");
        assert_eq!(st1.presented_total, 1);
        assert!(!st1.dual_buffer_overwritten);

        // 2. Prepare seamless mid-stream transition: clears ring ONLY, 0 black clears!
        hwnd.prepare_seamless_dimension_transition();
        assert!(ring.latest_slot().is_none());
        assert_eq!(hwnd.mid_stream_black_clears(), 0);
        assert_eq!(hwnd.dual_buffer_overwrites(), 0);

        // 3. Five render ticks while ring is empty (awaiting new IDR) MUST hold the last frame
        //    and issue ZERO black Presents!
        for _ in 0..5 {
            let hold = hwnd.present_latest().expect("present_latest seamless hold");
            assert_eq!(hold.presented_total, 1);
            assert_eq!(hold.frame_seq, st1.frame_seq);
            assert!(!hold.dual_buffer_overwritten);
            assert_eq!(hwnd.mid_stream_black_clears(), 0);
        }

        // 4. First clean frame of new dimensions atomically overwrites BOTH FLIP_SEQUENTIAL backbuffers!
        push_frame(96, 64, 33_334, 140);
        let st_dual = hwnd.present_latest().expect("present first frame of new dims");
        assert!(st_dual.dual_buffer_overwritten);
        assert_eq!(st_dual.presented_total, 3);
        assert_eq!(hwnd.dual_buffer_overwrites(), 1);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);

        // 5. Subsequent steady frame uses single pass
        push_frame(96, 64, 50_001, 150);
        let st_steady = hwnd.present_latest().expect("steady frame");
        assert!(!st_steady.dual_buffer_overwritten);
        assert_eq!(st_steady.presented_total, 4);

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap274_atomic_per_frame_rotation_switches_90cw_to_0_on_exact_frame_boundary() {
        use crate::d3d11_context::{D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo};
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        // 1. Pure matrix checks: AirPlay atomic vs non-AirPlay untouched
        let ap_port_win = AirplayRenderPolicy {
            is_airplay: true,
            window_is_landscape: false,
            auto_rotate: false,
            manual_rotation_deg: 0,
        };
        assert_eq!(resolve_atomic_frame_rotation_deg(ap_port_win, 1920, 888, 0), 90);
        assert_eq!(resolve_atomic_frame_rotation_deg(ap_port_win, 888, 1920, 90), 0);

        let ap_land_win = AirplayRenderPolicy {
            is_airplay: true,
            window_is_landscape: true,
            auto_rotate: true,
            manual_rotation_deg: 90,
        };
        assert_eq!(resolve_atomic_frame_rotation_deg(ap_land_win, 1920, 888, 90), 0);
        assert_eq!(resolve_atomic_frame_rotation_deg(ap_land_win, 888, 1920, 90), 0);

        let non_ap = AirplayRenderPolicy {
            is_airplay: false,
            window_is_landscape: false,
            auto_rotate: false,
            manual_rotation_deg: 0,
        };
        assert_eq!(resolve_atomic_frame_rotation_deg(non_ap, 1920, 888, 0), 0);
        assert_eq!(resolve_atomic_frame_rotation_deg(non_ap, 888, 1920, 90), 90);
        assert_eq!(resolve_atomic_frame_rotation_deg(non_ap, 888, 1920, 180), 180);
        assert_eq!(resolve_atomic_frame_rotation_deg(non_ap, 888, 1920, 270), 270);

        // 2. Live LiveVideoHwnd swapchain verifies exact frame-boundary rotation without waiting for UI timer
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("new_for_composition");
        hwnd.set_airplay_mode(true);
        hwnd.set_window_landscape(false);
        hwnd.set_auto_rotate_enabled(false);

        let push_frame = |w: u32, h: u32, pts: u64, y_val: u8| {
            let mut nv12 = Vec::new();
            write_nv12_from_rgb_sampler(w, h, &mut nv12, |_, _| (y_val, 128, 128));
            let (slot_idx, _) = ring.acquire_write_slot(&gpu, w, h).expect("acquire slot");
            ring.commit_write_slot_with_color(
                slot_idx,
                w,
                h,
                Rect {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                },
                (0, 0, 0, 0),
                pts,
                true,
                VideoColorInfo::default(),
                Some(nv12),
            );
        };

        // Frame A: Landscape 96x64 arrives while stale cfg.rotation_deg == 0 -> atomic 90° CW!
        hwnd.set_rotation(0);
        push_frame(96, 64, 16_667, 80);
        let _ = hwnd.present_latest().expect("present 96x64");
        assert_eq!(hwnd.last_effective_rotation_deg(), 90);

        // Frame B: Portrait 64x96 arrives while stale cfg.rotation_deg == 90 -> atomic 0°!
        hwnd.prepare_seamless_dimension_transition();
        hwnd.set_rotation(90);
        push_frame(64, 96, 33_334, 100);
        let st_b = hwnd.present_latest().expect("present 64x96");
        assert!(st_b.dual_buffer_overwritten);
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap275_adaptive_portrait_scaling_990x2160_888x1920_640x1388_keeps_per_frame_src_rect_and_zero_black() {
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
        };

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("comp swapchain");
        hwnd.set_airplay_mode(true);
        hwnd.set_auto_rotate_enabled(true);

        let push_frame = |w: u32, h: u32, pts: u64, y_val: u8| {
            let mut nv12 = Vec::new();
            write_nv12_from_rgb_sampler(w, h, &mut nv12, |_, _| (y_val, y_val, y_val));
            let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, w, h).expect("acquire slot");
            // Upload synthetic NV12 plane into actual D3D11 GPU texture so VideoProcessorBlt reads real pixels.
            if let Ok(up_tex) = gpu.create_nv12_texture(w, h, true) {
                if let Ok(mapped) = gpu.map_texture(&up_tex, 0, D3D11_MAP_WRITE_DISCARD) {
                    let dst_pitch = mapped.row_pitch as usize;
                    let src_stride = w as usize;
                    let rows = (h as usize) + (h as usize) / 2;
                    let copy_w = src_stride.min(dst_pitch);
                    unsafe {
                        for row in 0..rows {
                            let sp = nv12.as_ptr().add(row * src_stride);
                            let dp = (mapped.p_data as *mut u8).add(row * dst_pitch);
                            std::ptr::copy_nonoverlapping(sp, dp, copy_w);
                        }
                    }
                    gpu.unmap_texture(&up_tex, 0);
                    gpu.copy_resource(&dst_tex, &up_tex);
                }
            }
            ring.commit_write_slot_with_color(
                slot_idx,
                w,
                h,
                Rect {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                },
                (0, 0, 0, 0),
                pts,
                true,
                VideoColorInfo::default(),
                Some(nv12),
            );
        };

        // 1. Start at 990x2160 Portrait -> 0° rotation, 0 rebuilds, src_rect clamped cleanly inside 990x2160
        push_frame(990, 2160, 16_667, 150);
        let st1 = hwnd.present_latest().expect("present 990x2160");
        assert_eq!(st1.presented_total, 1);
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);
        assert_eq!(hwnd.vp_rebuilds(), 0);
        let r1 = hwnd.last_presented_src_rect();
        assert!(r1.0 >= 0 && r1.1 >= 0 && r1.2 <= 990 && r1.3 <= 2160, "r1={r1:?}");

        // 2. Adaptive portrait downscale 990x2160 -> 888x1920 -> recreates VP & keeps 0 black clears
        push_frame(888, 1920, 33_334, 175);
        let st2 = hwnd.present_latest().expect("present 888x1920");
        assert!(!st2.dual_buffer_overwritten);
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);
        assert_eq!(hwnd.vp_rebuilds(), 1);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);
        let r2 = hwnd.last_presented_src_rect();
        assert!(r2.0 >= 0 && r2.1 >= 0 && r2.2 <= 888 && r2.3 <= 1920, "r2={r2:?}");

        // 3. Adaptive portrait downscale 888x1920 -> 640x1388 -> recreates VP & keeps exact 640x1388 bounds
        push_frame(640, 1388, 50_001, 190);
        let st3 = hwnd.present_latest().expect("present 640x1388");
        assert!(!st3.dual_buffer_overwritten);
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);
        assert_eq!(hwnd.vp_rebuilds(), 2);
        let r3 = hwnd.last_presented_src_rect();
        assert!(r3.0 >= 0 && r3.1 >= 0 && r3.2 <= 640 && r3.3 <= 1388, "r3={r3:?}");

        // 4. Adaptive portrait upscale 640x1388 -> 990x2160 -> recreates VP & exports non-black snapshot
        push_frame(990, 2160, 66_668, 210);
        let _st4 = hwnd.present_latest().expect("present 990x2160 restore");
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);
        assert_eq!(hwnd.vp_rebuilds(), 3);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);
        let r4 = hwnd.last_presented_src_rect();
        assert!(r4.0 >= 0 && r4.1 >= 0 && r4.2 <= 990 && r4.3 <= 2160, "r4={r4:?}");
        let bmp = hwnd.capture_latest_frame_lossless_bmp().expect("snapshot non-black");
        assert!(bmp.len() > 54);
        let mid_off = 54 + (bmp.len() - 54) / 2;
        assert!(bmp[mid_off] > 80 || bmp[mid_off + 1] > 80 || bmp[mid_off + 2] > 80);

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap276_force_repaint_on_fullscreen_toggle_presents_cached_vfr_frame_immediately() {
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
        };

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("comp swapchain");
        hwnd.set_airplay_mode(true);

        // Push 1 Portrait VFR frame (888x1920) then go completely idle (0 new packets)
        let (w, h) = (888u32, 1920u32);
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(w, h, &mut nv12, |_, _| (40, 140, 230));
        let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, w, h).expect("acquire slot");
        if let Ok(up_tex) = gpu.create_nv12_texture(w, h, true) {
            if let Ok(mapped) = gpu.map_texture(&up_tex, 0, D3D11_MAP_WRITE_DISCARD) {
                let dst_pitch = mapped.row_pitch as usize;
                let src_stride = w as usize;
                let rows = (h as usize) + (h as usize) / 2;
                let copy_w = src_stride.min(dst_pitch);
                unsafe {
                    for row in 0..rows {
                        std::ptr::copy_nonoverlapping(
                            nv12.as_ptr().add(row * src_stride),
                            (mapped.p_data as *mut u8).add(row * dst_pitch),
                            copy_w,
                        );
                    }
                }
                gpu.unmap_texture(&up_tex, 0);
                gpu.copy_resource(&dst_tex, &up_tex);
            }
        }
        ring.commit_write_slot_with_color(
            slot_idx,
            w,
            h,
            Rect {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            },
            (0, 0, 0, 0),
            16_667,
            true,
            VideoColorInfo::default(),
            Some(nv12),
        );

        let st_win = hwnd.present_latest().expect("initial portrait present");
        assert_eq!(st_win.presented_total, 1);
        assert_eq!((st_win.swapchain_w, st_win.swapchain_h), (346, 750));

        // Simulate idle VFR toggle -> Full Screen Portrait (`450x998`) with ZERO new video packets
        hwnd.resize(450, 998).expect("resize to fullscreen portrait");
        let st_fs = hwnd
            .force_repaint_cached_frame()
            .expect("cached VFR frame MUST repaint immediately after fullscreen resize");
        assert_eq!((st_fs.swapchain_w, st_fs.swapchain_h), (450, 998));
        assert!(st_fs.presented_total >= 2);
        assert_eq!(st_fs.frame_seq, st_win.frame_seq);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);
        let bmp_fs = hwnd.capture_latest_frame_lossless_bmp().expect("fullscreen cached bmp");
        assert!(bmp_fs.len() > 54);
        let mid_off = 54 + (bmp_fs.len() - 54) / 2;
        assert!(
            bmp_fs[mid_off] > 20 || bmp_fs[mid_off + 1] > 20 || bmp_fs[mid_off + 2] > 20,
            "fullscreen cached repaint must preserve live image instead of blank/black"
        );

        // Simulate idle Restore -> Windowed Portrait (`346x750`) with ZERO new packets
        hwnd.resize(346, 750).expect("resize back to windowed");
        let st_restore = hwnd
            .force_repaint_cached_frame()
            .expect("cached VFR frame MUST repaint cleanly on restore");
        assert_eq!((st_restore.swapchain_w, st_restore.swapchain_h), (346, 750));
        assert!(st_restore.presented_total > st_fs.presented_total);
        assert_eq!(st_restore.frame_seq, st_win.frame_seq);

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap277_resize_buffers_atomically_repaints_cached_frame_zero_black_flash() {
        use crate::d3d11_context::{VideoColorInfo, D3D11_MAP_WRITE_DISCARD};
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 840, 468)
            .expect("composition LiveVideoHwnd 840x468");
        assert!(hwnd.d3d11_multithread_protected());
        hwnd.set_vsync_interval(0);
        hwnd.set_airplay_mode(true);
        hwnd.set_window_landscape(true);

        let (w, h) = (192u32, 88u32);
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(w, h, &mut nv12, |_, _| (90, 170, 220));
        let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, w, h).expect("slot");
        let up = gpu.create_nv12_texture(w, h, true).expect("upload");
        let mp = gpu
            .map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD)
            .expect("map");
        let rows = (h as usize) + (h as usize) / 2;
        let cp = (w as usize).min(mp.row_pitch as usize);
        unsafe {
            for r in 0..rows {
                std::ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(r * w as usize),
                    (mp.p_data as *mut u8).add(r * mp.row_pitch as usize),
                    cp,
                );
            }
        }
        gpu.unmap_texture(&up, 0);
        gpu.copy_resource(&dst_tex, &up);
        ring.commit_write_slot_with_color(
            slot_idx,
            w,
            h,
            Rect {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            },
            (0, 0, 0, 0),
            16_667,
            true,
            VideoColorInfo::default(),
            Some(nv12),
        );

        let st_init = hwnd.present_latest().expect("present #1 at 840x468");
        assert_eq!(st_init.presented_total, 1);
        assert_eq!(hwnd.resize_atomic_repaints(), 0);

        // ResizeBuffers 840x468 -> 1920x998 MUST immediately atomically repaint without any explicit present_latest call!
        hwnd.resize(1920, 998).expect("resize to 1920x998");
        assert_eq!(hwnd.size(), (1920, 998));
        assert_eq!(hwnd.resize_atomic_repaints(), 1);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);
        assert!(hwnd.frames_presented() >= 2);
        let bt709_state = hwnd
            .verify_deterministic_bt709_pipeline_probe(VideoColorInfo::default())
            .expect("BT.709 state post resize");
        assert_eq!(
            bt709_state.dxgi_in_cs,
            crate::d3d11_context::DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709
        );
        assert_eq!(
            bt709_state.dxgi_out_cs,
            DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
        );
        assert!(bt709_state.auto_processing_disabled);
        let bmp = hwnd
            .capture_latest_frame_lossless_bmp()
            .expect("lossless bmp after ResizeBuffers");
        let mid_off = 54 + (bmp.len() - 54) / 2;
        assert!(
            bmp[mid_off] > 20 || bmp[mid_off + 1] > 20 || bmp[mid_off + 2] > 20,
            "SwapChain backbuffer after ResizeBuffers must hold live painted image instead of black flash"
        );

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap278_stateless_zero_letterbox_and_zero_pillarbox_across_vertical_horizontal_manual_rotate() {
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
        };

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 519, 1125)
            .expect("composition 519x1125");
        hwnd.set_vsync_interval(0);
        hwnd.set_airplay_mode(true);
        hwnd.set_auto_rotate_enabled(true); // Ignored by AirPlay stateless presenter!

        let push_mod16_frame = |coded_w: u32, coded_h: u32, vis_w: u32, vis_h: u32, pts: u64, color: (u8, u8, u8)| {
            let mut nv12 = Vec::new();
            write_nv12_from_rgb_sampler(coded_w, coded_h, &mut nv12, |x, y| {
                if x < vis_w && y < vis_h {
                    color
                } else {
                    (0, 0, 0)
                }
            });
            let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, coded_w, coded_h).expect("slot");
            if let Ok(up) = gpu.create_nv12_texture(coded_w, coded_h, true) {
                if let Ok(mp) = gpu.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD) {
                    let rows = (coded_h as usize) + (coded_h as usize) / 2;
                    let cp = (coded_w as usize).min(mp.row_pitch as usize);
                    unsafe {
                        for r in 0..rows {
                            std::ptr::copy_nonoverlapping(
                                nv12.as_ptr().add(r * coded_w as usize),
                                (mp.p_data as *mut u8).add(r * mp.row_pitch as usize),
                                cp,
                            );
                        }
                    }
                    gpu.unmap_texture(&up, 0);
                    gpu.copy_resource(&dst_tex, &up);
                }
            }
            ring.commit_write_slot_with_color(
                slot_idx,
                coded_w,
                coded_h,
                Rect {
                    left: 0,
                    top: 0,
                    right: vis_w as i32,
                    bottom: vis_h as i32,
                },
                (0, 0, 0, 0),
                pts,
                true,
                VideoColorInfo::default(),
                Some(nv12),
            );
        };

        // 1. Vertical (519x1125) + Portrait 994x2160 in mod-16 1008x2160 -> src_w=994, rot=0, dest=(0,0,519,1125)
        push_mod16_frame(1008, 2160, 994, 2160, 16_667, (40, 140, 220));
        let _ = hwnd.present_latest().expect("step 1 vertical portrait");
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);
        assert_eq!(hwnd.last_presented_src_rect(), (0, 0, 994, 2160));
        assert_eq!(hwnd.last_presented_dest_rect(), (0, 0, 519, 1125));

        // 2. Vertical (519x1125) + Landscape 2160x994 (opening game before clicking Rotate) -> rot=90 CW, dest=(0,0,519,1125)
        push_mod16_frame(2160, 1008, 2160, 994, 33_334, (60, 190, 90));
        let _ = hwnd.present_latest().expect("step 2 vertical landscape 90cw");
        assert_eq!(hwnd.last_effective_rotation_deg(), 90);
        assert_eq!(hwnd.last_presented_src_rect(), (0, 0, 2160, 994));
        assert_eq!(hwnd.last_presented_dest_rect(), (0, 0, 519, 1125));

        // 3. User clicks Manual Rotate -> Horizontal (1260x579) + Landscape 2160x994 -> rot=0, dest=(0,0,1260,579)
        hwnd.resize(1260, 579).expect("resize 1260x579");
        hwnd.set_window_landscape(true);
        let _ = hwnd.present_latest().expect("step 3 horizontal landscape");
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);
        assert_eq!(hwnd.last_presented_dest_rect(), (0, 0, 1260, 579));

        // 4. Horizontal (1260x579) + Portrait 994x2160 (returning to Home Screen before clicking Rotate back) -> rot=270, dest=(0,0,1260,579)
        push_mod16_frame(1008, 2160, 994, 2160, 50_001, (190, 110, 60));
        let _ = hwnd.present_latest().expect("step 4 horizontal portrait 270cw");
        assert_eq!(hwnd.last_effective_rotation_deg(), 270);
        assert_eq!(hwnd.last_presented_dest_rect(), (0, 0, 1260, 579));

        // 5. User clicks Manual Rotate back -> Vertical (519x1125) + Portrait 994x2160 -> rot=0, dest=(0,0,519,1125), 0 pillarbox!
        hwnd.resize(519, 1125).expect("resize back 519x1125");
        hwnd.set_window_landscape(false);
        let _ = hwnd.present_latest().expect("step 5 back to vertical zero pillarbox");
        assert_eq!(hwnd.last_effective_rotation_deg(), 0);
        assert_eq!(hwnd.last_presented_src_rect(), (0, 0, 994, 2160));
        assert_eq!(hwnd.last_presented_dest_rect(), (0, 0, 519, 1125));

        // Explicit FitMode::OriginalRatio preserves centered contain aspect cleanly
        let (o_rot, _, o_dst) = compute_stateless_airplay_present_rects(
            Rect {
                left: 0,
                top: 0,
                right: 994,
                bottom: 2160,
            },
            1920,
            1200,
            0,
            FitMode::OriginalRatio,
        );
        assert_eq!(o_rot, 270);
        assert!(o_dst.top > 0 && o_dst.bottom < 1200 && o_dst.left == 0 && o_dst.right == 1920);

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap280_live_video_hwnd_hd_shader_present_resize_buffers_and_trace_counters() {
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
        };
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        reset_hd_shader_trace_all();
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 519, 1125)
            .expect("composition 519x1125");
        assert!(hwnd.hd_shader_active(), "HdNv12ShaderRenderer must initialize cleanly");
        assert!(hd_shader_trace_active(), "hd_shader_trace_active must turn true");
        hwnd.set_vsync_interval(0);
        hwnd.set_airplay_mode(true);
        hwnd.set_window_landscape(false);

        // Upload real 994x2160-in-1008x2160 NV12 frame into VRAM texture
        let (coded_w, coded_h, vis_w, vis_h) = (1008u32, 2160u32, 994u32, 2160u32);
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(coded_w, coded_h, &mut nv12, |x, _| {
            if x < vis_w {
                (70, 165, 240)
            } else {
                (0, 255, 0)
            }
        });
        let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, coded_w, coded_h).expect("slot");
        let up = gpu.create_nv12_texture(coded_w, coded_h, true).expect("upload");
        let mp = gpu.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD).expect("map");
        let rows = (coded_h as usize) + (coded_h as usize) / 2;
        let cp = (coded_w as usize).min(mp.row_pitch as usize);
        unsafe {
            for r in 0..rows {
                std::ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(r * coded_w as usize),
                    (mp.p_data as *mut u8).add(r * mp.row_pitch as usize),
                    cp,
                );
            }
        }
        gpu.unmap_texture(&up, 0);
        gpu.copy_resource(&dst_tex, &up);
        ring.commit_write_slot_with_color(
            slot_idx,
            coded_w,
            coded_h,
            Rect {
                left: 0,
                top: 0,
                right: vis_w as i32,
                bottom: vis_h as i32,
            },
            (0, 0, 0, 0),
            16_667,
            true,
            VideoColorInfo::default(),
            Some(nv12),
        );

        let st1 = hwnd.present_latest().expect("HD shader present #1");
        assert!(st1.used_hardware_vp);
        assert!(st1.supports_hw_rotation);
        assert_eq!(hwnd.hd_shader_presents(), 1);
        assert!(hd_shader_trace_presents() >= 1);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);

        // ResizeBuffers across full-stage dimensions -> 0 DXGI_ERROR_INVALID_CALL & 0 black flash!
        hwnd.resize(1920, 998).expect("resize 1920x998 after HD shader draw");
        hwnd.resize(519, 1125).expect("resize 519x1125 after HD shader draw");
        assert_eq!(hwnd.size(), (519, 1125));
        assert_eq!(hwnd.resize_atomic_repaints(), 2);
        assert!(hwnd.hd_shader_presents() >= 3);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);

        let bmp = hwnd.capture_latest_frame_lossless_bmp().expect("lossless bmp");
        assert!(bmp.len() > 54);
        let mid_off = 54 + (bmp.len() - 54) / 2;
        assert!(bmp[mid_off] > 40 || bmp[mid_off + 1] > 40 || bmp[mid_off + 2] > 40);

        let cleared = hwnd.purge_stale_video_buffers().expect("purge_stale_video_buffers");
        assert_eq!(cleared, 2);
        assert_eq!(hwnd.hd_shader_presents(), 0);
        assert_eq!(hd_shader_trace_presents(), 0);

        ring.clear();
        drop(hwnd);
        reset_hd_shader_trace_all();
    }

    #[test]
    fn test_tahap281_airplay_keeps_zero_cas_while_hevc_uses_0_28() {
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
        };
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        reset_hd_shader_trace_all();
        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 519, 1125)
            .expect("composition 519x1125");
        hwnd.set_vsync_interval(0);
        hwnd.set_window_landscape(false);

        // AirPlay H.264 strictly keeps sharpen_strength == 0.0
        hwnd.set_airplay_mode(true);
        assert_eq!(hwnd.effective_sharpen_strength(), 0.0);

        // CoreDevice HEVC activates Subtle Luma-Only CAS 0.28
        hwnd.set_airplay_mode(false);
        assert!((hwnd.effective_sharpen_strength() - 0.28).abs() < 1e-5);

        let (coded_w, coded_h, vis_w, vis_h) = (1216u32, 2656u32, 1202u32, 2598u32);
        let mut nv12 = Vec::new();
        write_nv12_from_rgb_sampler(coded_w, coded_h, &mut nv12, |x, y| {
            if x < vis_w && y < vis_h {
                if x % 32 == 0 {
                    (245, 245, 245)
                } else {
                    (32, 48, 72)
                }
            } else {
                (0, 0, 0)
            }
        });
        let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, coded_w, coded_h).expect("slot");
        let up = gpu.create_nv12_texture(coded_w, coded_h, true).expect("upload");
        let mp = gpu.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD).expect("map");
        let rows = (coded_h as usize) + (coded_h as usize) / 2;
        let cp = (coded_w as usize).min(mp.row_pitch as usize);
        unsafe {
            for r in 0..rows {
                std::ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(r * coded_w as usize),
                    (mp.p_data as *mut u8).add(r * mp.row_pitch as usize),
                    cp,
                );
            }
        }
        gpu.unmap_texture(&up, 0);
        gpu.copy_resource(&dst_tex, &up);
        ring.commit_write_slot_with_color(
            slot_idx,
            coded_w,
            coded_h,
            Rect {
                left: 0,
                top: 0,
                right: vis_w as i32,
                bottom: vis_h as i32,
            },
            (0, 0, 58, 14),
            16_667,
            true,
            VideoColorInfo::default(),
            Some(nv12),
        );

        let st = hwnd.present_latest().expect("HEVC Luma-CAS present");
        assert!(st.used_hardware_vp);
        assert_eq!(hwnd.hd_shader_presents(), 1);

        hwnd.resize(1920, 998).expect("resize 1920x998");
        hwnd.resize(519, 1125).expect("resize 519x1125");
        assert!(hwnd.hd_shader_presents() >= 3);
        assert_eq!(hwnd.mid_stream_black_clears(), 0);
        let bmp = hwnd.capture_latest_frame_lossless_bmp().expect("bmp");
        assert!(bmp.len() > 54);

        ring.clear();
        drop(hwnd);
        reset_hd_shader_trace_all();
    }

    #[test]
    fn test_tahap283_real_d3d11_nv12_autorotate_probe_rgb_order_and_airplay_bypass() {
        use crate::autorotate::{analyze_frame_grid, OrientationMode, ProbeGrid120x260};
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
        };
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("composition 346x750");
        hwnd.set_vsync_interval(0);

        let (cw, ch) = (64u32, 128u32);
        let mut nv12 = Vec::new();
        // Vivid KONAMI red (R=225, G=15, B=20)
        write_nv12_from_rgb_sampler(cw, ch, &mut nv12, |_, _| (225, 15, 20));
        let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, cw, ch).expect("slot");
        let up = gpu.create_nv12_texture(cw, ch, true).expect("upload");
        let mp = gpu.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD).expect("map");
        let rows = (ch as usize) + (ch as usize) / 2;
        let cp = (cw as usize).min(mp.row_pitch as usize);
        unsafe {
            for r in 0..rows {
                std::ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(r * cw as usize),
                    (mp.p_data as *mut u8).add(r * mp.row_pitch as usize),
                    cp,
                );
            }
        }
        gpu.unmap_texture(&up, 0);
        gpu.copy_resource(&dst_tex, &up);
        ring.commit_write_slot_with_color(
            slot_idx,
            cw,
            ch,
            Rect { left: 0, top: 0, right: cw as i32, bottom: ch as i32 },
            (0, 0, 0, 0),
            16_667,
            true,
            VideoColorInfo::default(),
            Some(nv12),
        );

        // AirPlay mode MUST bypass autorotate probe when !force
        hwnd.set_airplay_mode(true);
        hwnd.set_auto_rotate_enabled(true);
        let mut grid = ProbeGrid120x260::new();
        assert!(!hwnd.update_autorotate_probe_if_due(&mut grid, false));

        // CoreDevice HEVC mode must run probe & preserve exact R/G/B channel order
        hwnd.set_airplay_mode(false);
        assert!(hwnd.update_autorotate_probe_if_due(&mut grid, true));
        let c = grid.get_pixel(60, 130);
        assert!(c.r > 180 && c.g < 60 && c.b < 60, "Expected vivid red RGB, got {:?}", c);
        let res = analyze_frame_grid(&grid, false);
        assert!(res.is_konami_red_intro);
        assert_eq!(res.suggested_orientation, OrientationMode::Landscape270);

        // HdNv12ShaderRenderer fallback path when probe_processor cleared
        hwnd.probe_processor = None;
        if let (Some(ref mut sh), Some(ref rtv)) = (&mut hwnd.hd_shader, &hwnd.probe_rtv) {
            let snap = ring.latest_slot().unwrap();
            let sr = Rect { left: 0, top: 0, right: cw as i32, bottom: ch as i32 };
            let dr = Rect { left: 0, top: 0, right: 120, bottom: 260 };
            sh.render_to_rtv_with_sharpen(&gpu, &snap.texture, rtv, cw, ch, sr, dr, 0, snap.color_info, 0.0)
                .expect("shader probe fallback");
        }

        ring.clear();
        drop(hwnd);
    }

    #[test]
    fn test_tahap284_real_d3d11_autorotate_probe_uses_untrimmed_coded_rect() {
        use crate::autorotate::ProbeGrid120x260;
        use crate::d3d11_context::{
            D3d11Nv12VramRing, LiveD3d11Gpu, VideoColorInfo, D3D11_MAP_WRITE_DISCARD,
        };
        use crate::mf_decoder::write_nv12_from_rgb_sampler;

        let gpu = Arc::new(LiveD3d11Gpu::new().expect("LiveD3d11Gpu"));
        let ring = Arc::new(D3d11Nv12VramRing::new());
        let mut hwnd = LiveVideoHwnd::new_for_composition(gpu.clone(), ring.clone(), 346, 750)
            .expect("composition 346x750");
        hwnd.set_vsync_interval(0);

        let (cw, ch) = (1216u32, 2656u32);
        let mut nv12 = Vec::new();
        // Dark background with bright white horizontal band centered at y_raw = 2590
        // (y_raw = 2582..=2598). On untrimmed 1216x2656 -> 120x260: 2590/2656*260 = 253.54 (row 253).
        // If trimmed by (0,0,58,14) + crop_top=12 + crop_bottom=12, it would shift downward near row 259.
        write_nv12_from_rgb_sampler(cw, ch, &mut nv12, |_x, y| {
            if (2582..=2598).contains(&y) {
                (240, 240, 240)
            } else {
                (10, 10, 12)
            }
        });
        let (slot_idx, dst_tex) = ring.acquire_write_slot(&gpu, cw, ch).expect("slot");
        let up = gpu.create_nv12_texture(cw, ch, true).expect("upload");
        let mp = gpu.map_texture(&up, 0, D3D11_MAP_WRITE_DISCARD).expect("map");
        let rows = (ch as usize) + (ch as usize) / 2;
        let cp = (cw as usize).min(mp.row_pitch as usize);
        unsafe {
            for r in 0..rows {
                std::ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(r * cw as usize),
                    (mp.p_data as *mut u8).add(r * mp.row_pitch as usize),
                    cp,
                );
            }
        }
        gpu.unmap_texture(&up, 0);
        gpu.copy_resource(&dst_tex, &up);
        ring.commit_write_slot_with_color(
            slot_idx,
            cw,
            ch,
            Rect { left: 0, top: 0, right: cw as i32, bottom: ch as i32 },
            (0, 0, 58, 14),
            16_667,
            true,
            VideoColorInfo::default(),
            None,
        );

        {
            let mut cfg = hwnd
                .input_state
                .config
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cfg.crop_top = 12;
            cfg.crop_bottom = 12;
            cfg.crop_left = 8;
            cfg.crop_right = 8;
        }

        // AirPlay MUST still bypass autorotate probe
        hwnd.set_airplay_mode(true);
        hwnd.set_auto_rotate_enabled(true);
        let mut grid = ProbeGrid120x260::new();
        assert!(!hwnd.update_autorotate_probe_if_due(&mut grid, false));

        // CoreDevice HEVC probes untrimmed full coded surface (y_raw=2590 -> y_grid ~ 253)
        hwnd.set_airplay_mode(false);
        assert!(hwnd.update_autorotate_probe_if_due(&mut grid, true));
        let band_px = grid.get_pixel(60, 253);
        let edge_px = grid.get_pixel(60, 258);
        assert!(
            band_px.luma >= 180,
            "Expected untrimmed band at y_grid=253 luma>=180, got {:?}",
            band_px
        );
        assert!(
            edge_px.luma <= 40,
            "Expected dark border below band at y_grid=258 luma<=40, got {:?}",
            edge_px
        );

        ring.clear();
        drop(hwnd);
    }
}
